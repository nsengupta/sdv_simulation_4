//! Contract tests for async VehicleController facade APIs.

use crate::digital_twin::DigitalTwinCarVocabulary;
use crate::twin_runtime::controller::virtual_car_actor::VirtualCarActor;
use crate::fsm::{FsmEvent, FsmState};
use crate::test::ActorGuard;
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

    controller
        .submit_fsm_event(FsmEvent::PowerOn)
        .await
        .expect("power on should enqueue");
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

    // Each applied FSM event advances Counter A by exactly one, sink or not.
    controller
        .submit_fsm_event(FsmEvent::PowerOn)
        .await
        .expect("power on should enqueue");
    let after_one = controller
        .get_snapshot(Some(Duration::from_millis(250)))
        .await
        .expect("snapshot");
    assert_eq!(after_one.as_of_seq(), 1);

    controller
        .submit_physical_car_event(PhysicalCarVocabulary::TelemetryUpdate(
            crate::VssSignal::EngineRpm(1500),
        ))
        .await
        .expect("telemetry should enqueue");
    let after_two = controller
        .get_snapshot(Some(Duration::from_millis(250)))
        .await
        .expect("snapshot");
    assert_eq!(after_two.as_of_seq(), 2);
    // A pure query does not advance the ledger.
    let again = controller
        .get_snapshot(Some(Duration::from_millis(250)))
        .await
        .expect("snapshot");
    assert_eq!(again.as_of_seq(), 2);
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

    controller.send_power_on().await.expect("send_power_on");
    controller.send_power_off().await.expect("send_power_off");
    crate::test::wait_fsm_state(&controller, FsmState::Off, Duration::from_millis(250)).await;

    let snapshot = controller
        .get_snapshot(Some(Duration::from_millis(250)))
        .await
        .expect("snapshot");
    assert_eq!(*snapshot.current_state(), FsmState::Off);
}
