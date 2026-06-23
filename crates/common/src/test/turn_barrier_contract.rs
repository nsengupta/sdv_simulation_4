//! Phase 4 — `VecDeque<TurnBarrier>` reorder-buffer: ordering invariants (RED → GREEN).
//!
//! ## Why these tests are RED in Phase 3
//!
//! Phase 3 uses `pending_turn + fsm_backlog`: a second `Fsm` event arriving while a zone
//! tell is in flight sits in `fsm_backlog` **without a turn_id**.  A manually injected
//! `ZoneReady { turn_id: N+1 }` does not match `pending_turn { turn_id: N }` and is
//! **dropped**.  After the front turn resolves, `pump_fsm_backlog` starts the second
//! event with the SAME `turn_id` (N+1) but a fresh zone tell to the silent headlamp;
//! eventually all retries exhaust and a synthetic reply is committed, which carries a
//! `LogWarning` action.
//!
//! Phase 4 gives every `Fsm` event its own `TurnBarrier` immediately.  The injected
//! `ZoneReply` is stored in that barrier; when the front barrier drains, the rear one
//! drains with its **real** reply — no synthetic, no `LogWarning`.
//!
//! ## RED assertion
//!
//! `rows[1].actions.is_empty()` — in Phase 3 the rear event's synthetic reply injects
//! `PublishedDomainAction::LogWarning`; in Phase 4 the real injected reply has no outcomes.
//!
//! ## Timing discipline
//!
//! `ZONE_TELL_BACK_WAIT` in test mode is 50 ms.  All manual injections happen within
//! ~5 ms of event submission (well before the first retry), so `tell_attempt` is still 0
//! in both the actor's wait state and our injected message.

use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::digital_twin::{DigitalTwinCarVocabulary, ZoneReply};
use crate::fsm::{FsmEvent, FsmState, HeadlampState, ZoneId};
use crate::published::{PublishedDomainAction, PublishedTransitionRecord};
use crate::test::ActorGuard;
use crate::twin_runtime::controller::vehicle_controller::VehicleControllerRuntimeOptions;
use crate::vehicle_physics::LUX_ON_THRESHOLD;
use crate::vehicle_state::{HeadlampContext, HeadlampZoneReply};
use crate::VehicleController;

// ── helpers ─────────────────────────────────────────────────────────────────

fn zone_reply(state: HeadlampState) -> ZoneReply {
    ZoneReply::Headlamp(HeadlampZoneReply {
        ctx: HeadlampContext {
            state,
            ack_pending_since: if matches!(
                state,
                HeadlampState::OnRequested | HeadlampState::OffRequested
            ) {
                Some(Instant::now())
            } else {
                None
            },
        },
        outcomes: vec![],
    })
}

async fn drain_n(
    rx: &mut mpsc::Receiver<PublishedTransitionRecord>,
    n: usize,
    timeout: Duration,
) -> Vec<PublishedTransitionRecord> {
    let mut rows = Vec::with_capacity(n);
    for i in 0..n {
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(r)) => rows.push(r),
            Ok(None) => panic!("transition channel closed (row {}/{})", i + 1, n),
            Err(_) => panic!("timeout waiting for row {}/{}", i + 1, n),
        }
    }
    rows
}

async fn assert_no_row(rx: &mut mpsc::Receiver<PublishedTransitionRecord>, window: Duration) {
    match tokio::time::timeout(window, rx.recv()).await {
        Ok(Some(r)) => panic!("unexpected early commit: {:?}", r.event),
        Ok(None) => panic!("transition channel closed"),
        Err(_) => {} // nothing arrived — correct
    }
}

fn inject_zone_ready(controller: &VehicleController, turn_id: u64, state: HeadlampState) {
    controller
        .get_actor_ref()
        .send_message(DigitalTwinCarVocabulary::ZoneReady {
            zone_id: ZoneId::Headlamp,
            turn_id,
            tell_attempt: 0,
            reply: zone_reply(state),
        })
        .expect("inject_zone_ready");
}

fn inject_timeout(controller: &VehicleController, turn_id: u64, attempt: u32) {
    controller
        .get_actor_ref()
        .send_message(DigitalTwinCarVocabulary::ZoneTellBackTimeout {
            zone_id: ZoneId::Headlamp,
            turn_id,
            tell_attempt: attempt,
        })
        .expect("inject_timeout");
}

/// Spawn a silent-headlamp controller with a fresh transition channel.
///
/// `initial_headlamp_ctx` is intentionally omitted: Phase 5 sets the headlamp
/// to `Ready` automatically when `boot_silent` injects the `BecomeOn` zone reply
/// for the startup barrier (turn 2).  Removing the override ensures that the actor
/// exercises the real `BecomeOn` path during boot.
async fn spawn_silent(
    identity: &str,
) -> (
    VehicleController,
    mpsc::Receiver<PublishedTransitionRecord>,
    ActorGuard<DigitalTwinCarVocabulary>,
) {
    let (tx, rx) = mpsc::channel(32);
    let opts = VehicleControllerRuntimeOptions {
        transition_tx: Some(tx),
        test_silent_headlamp: true, // suppress real headlamp replies; we inject manually
        ..Default::default()
    };
    let (controller, handle) =
        VehicleController::install_and_start_with_options(identity.to_string(), opts)
            .await
            .unwrap();
    let guard = ActorGuard {
        addr: controller.get_actor_ref().clone(),
        handle,
    };
    (controller, rx, guard)
}

/// Turn ID allocated for the headlamp startup barrier (first in MANAGED_ASSEMBLIES).
///   PowerOn → turn 1 (passthrough), headlamp startup → turn 2.
const STARTUP_BARRIER_TURN: u64 = 2;

/// Turn ID allocated for the wiper startup barrier (second in MANAGED_ASSEMBLIES).
///   Phase 7: wiper startup → turn 3.
const WIPER_STARTUP_BARRIER_TURN: u64 = 3;

/// First turn ID available to user-driven events after `boot_silent`.
///   Phase 7: PowerOn=1, headlamp startup=2, wiper startup=3, first user event=4.
const FIRST_USER_TURN: u64 = 4;

/// Boot sequence for a silent-headlamp actor (Phase 7: wiper is non-silent).
///
/// Sends `PowerOn` (turn 1 passthrough), manually injects the headlamp `BecomeOn` `ZoneReady`
/// reply (turn 2 — headlamp is silent), then waits for the wiper to auto-reply (turn 3 —
/// wiper is non-silent by default), and waits for the FSM to reach `Idle`.
/// Drains the three resulting ledger rows:
///   PowerOn + AssemblyZoneReady(Headlamp) + AssemblyZoneReady(Wiper).
async fn boot_silent(
    controller: &VehicleController,
    rx: &mut mpsc::Receiver<PublishedTransitionRecord>,
) {
    controller.send_power_on().await.expect("power on");
    // Give the actor a moment to process PowerOn and create the startup barriers.
    tokio::task::yield_now().await;
    inject_zone_ready(controller, STARTUP_BARRIER_TURN, HeadlampState::Ready);
    // Wiper (turn 3) is non-silent and auto-replies.
    let _ = WIPER_STARTUP_BARRIER_TURN; // documented for clarity
    crate::test::wait_fsm_state(controller, FsmState::Idle, std::time::Duration::from_millis(500)).await;
    // Phase 7: drain THREE ledger rows: PowerOn + AssemblyZoneReady(Headlamp) + AssemblyZoneReady(Wiper).
    drain_n(rx, 3, std::time::Duration::from_secs(3)).await;
}

// ── Test 1 ──────────────────────────────────────────────────────────────────

/// Two zone-directed events; rear-barrier reply arrives before front-barrier reply.
///
/// Phase 4 (GREEN): `ZoneReady(4)` is stored in `barrier(4)`.  Nothing drains until
/// `ZoneReady(3)` completes `barrier(3)`.  Both then drain in FIFO order; `rows[1]`
/// carries the real injected reply → `actions` is empty.
///
/// Phase 3 (RED): `ZoneReady(4)` is dropped (pending turn is turn 3).  After
/// `ZoneReady(3)` commits turn 3, `pump_fsm_backlog` starts lux2 with a fresh zone
/// tell to the silent headlamp; retries exhaust → synthetic reply → `rows[1].actions`
/// contains `LogWarning` → assertion fails.
#[tokio::test]
async fn two_zone_directed_events_commit_in_arrival_order() {
    let (controller, mut rx, _guard) = spawn_silent("ROB-ORDER-1").await;

    boot_silent(&controller, &mut rx).await;

    // Turn 3: zone-directed (headlamp zone tell needed).
    controller
        .submit_fsm_event(FsmEvent::UpdateAmbientLux(20))
        .await
        .unwrap();
    // Turn 4: also zone-directed.
    controller
        .submit_fsm_event(FsmEvent::UpdateAmbientLux(LUX_ON_THRESHOLD + 100))
        .await
        .unwrap();

    // Inject turn-4 reply FIRST — before the 50 ms retry timer fires (all injections at ~5 ms).
    // Phase 4 stores it; Phase 3 drops it (pending = turn 3).
    tokio::task::yield_now().await;
    inject_zone_ready(&controller, FIRST_USER_TURN + 1, HeadlampState::Ready);

    // Front barrier (turn 3) still pending — nothing must drain.
    assert_no_row(&mut rx, Duration::from_millis(30)).await;

    // Inject turn-3 reply → front completes → drain: turn 3 then turn 4.
    inject_zone_ready(&controller, FIRST_USER_TURN, HeadlampState::Ready);

    let rows = drain_n(&mut rx, 2, Duration::from_secs(3)).await;
    assert_eq!(rows.len(), 2, "both events must commit");
    assert!(
        rows[0].record_seq < rows[1].record_seq,
        "turn 3 must precede turn 4 in the ledger"
    );
    // RED assertion: Phase 4 → injected reply (no LogWarning); Phase 3 → synthetic (LogWarning).
    assert!(
        rows[1].actions.iter().all(|a| !matches!(a, PublishedDomainAction::LogWarning(_))),
        "rear barrier must commit with real reply (no LogWarning), got {:?}",
        rows[1].actions
    );
}

// ── Test 2 ──────────────────────────────────────────────────────────────────

/// Three events (zone, zone, non-zone); zone replies arrive out of order.
///
/// Phase 4 (GREEN): `barrier(5=UpdateRpm)` is immediately complete; after both zone
/// replies are stored and the front drains, all three commit in order with no synthetic.
///
/// Phase 3 (RED): `ZoneReady(4)` dropped; lux2 restarts via pump and exhausts via timer;
/// `rows[1]` is synthetic (LogWarning) → assertion fails.
#[tokio::test]
async fn three_events_drain_in_arrival_order_when_zone_replies_arrive_out_of_order() {
    use crate::vehicle_physics::RPM_DRIVING_THRESHOLD;

    let (controller, mut rx, _guard) = spawn_silent("ROB-ORDER-2").await;

    boot_silent(&controller, &mut rx).await;

    controller
        .submit_fsm_event(FsmEvent::UpdateAmbientLux(20))
        .await
        .unwrap(); // turn 3, zone
    controller
        .submit_fsm_event(FsmEvent::UpdateAmbientLux(LUX_ON_THRESHOLD + 100))
        .await
        .unwrap(); // turn 4, zone
    controller
        .submit_fsm_event(FsmEvent::UpdateRpm(RPM_DRIVING_THRESHOLD + 100))
        .await
        .unwrap(); // turn 5, no zone → immediately complete

    tokio::task::yield_now().await;

    inject_zone_ready(&controller, FIRST_USER_TURN + 1, HeadlampState::Ready); // turn 4
    assert_no_row(&mut rx, Duration::from_millis(30)).await;
    inject_zone_ready(&controller, FIRST_USER_TURN, HeadlampState::Ready); // turn 3

    let rows = drain_n(&mut rx, 3, Duration::from_secs(3)).await;
    assert_eq!(rows.len(), 3, "all three events must commit");
    assert!(rows[0].record_seq < rows[1].record_seq);
    assert!(rows[1].record_seq < rows[2].record_seq);
    // RED assertion: turn 4 (rows[1]) must use the real reply, not a synthetic.
    assert!(
        rows[1].actions.iter().all(|a| !matches!(a, PublishedDomainAction::LogWarning(_))),
        "turn-4 barrier must commit with real reply (no LogWarning), got {:?}",
        rows[1].actions
    );
}

// ── Test 3 ──────────────────────────────────────────────────────────────────

/// Manually exhaust front-barrier retries; rear barrier had a real reply stored
/// before exhaustion occurred.
///
/// Phase 4 (GREEN): `ZoneReady(4)` stored in `barrier(4)` before any timeout fires.
/// Injected timeouts exhaust `barrier(3)` → synthetic commit.  Drain loop immediately
/// finds `barrier(4)` complete → commits with stored real reply → `rows[1].actions` empty.
///
/// Phase 3 (RED): `ZoneReady(4)` dropped.  After turn 3 exhausts, `pump_fsm_backlog`
/// restarts lux2 with a fresh zone tell; headlamp is silent; lux2 exhausts via timer →
/// `rows[1]` is synthetic → `LogWarning` present → assertion fails.
#[tokio::test]
async fn exhausted_front_barrier_unblocks_rear_with_stored_reply() {
    use crate::twin_runtime::constants::ZONE_TELL_BACK_MAX_RETRIES;

    let (controller, mut rx, _guard) = spawn_silent("ROB-TIMEOUT-1").await;

    boot_silent(&controller, &mut rx).await;

    controller
        .submit_fsm_event(FsmEvent::UpdateAmbientLux(20))
        .await
        .unwrap(); // turn 3, zone
    controller
        .submit_fsm_event(FsmEvent::UpdateAmbientLux(LUX_ON_THRESHOLD + 100))
        .await
        .unwrap(); // turn 4, zone

    tokio::task::yield_now().await;

    // Store turn-4 reply BEFORE exhausting turn 3.
    // Phase 4: stored in barrier(4). Phase 3: dropped (pending = turn 3).
    inject_zone_ready(&controller, FIRST_USER_TURN + 1, HeadlampState::Ready);

    // Manually exhaust turn-3 retries (attempt 0 → retry → 1 → retry → 2 → gave up).
    for attempt in 0..=(ZONE_TELL_BACK_MAX_RETRIES as u32) {
        inject_timeout(&controller, FIRST_USER_TURN, attempt);
    }

    // Turn 3 committed (synthetic). Turn 4 committed (real in Phase 4; synthetic in Phase 3).
    let rows = drain_n(&mut rx, 2, Duration::from_secs(3)).await;
    assert_eq!(rows.len(), 2);
    assert!(rows[0].record_seq < rows[1].record_seq);
    // RED assertion: rows[1] must use real reply (no LogWarning).
    assert!(
        rows[1].actions.iter().all(|a| !matches!(a, PublishedDomainAction::LogWarning(_))),
        "rear barrier must use stored real reply (no LogWarning), got {:?}",
        rows[1].actions
    );
}

// ── Test 4 ──────────────────────────────────────────────────────────────────

/// Drain stops when the front barrier is incomplete; only advances when front is resolved.
///
/// Phase 4 (GREEN): `barrier(4=UpdateRpm)` is immediately complete but blocked by
/// incomplete `barrier(3=lux)`.  `assert_no_row` verifies nothing drains prematurely.
/// `ZoneReady(3)` completes the front → both drain.  `rows[1]` (UpdateRpm) has no zone
/// reply → `actions` empty regardless of Phase; the key RED property here is that
/// `rows[0]` (lux) must also carry no LogWarning (Phase 4 uses real reply, Phase 3 uses
/// the injected `ZoneReady(3)` directly and commits it — this part passes in both).
///
/// To make the test RED: also send a second zone event (turn 4 = lux2) so that the
/// injected `ZoneReady(4)` before the front resolves is dropped in Phase 3.
#[tokio::test]
async fn second_zone_reply_before_first_does_not_drain_anything_prematurely() {
    let (controller, mut rx, _guard) = spawn_silent("ROB-DRAIN-1").await;

    boot_silent(&controller, &mut rx).await;

    controller
        .submit_fsm_event(FsmEvent::UpdateAmbientLux(20))
        .await
        .unwrap(); // turn 3, zone
    controller
        .submit_fsm_event(FsmEvent::UpdateAmbientLux(LUX_ON_THRESHOLD + 100))
        .await
        .unwrap(); // turn 4, zone

    tokio::task::yield_now().await;

    // Inject turn-4 reply first — Phase 3 drops it; Phase 4 stores it.
    inject_zone_ready(&controller, FIRST_USER_TURN + 1, HeadlampState::Ready);

    // Front (turn 3) still incomplete → nothing must drain yet.
    assert_no_row(&mut rx, Duration::from_millis(30)).await;

    // Complete the front → drain: turn 3 then turn 4.
    inject_zone_ready(&controller, FIRST_USER_TURN, HeadlampState::Ready);

    let rows = drain_n(&mut rx, 2, Duration::from_secs(3)).await;
    assert_eq!(rows.len(), 2);
    assert!(rows[0].record_seq < rows[1].record_seq);
    // RED assertion: real reply stored for turn 4 → no LogWarning in rows[1].
    assert!(
        rows[1].actions.iter().all(|a| !matches!(a, PublishedDomainAction::LogWarning(_))),
        "turn-4 must commit with stored real reply (no LogWarning), got {:?}",
        rows[1].actions
    );
}
