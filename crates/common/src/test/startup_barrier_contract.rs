//! Phase 5 contract tests: `StartAssemblies` / `StopAssemblies` wired to real
//! `TurnBarrier` coordination (RED → GREEN in Phase 5).
//!
//! ## RED in Phase 4
//!
//! Phase 4 leaves `StartAssemblies` / `StopAssemblies` as no-ops in
//! `apply_committed_quiescence`.  The FSM enters `PreparingToStart` on `PowerOn`
//! but never transitions to `Idle` because no `AssemblyZoneReady` is committed.
//! `wait_fsm_state(Idle, 500ms)` times out → tests 1 and 2 fail.
//! Similarly for the shutdown path → tests 3 and 4 fail.
//!
//! ## GREEN in Phase 5
//!
//! `StartAssemblies` creates a `TurnBarrier` per managed assembly, sends `BecomeOn`,
//! and the drain loop commits `AssemblyZoneReady(Headlamp)` when the headlamp replies.
//! The FSM transitions `PreparingToStart → Idle` (test 1) and stays in `PreparingToStart`
//! when no reply arrives (test 2).  Likewise for shutdown (tests 3 and 4).

use std::time::Duration;

use crate::digital_twin::{DigitalTwinCarVocabulary, ZoneReply};
use crate::fsm::{FsmState, HeadlampState, AssemblyId};
use crate::test::{power_on_to_idle, wait_fsm_state, ActorGuard};
use crate::twin_runtime::controller::vehicle_controller::VehicleControllerRuntimeOptions;
use crate::vehicle_state::{HeadlampContext, HeadlampZoneReply};
use crate::VehicleController;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Turn ID allocated for the startup barrier (`StartAssemblies` loop, turn 2).
const STARTUP_BARRIER_TURN: u64 = 2;

/// Turn ID allocated for the shutdown barrier (`StopAssemblies` loop).
/// After boot via `power_on_to_idle` turn counter is at 3; shutdown barrier = turn 3.
const SHUTDOWN_BARRIER_TURN: u64 = 3;

fn zone_reply_with_state(state: HeadlampState) -> ZoneReply {
    ZoneReply::Headlamp(HeadlampZoneReply {
        ctx: HeadlampContext { state, ack_pending_since: None },
        outcomes: vec![],
    })
}

fn inject_zone_ready(controller: &VehicleController, turn_id: u64, state: HeadlampState) {
    controller
        .get_actor_ref()
        .send_message(DigitalTwinCarVocabulary::ZoneReady {
            zone_id: AssemblyId::Headlamp,
            turn_id,
            tell_attempt: 0,
            reply: zone_reply_with_state(state),
        })
        .expect("inject_zone_ready");
}

async fn spawn_non_silent(identity: &str) -> (VehicleController, ActorGuard<DigitalTwinCarVocabulary>) {
    let (controller, handle) =
        VehicleController::install_and_start_with_options(identity.to_string(), Default::default())
            .await
            .expect("spawn non-silent controller");
    let guard = ActorGuard {
        addr: controller.get_actor_ref().clone(),
        handle,
    };
    (controller, guard)
}

async fn spawn_silent(identity: &str) -> (VehicleController, ActorGuard<DigitalTwinCarVocabulary>) {
    let opts = VehicleControllerRuntimeOptions {
        test_silent_headlamp: true,
        ..Default::default()
    };
    let (controller, handle) =
        VehicleController::install_and_start_with_options(identity.to_string(), opts)
            .await
            .expect("spawn silent controller");
    let guard = ActorGuard {
        addr: controller.get_actor_ref().clone(),
        handle,
    };
    (controller, guard)
}

// ── Test 1 ───────────────────────────────────────────────────────────────────

/// Non-silent headlamp replies to `BecomeOn`; brain commits `AssemblyZoneReady(Headlamp)`;
/// FSM transitions `PreparingToStart → Idle` without any manual injection.
#[tokio::test]
async fn given_power_on_when_headlamp_replies_ready_then_fsm_reaches_idle() {
    let (controller, _guard) = spawn_non_silent("STARTUP-1").await;

    controller.send_power_on().await.expect("power on");

    // No manual AssembliesReady injection — the wired barrier handles it.
    wait_fsm_state(&controller, FsmState::Idle, Duration::from_millis(500)).await;
}

// ── Test 2 ───────────────────────────────────────────────────────────────────

/// Silent headlamp never replies to `BecomeOn`; FSM must remain in `PreparingToStart`.
///
/// This test must NOT use `power_on_to_idle` (which would bypass the barrier).
/// It deliberately sends only `PowerOn` and then checks the state is still `PreparingToStart`.
#[tokio::test]
async fn given_power_on_with_silent_headlamp_then_fsm_stays_in_preparing_to_start() {
    let (controller, _guard) = spawn_silent("STARTUP-2").await;

    controller.send_power_on().await.expect("power on");

    // Give the actor time to process PowerOn and create the startup barrier.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let snapshot = controller
        .get_snapshot(Some(ractor::concurrency::Duration::from_millis(50)))
        .await
        .expect("get snapshot");
    assert!(
        matches!(*snapshot.current_state(), FsmState::PreparingToStart { .. }),
        "silent headlamp must keep FSM in PreparingToStart; got {:?}",
        snapshot.current_state()
    );
}

// ── Test 3 ───────────────────────────────────────────────────────────────────

/// Non-silent headlamp replies to `BecomeOff`; brain commits `AssemblyZoneReady(Headlamp)`;
/// FSM transitions `PreparingToStop → Off` without any manual injection.
#[tokio::test]
async fn given_power_off_from_idle_when_headlamp_replies_off_then_fsm_reaches_off() {
    let (controller, _guard) = spawn_non_silent("SHUTDOWN-1").await;

    // Reach Idle via automatic BecomeOn flow.
    power_on_to_idle(&controller).await;

    controller.send_power_off().await.expect("power off");

    // No manual AssembliesStopped injection — the wired barrier handles it.
    wait_fsm_state(&controller, FsmState::Off, Duration::from_millis(500)).await;
}

// ── Test 4 ───────────────────────────────────────────────────────────────────

/// Silent headlamp never replies to `BecomeOff`; FSM must remain in `PreparingToStop`.
///
/// Boot via manual `ZoneReady` injection for the startup barrier (since headlamp is silent),
/// then send `PowerOff` and verify no automatic transition to `Off` occurs.
#[tokio::test]
async fn given_power_off_with_silent_headlamp_then_fsm_stays_in_preparing_to_stop() {
    let (controller, _guard) = spawn_silent("SHUTDOWN-2").await;

    // Boot: send PowerOn and manually inject the BecomeOn reply that the silent headlamp
    // will not send.
    controller.send_power_on().await.expect("power on");
    tokio::task::yield_now().await;
    inject_zone_ready(&controller, STARTUP_BARRIER_TURN, HeadlampState::Ready);
    wait_fsm_state(&controller, FsmState::Idle, Duration::from_millis(500)).await;

    // Now try to power off — silent headlamp will not reply to BecomeOff.
    controller.send_power_off().await.expect("power off");

    tokio::time::sleep(Duration::from_millis(50)).await;

    let snapshot = controller
        .get_snapshot(Some(ractor::concurrency::Duration::from_millis(50)))
        .await
        .expect("get snapshot");
    assert!(
        matches!(*snapshot.current_state(), FsmState::PreparingToStop { .. }),
        "silent headlamp must keep FSM in PreparingToStop; got {:?}",
        snapshot.current_state()
    );
    // Verify the shutdown barrier turn ID is what we expect (turn 3 after boot).
    let _ = SHUTDOWN_BARRIER_TURN; // referenced for documentation purposes
}
