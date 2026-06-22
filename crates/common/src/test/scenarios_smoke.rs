//! Black-box scenario smoke tests: actor lifecycle + state journeys.

use crate::digital_twin::DigitalTwinCarVocabulary;
use crate::twin_runtime::controller::virtual_car_actor::VirtualCarActor;
use crate::fsm::{FsmEvent, FsmState};
use crate::test::{power_on_to_idle, ActorGuard};
use ractor::concurrency::Duration;
use ractor::Actor;

/// Default timeout for [`get_snapshot`] and actor `call` in scenario tests.
const DEFAULT_ACTOR_TIMEOUT: Duration = Duration::from_millis(250);

/// Snapshot retrieval with a caller-defined timeout.
async fn get_snapshot(
    actor: &ractor::ActorRef<DigitalTwinCarVocabulary>,
    timeout: Duration,
) -> crate::CarSnapshot {
    use ractor::rpc::CallResult;

    match actor
        .call(
            |port| DigitalTwinCarVocabulary::GetStatus(port),
            Some(timeout),
        )
        .await
    {
        Ok(CallResult::Success(snapshot)) => snapshot,
        Ok(CallResult::SenderError) => panic!("Actor dropped the reply port without responding."),
        Ok(CallResult::Timeout) => panic!(
            "Scenario Timeout: Actor failed to respond within {:?}.",
            timeout,
        ),
        Err(e) => panic!(
            "Scenario Timeout: Actor failed to respond within {:?}. Error: {}",
            timeout, e
        ),
    }
}

async fn wait_for_ambient_lux(
    actor: &ractor::ActorRef<DigitalTwinCarVocabulary>,
    expected: u16,
    timeout: std::time::Duration,
) -> crate::CarSnapshot {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let snapshot = get_snapshot(actor, DEFAULT_ACTOR_TIMEOUT).await;
        if snapshot.context().visibility.ambient_lux == expected {
            return snapshot;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "timed out waiting for ambient_lux {expected}, last={}",
                snapshot.context().visibility.ambient_lux
            );
        }
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn scenario_cold_start_get_status_shows_off() {
    let (actor, handle) = Actor::spawn(None, VirtualCarActor::default(), "QUICK".into())
        .await
        .unwrap();
    let _guard = ActorGuard {
        addr: actor.clone(),
        handle,
    };

    let car = get_snapshot(&actor, DEFAULT_ACTOR_TIMEOUT).await;
    assert_eq!(*car.current_state(), FsmState::Off);
}

#[tokio::test]
async fn scenario_power_on_then_drive_rpm_enters_driving() {
    let (actor, handle) = Actor::spawn(None, VirtualCarActor::default(), "WARMUP".into())
        .await
        .unwrap();
    let _guard = ActorGuard {
        addr: actor.clone(),
        handle,
    };

    // Phase 5: startup barrier drains automatically.
    let controller = crate::VehicleController::new(actor.clone());
    power_on_to_idle(&controller).await;

    actor
        .send_message(DigitalTwinCarVocabulary::from(FsmEvent::UpdateAmbientLux(
            crate::vehicle_physics::LUX_ON_THRESHOLD + 100,
        )))
        .unwrap();

    wait_for_ambient_lux(
        &actor,
        crate::vehicle_physics::LUX_ON_THRESHOLD + 100,
        std::time::Duration::from_millis(250),
    )
    .await;

    actor
        .send_message(DigitalTwinCarVocabulary::from(FsmEvent::UpdateRpm(1200)))
        .unwrap();

    let car = get_snapshot(&actor, DEFAULT_ACTOR_TIMEOUT).await;
    assert_eq!(*car.current_state(), FsmState::Driving);
    car.verify_all_invariants().expect("Safety breach on warmup");
}

#[tokio::test]
async fn scenario_rpm_input_ignored_when_ignition_off() {
    let (actor, handle) = Actor::spawn(None, VirtualCarActor::default(), "INVALID".into())
        .await
        .unwrap();
    let _guard = ActorGuard {
        addr: actor.clone(),
        handle,
    };

    actor
        .send_message(DigitalTwinCarVocabulary::from(FsmEvent::UpdateRpm(3000)))
        .unwrap();
    tokio::task::yield_now().await;

    let car = get_snapshot(&actor, DEFAULT_ACTOR_TIMEOUT).await;
    assert_eq!(*car.current_state(), FsmState::Off);
    assert_eq!(car.context().powertrain.wheel_rpm.front_left, 3000);
    car.verify_all_invariants()
        .expect("Safety breach on invalid input");
}

#[tokio::test]
async fn scenario_redline_rpm_from_driving_enters_warning() {
    let (actor, handle) = Actor::spawn(None, VirtualCarActor::default(), "OVERSPEED".into())
        .await
        .unwrap();
    let _guard = ActorGuard {
        addr: actor.clone(),
        handle,
    };

    // Phase 5: startup barrier drains automatically.
    let controller = crate::VehicleController::new(actor.clone());
    power_on_to_idle(&controller).await;

    actor
        .send_message(DigitalTwinCarVocabulary::from(FsmEvent::UpdateAmbientLux(
            crate::vehicle_physics::LUX_ON_THRESHOLD + 100,
        )))
        .unwrap();

    let _ = wait_for_ambient_lux(
        &actor,
        crate::vehicle_physics::LUX_ON_THRESHOLD + 100,
        std::time::Duration::from_millis(250),
    )
    .await;

    actor
        .send_message(DigitalTwinCarVocabulary::from(FsmEvent::UpdateRpm(2000)))
        .unwrap();
    actor
        .send_message(DigitalTwinCarVocabulary::from(FsmEvent::UpdateRpm(7500)))
        .unwrap();

    let car = get_snapshot(&actor, DEFAULT_ACTOR_TIMEOUT).await;
    assert!(matches!(
        car.current_state(),
        FsmState::ExtremeOperationWarning(_)
    ));
}

#[tokio::test]
async fn scenario_get_status_after_power_on_reports_idle() {
    let (actor_ref, handle) =
        Actor::spawn(None, VirtualCarActor::default(), "SCENARIO-TEST-01".into())
            .await
            .expect("Failed to start DigitalTwin Actor");
    let _guard = ActorGuard {
        addr: actor_ref.clone(),
        handle,
    };

    // Phase 5: startup barrier drains automatically when headlamp replies ZoneReady.
    let controller = crate::VehicleController::new(actor_ref.clone());
    power_on_to_idle(&controller).await;

    let twin_snapshot = actor_ref
        .call(
            |port| DigitalTwinCarVocabulary::GetStatus(port),
            Some(DEFAULT_ACTOR_TIMEOUT),
        )
        .await
        .expect("Failed to enqueue GetStatus")
        .expect("Actor failed to respond or timed out during GetStatus request");

    assert_eq!(
        *twin_snapshot.current_state(),
        FsmState::Idle,
        "Car should be in Idle state after PowerOn + startup barrier"
    );

    twin_snapshot
        .verify_all_invariants()
        .expect("Safety invariant breach detected in test snapshot");
}
