//! Phase 7 contract tests: Wiper as second managed assembly.
//!
//! Tests 1–7 are pure unit tests (L1 state machine, `zone_message_for_event`).
//! Tests 8–10 are actor-level integration tests requiring the full wiper wiring.
//!
//! ## RED state
//!
//! All tests fail to compile until `ZoneId::Wiper`, `WiperMessage`, `WiperContext`,
//! `WiperState`, `FsmEvent::RainsStarted/RainsStopped`, and `ZoneMessage::Wiper` exist.
//! Actor tests additionally require `test_silent_wiper` in `VehicleControllerRuntimeOptions`
//! and the `WiperActor` wired into `VirtualCarRuntimeState`.

use std::time::Duration;

use crate::digital_twin::{DigitalTwinCarVocabulary, ZoneMessage, ZoneReply};
use crate::fsm::{FsmEvent, FsmState, ZoneId};
use crate::test::ActorGuard;
use crate::twin_runtime::controller::vehicle_controller::VehicleControllerRuntimeOptions;
use crate::twin_runtime::zone_turn::zone_message_for_event;
use crate::vehicle_state::{WiperContext, WiperMessage, WiperState, WiperZoneReply};
use crate::VehicleController;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Turn IDs after Phase 7 boot (2 assembly barriers: headlamp + wiper).
///
/// PowerOn = turn 1 (passthrough)
/// Headlamp startup barrier = turn 2
/// Wiper startup barrier = turn 3
/// First user event = turn 4
const HEADLAMP_STARTUP_TURN: u64 = 2;
const WIPER_STARTUP_TURN: u64 = 3;
const FIRST_USER_TURN: u64 = 4;

/// Poll until the wiper assembly reaches `expected` state.
pub async fn wait_wiper_state(
    controller: &VehicleController,
    expected: WiperState,
    timeout: Duration,
) {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Ok(snapshot) = controller
            .get_snapshot(Some(ractor::concurrency::Duration::from_millis(50)))
            .await
        {
            if snapshot.context().wiper.state == expected {
                return;
            }
        }
        if std::time::Instant::now() >= deadline {
            let last = controller
                .get_snapshot(Some(ractor::concurrency::Duration::from_millis(50)))
                .await
                .ok();
            panic!(
                "timed out after {timeout:?} waiting for wiper {expected:?}, last={:?}",
                last.as_ref().map(|s| s.context().wiper.state)
            );
        }
        tokio::task::yield_now().await;
    }
}

fn inject_wiper_zone_ready(controller: &VehicleController, turn_id: u64) {
    controller
        .get_actor_ref()
        .send_message(DigitalTwinCarVocabulary::ZoneReady {
            zone_id: ZoneId::Wiper,
            turn_id,
            tell_attempt: 0,
            reply: ZoneReply::Wiper(WiperZoneReply {
                ctx: WiperContext { state: WiperState::Ready },
                outcomes: vec![],
            }),
        })
        .expect("inject_wiper_zone_ready");
}

fn inject_headlamp_zone_ready_startup(controller: &VehicleController) {
    use crate::vehicle_state::{HeadlampContext, HeadlampState, HeadlampZoneReply};
    controller
        .get_actor_ref()
        .send_message(DigitalTwinCarVocabulary::ZoneReady {
            zone_id: ZoneId::Headlamp,
            turn_id: HEADLAMP_STARTUP_TURN,
            tell_attempt: 0,
            reply: ZoneReply::Headlamp(HeadlampZoneReply {
                ctx: HeadlampContext { state: HeadlampState::Ready, ack_pending_since: None },
                outcomes: vec![],
            }),
        })
        .expect("inject_headlamp_zone_ready_startup");
}

async fn spawn_non_silent(
    identity: &str,
) -> (VehicleController, ActorGuard<DigitalTwinCarVocabulary>) {
    let (controller, handle) =
        VehicleController::install_and_start_with_options(identity.to_string(), Default::default())
            .await
            .expect("spawn non-silent");
    let guard = ActorGuard { addr: controller.get_actor_ref().clone(), handle };
    (controller, guard)
}

async fn spawn_silent_wiper(
    identity: &str,
) -> (
    VehicleController,
    tokio::sync::mpsc::Receiver<crate::published::PublishedTransitionRecord>,
    ActorGuard<DigitalTwinCarVocabulary>,
) {
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    let opts = VehicleControllerRuntimeOptions {
        transition_tx: Some(tx),
        test_silent_wiper: true,
        ..Default::default()
    };
    let (controller, handle) =
        VehicleController::install_and_start_with_options(identity.to_string(), opts)
            .await
            .expect("spawn silent-wiper");
    let guard = ActorGuard { addr: controller.get_actor_ref().clone(), handle };
    (controller, rx, guard)
}

async fn spawn_silent_both(
    identity: &str,
) -> (
    VehicleController,
    tokio::sync::mpsc::Receiver<crate::published::PublishedTransitionRecord>,
    ActorGuard<DigitalTwinCarVocabulary>,
) {
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    let opts = VehicleControllerRuntimeOptions {
        transition_tx: Some(tx),
        test_silent_headlamp: true,
        test_silent_wiper: true,
        ..Default::default()
    };
    let (controller, handle) =
        VehicleController::install_and_start_with_options(identity.to_string(), opts)
            .await
            .expect("spawn silent-both");
    let guard = ActorGuard { addr: controller.get_actor_ref().clone(), handle };
    (controller, rx, guard)
}

async fn drain_n(
    rx: &mut tokio::sync::mpsc::Receiver<crate::published::PublishedTransitionRecord>,
    n: usize,
    timeout: Duration,
) -> Vec<crate::published::PublishedTransitionRecord> {
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

async fn assert_no_row(
    rx: &mut tokio::sync::mpsc::Receiver<crate::published::PublishedTransitionRecord>,
    window: Duration,
) {
    match tokio::time::timeout(window, rx.recv()).await {
        Ok(Some(r)) => panic!("unexpected early commit: {:?}", r.event),
        Ok(None) => panic!("transition channel closed"),
        Err(_) => {} // nothing arrived — correct
    }
}

/// Boot a silent-both controller to Idle by manually injecting both assembly ZoneReady replies.
/// Drains the 3 resulting ledger rows: PowerOn + AssemblyZoneReady(Headlamp) + AssemblyZoneReady(Wiper).
async fn boot_silent_both(
    controller: &VehicleController,
    rx: &mut tokio::sync::mpsc::Receiver<crate::published::PublishedTransitionRecord>,
) {
    controller.send_power_on().await.expect("power on");
    tokio::task::yield_now().await;
    inject_headlamp_zone_ready_startup(controller);
    inject_wiper_zone_ready(controller, WIPER_STARTUP_TURN);
    crate::test::wait_fsm_state(controller, FsmState::Idle, Duration::from_millis(500)).await;
    drain_n(rx, 3, Duration::from_secs(3)).await;
}

// ── Test 1: ZoneId::Wiper is distinct ─────────────────────────────────────────

#[test]
fn test_wiper_zone_id_exists_and_is_distinct_from_headlamp() {
    let headlamp = ZoneId::Headlamp;
    let wiper = ZoneId::Wiper;
    assert_ne!(headlamp, wiper);
    assert_eq!(format!("{headlamp:?}"), "Headlamp");
    assert_eq!(format!("{wiper:?}"), "Wiper");
}

// ── Test 2: RainsStarted routes to wiper zone ─────────────────────────────────

#[test]
fn test_rains_started_routes_to_wiper_zone() {
    let result = zone_message_for_event(&FsmEvent::RainsStarted, &FsmState::Driving);
    match result {
        Some((zone_id, ZoneMessage::Wiper(WiperMessage::Start))) => {
            assert_eq!(zone_id, ZoneId::Wiper);
        }
        other => panic!("expected Some((Wiper, Wiper(Start))), got {other:?}"),
    }
}

// ── Test 3: RainsStarted suppressed during PreparingToStart ──────────────────

#[test]
fn test_rains_started_suppressed_during_preparing_to_start() {
    let result = zone_message_for_event(&FsmEvent::RainsStarted, &FsmState::PreparingToStart);
    assert!(result.is_none(), "expected None during PreparingToStart, got {result:?}");
}

// ── Test 4: BecomeOn → Ready ──────────────────────────────────────────────────

#[test]
fn test_wiper_become_on_transitions_to_ready() {
    let ctx = WiperContext::default();
    assert_eq!(ctx.state, WiperState::Off);
    let reply = ctx.on_receiving_message(WiperMessage::BecomeOn);
    assert_eq!(reply.ctx.state, WiperState::Ready);
    assert!(reply.outcomes.is_empty());
}

// ── Test 5: Start while Ready → Running ──────────────────────────────────────

#[test]
fn test_wiper_start_while_ready_transitions_to_running() {
    let mut ctx = WiperContext::default();
    ctx.state = WiperState::Ready;
    let reply = ctx.on_receiving_message(WiperMessage::Start);
    assert_eq!(reply.ctx.state, WiperState::Running);
    use crate::vehicle_state::WiperOutcome;
    assert_eq!(reply.outcomes, vec![WiperOutcome::StartWiping]);
}

// ── Test 6: Stop while Running → Ready ───────────────────────────────────────

#[test]
fn test_wiper_stop_while_running_transitions_to_ready() {
    let mut ctx = WiperContext::default();
    ctx.state = WiperState::Running;
    let reply = ctx.on_receiving_message(WiperMessage::Stop);
    assert_eq!(reply.ctx.state, WiperState::Ready);
    use crate::vehicle_state::WiperOutcome;
    assert_eq!(reply.outcomes, vec![WiperOutcome::StopWiping]);
}

// ── Test 7: BecomeOff from Running → Off directly ────────────────────────────

#[test]
fn test_wiper_become_off_from_running_transitions_to_off_directly() {
    let mut ctx = WiperContext::default();
    ctx.state = WiperState::Running;
    let reply = ctx.on_receiving_message(WiperMessage::BecomeOff);
    assert_eq!(
        reply.ctx.state,
        WiperState::Off,
        "BecomeOff must go directly to Off from Running (no intermediate state)"
    );
    assert!(reply.outcomes.is_empty());
}

// ── Test 8: Both assemblies included in startup barrier ───────────────────────

/// Non-silent spawn; after `PowerOn` both twinlets must reach their `Ready` state.
/// This verifies that `BecomeOn` is sent to both assemblies and both barriers drain.
#[tokio::test]
async fn test_wiper_included_in_startup_barrier() {
    let (controller, _guard) = spawn_non_silent("WIPER-STARTUP-1").await;

    controller.send_power_on().await.expect("power on");

    // Both assembly barriers must drain: headlamp → Ready, wiper → Ready.
    crate::test::wait_headlamp_state(
        &controller,
        crate::fsm::HeadlampState::Ready,
        Duration::from_millis(500),
    )
    .await;
    wait_wiper_state(&controller, WiperState::Ready, Duration::from_millis(500)).await;

    let snapshot = controller
        .get_snapshot(Some(ractor::concurrency::Duration::from_millis(50)))
        .await
        .expect("get snapshot");
    assert_eq!(*snapshot.current_state(), FsmState::Idle);
}

// ── Test 9: Concurrent cross-zone events commit in arrival order ──────────────

/// Both zones silent; manually inject wiper reply before headlamp reply (out of order).
/// HOB invariant: headlamp turn (front) must drain before wiper turn (rear).
#[tokio::test]
async fn test_concurrent_headlamp_and_wiper_events_commit_in_arrival_order() {
    let (controller, mut rx, _guard) = spawn_silent_both("WIPER-ORDER-1").await;
    boot_silent_both(&controller, &mut rx).await;

    // Turn 4: zone-directed to headlamp (UpdateAmbientLux).
    controller
        .submit_fsm_event(FsmEvent::UpdateAmbientLux(20))
        .await
        .expect("lux event");
    // Turn 5: zone-directed to wiper (RainsStarted).
    controller
        .submit_fsm_event(FsmEvent::RainsStarted)
        .await
        .expect("rains started");

    tokio::task::yield_now().await;

    // Inject wiper reply (turn 5) BEFORE headlamp reply (turn 4) — out of order.
    inject_wiper_zone_ready(&controller, FIRST_USER_TURN + 1);

    // Nothing must drain: headlamp (front, turn 4) still pending.
    assert_no_row(&mut rx, Duration::from_millis(30)).await;

    // Complete headlamp (turn 4) → drain: turn 4 then turn 5.
    use crate::vehicle_state::{HeadlampContext, HeadlampState, HeadlampZoneReply};
    controller
        .get_actor_ref()
        .send_message(DigitalTwinCarVocabulary::ZoneReady {
            zone_id: ZoneId::Headlamp,
            turn_id: FIRST_USER_TURN,
            tell_attempt: 0,
            reply: ZoneReply::Headlamp(HeadlampZoneReply {
                ctx: HeadlampContext { state: HeadlampState::Ready, ack_pending_since: None },
                outcomes: vec![],
            }),
        })
        .expect("inject headlamp zone ready");

    let rows = drain_n(&mut rx, 2, Duration::from_secs(3)).await;
    assert_eq!(rows.len(), 2, "both events must commit");
    assert!(
        rows[0].record_seq < rows[1].record_seq,
        "headlamp event (turn 4) must precede wiper event (turn 5) in ledger"
    );
}

// ── Test 10: Slow wiper does not delay headlamp event commit ─────────────────

/// Wiper is silent; headlamp auto-replies. Submit lux (headlamp zone) then rains (wiper zone).
/// Headlamp lux must commit immediately after the headlamp barrier drains, without waiting
/// for the wiper barrier to complete.
#[tokio::test]
async fn test_slow_wiper_does_not_delay_headlamp_event_commit() {
    let (controller, mut rx, _guard) = spawn_silent_wiper("WIPER-SLOW-1").await;

    // Boot: headlamp is non-silent (auto-replies); wiper is silent (needs manual inject).
    // We inject wiper startup reply at turn 3 to reach Idle.
    controller.send_power_on().await.expect("power on");
    tokio::task::yield_now().await;
    inject_wiper_zone_ready(&controller, WIPER_STARTUP_TURN);
    crate::test::wait_fsm_state(&controller, FsmState::Idle, Duration::from_millis(500)).await;
    // Drain 3 boot rows: PowerOn + AssemblyZoneReady(Headlamp) + AssemblyZoneReady(Wiper).
    drain_n(&mut rx, 3, Duration::from_secs(3)).await;

    // Turn 4: lux → headlamp zone (non-silent headlamp auto-replies).
    controller
        .submit_fsm_event(FsmEvent::UpdateAmbientLux(20))
        .await
        .expect("lux event");

    // Turn 5: rains → wiper zone (silent wiper, will not reply on its own).
    controller
        .submit_fsm_event(FsmEvent::RainsStarted)
        .await
        .expect("rains started");

    // Headlamp auto-replies for turn 4.
    // Turn 4 (headlamp lux) must commit before turn 5 (wiper rains) even though wiper is slow.
    let rows = drain_n(&mut rx, 1, Duration::from_millis(500)).await;
    assert_eq!(rows.len(), 1, "headlamp lux event must commit before wiper turn resolves");

    // Wiper turn 5 is still pending — inject manually to clean up.
    inject_wiper_zone_ready(&controller, FIRST_USER_TURN + 1);
    drain_n(&mut rx, 1, Duration::from_secs(3)).await;
}
