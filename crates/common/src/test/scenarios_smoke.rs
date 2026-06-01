//! Black-box scenario smoke tests: actor lifecycle + state journeys.

use crate::digital_twin::DigitalTwinCarVocabulary;
use crate::twin_runtime::controller::virtual_car_actor::VirtualCarActor;
use crate::fsm::{FsmEvent, FsmState};
use crate::test::ActorGuard;
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

    actor
        .send_message(DigitalTwinCarVocabulary::from(FsmEvent::PowerOn))
        .unwrap();
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

    actor
        .send_message(DigitalTwinCarVocabulary::from(FsmEvent::PowerOn))
        .unwrap();
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

    actor_ref
        .send_message(FsmEvent::PowerOn.into())
        .expect("Failed to send PowerOn stimulus");

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
        "Car should be in Idle state after PowerOn"
    );

    twin_snapshot
        .verify_all_invariants()
        .expect("Safety invariant breach detected in test snapshot");
}
