//! Contract tests for async VehicleController facade APIs.

use crate::digital_twin::DigitalTwinCarVocabulary;
use crate::twin_runtime::controller::virtual_car_actor::VirtualCarActor;
use crate::fsm::{FsmEvent, FsmState, Operational};
use crate::test::{power_off_to_off, power_on_to_idle, ActorGuard};
use crate::{PhysicalCarVocabulary, VehicleController};
use ractor::Actor;
use std::time::Duration;

#[tokio::test]
async fn given_physical_car_event_when_submitted_then_controller_drives_actor_state() {
    let (actor, handle) = Actor::spawn(None, VirtualCarActor::default(), "CTRL-API-01".into())
        .await
        .expect("spawn actor");
    let _guard = ActorGuard {
        addr: actor.clone(),
        handle,
    };
    let controller = VehicleController::new(actor.clone());

    // Phase 1: bridge PreparingToStart → Idle before driving.
    power_on_to_idle(&controller).await;
    crate::test::submit_daylight_ambient(&controller).await;
    controller
        .submit_physical_car_event(PhysicalCarVocabulary::TelemetryUpdate(
            crate::VssSignal::EngineRpm(1500),
        ))
        .await
        .expect("physical event should enqueue");

    let snapshot = controller
        .get_snapshot(Some(Duration::from_millis(250)))
        .await
        .expect("snapshot should be returned");
    assert_eq!(*snapshot.current_state(), FsmState::Driving);
}

#[tokio::test]
async fn given_controller_when_get_snapshot_called_then_returns_readonly_snapshot() {
    let (actor, handle) = Actor::spawn(None, VirtualCarActor::default(), "CTRL-API-02".into())
        .await
        .expect("spawn actor");
    let _guard = ActorGuard {
        addr: actor.clone(),
        handle,
    };
    let controller = VehicleController::new(actor.clone());

    controller
        .submit_fsm_event(FsmEvent::PowerOn)
        .await
        .expect("power on should enqueue");

    let direct = actor
        .call(
            |port| DigitalTwinCarVocabulary::GetStatus(port),
            Some(ractor::concurrency::Duration::from_millis(250)),
        )
        .await
        .expect("direct call should enqueue")
        .expect("direct call should reply");
    let via_api = controller
        .get_snapshot(Some(Duration::from_millis(250)))
        .await
        .expect("controller snapshot should reply");

    assert_eq!(direct.current_state(), via_api.current_state());
    assert_eq!(direct.context(), via_api.context());
}

#[tokio::test]
async fn given_applied_events_when_get_snapshot_then_as_of_seq_counts_every_event() {
    let (actor, handle) = Actor::spawn(None, VirtualCarActor::default(), "CTRL-API-04".into())
        .await
        .expect("spawn actor");
    let _guard = ActorGuard {
        addr: actor.clone(),
        handle,
    };
    let controller = VehicleController::new(actor);

    // Freshly-born twin: no event applied yet → as-of sequence is 0.
    let fresh = controller
        .get_snapshot(Some(Duration::from_millis(250)))
        .await
        .expect("snapshot");
    assert_eq!(fresh.as_of_seq(), 0);

    // Phase 1: PowerOn → PreparingToStart (seq 1).
    controller
        .submit_fsm_event(FsmEvent::PowerOn)
        .await
        .expect("power on should enqueue");
    let after_power_on = controller
        .get_snapshot(Some(Duration::from_millis(250)))
        .await
        .expect("snapshot");
    assert_eq!(after_power_on.as_of_seq(), 1, "PowerOn → PreparingToStart is seq 1");

    // Phase 1: AssembliesReady → Idle (seq 2).
    controller
        .submit_fsm_event(FsmEvent::Internal(Operational::AssembliesReady))
        .await
        .expect("assemblies ready should enqueue");
    let after_idle = controller
        .get_snapshot(Some(Duration::from_millis(250)))
        .await
        .expect("snapshot");
    assert_eq!(after_idle.as_of_seq(), 2, "AssembliesReady → Idle is seq 2");

    // RPM in dark (default lux=0): zone hop (seq 3) + LightingUnsafe internal hop (seq 4).
    controller
        .submit_physical_car_event(PhysicalCarVocabulary::TelemetryUpdate(
            crate::VssSignal::EngineRpm(1500),
        ))
        .await
        .expect("telemetry should enqueue");
    let after_rpm = controller
        .get_snapshot(Some(Duration::from_millis(250)))
        .await
        .expect("snapshot");
    assert_eq!(
        after_rpm.as_of_seq(),
        4,
        "dark driving entry emits zone hop (seq 3) + internal LightingUnsafe hop (seq 4)"
    );
    // A pure query does not advance the ledger.
    let again = controller
        .get_snapshot(Some(Duration::from_millis(250)))
        .await
        .expect("snapshot");
    assert_eq!(again.as_of_seq(), 4);
}

#[tokio::test]
async fn given_power_on_then_power_off_facade_when_idle_then_state_is_off() {
    let (actor, handle) = Actor::spawn(None, VirtualCarActor::default(), "CTRL-API-03".into())
        .await
        .expect("spawn actor");
    let _guard = ActorGuard {
        addr: actor.clone(),
        handle,
    };
    let controller = VehicleController::new(actor);

    // Phase 1: bridge through PreparingToStart → Idle → PreparingToStop → Off.
    power_on_to_idle(&controller).await;
    power_off_to_off(&controller).await;

    let snapshot = controller
        .get_snapshot(Some(Duration::from_millis(250)))
        .await
        .expect("snapshot");
    assert_eq!(*snapshot.current_state(), FsmState::Off);
}
