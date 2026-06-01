//! Controller/FSM integration tests for front-headlamp command outcomes.
//!
//! Scope:
//! - Uses `VehicleController` at projection boundary.
//! - Drives `PhysicalCarVocabulary` events directly.
//! - Verifies persisted context for ACK / NACK / timeout outcomes.
//!
//! Non-scope:
//! - SocketCAN bus transport wiring (`vcan0`) and separate actuator process orchestration.
//!   Those are covered by runtime/manual smoke scenarios and bus-level tests.

use std::time::Duration;

use common::facade::{
    FRONT_HEADLAMP_ON_ACK_WAIT, LightingState, PhysicalCarVocabulary, VehicleController,
    VehicleControllerRuntimeOptions, VssSignal,
};

#[tokio::test]
async fn controller_fsm_front_headlamp_ack_path() {
    let runtime_options = VehicleControllerRuntimeOptions::default();
    let (controller, _join) = VehicleController::install_and_start_with_options(
        "E2E-FRONT-HEADLAMP-ACK-01".to_string(),
        runtime_options,
    )
    .await
    .expect("controller start");

    controller.send_power_on().await.expect("power on");
    controller
        .submit_physical_car_event(PhysicalCarVocabulary::TelemetryUpdate(VssSignal::AmbientLux(
            20,
        )))
        .await
        .expect("low lux event");
    controller
        .submit_physical_car_event(PhysicalCarVocabulary::FrontHeadlampCommandConfirmed {
            on_command: true,
        })
        .await
        .expect("ack confirm event");

    let snapshot = controller
        .get_snapshot(Some(Duration::from_millis(300)))
        .await
        .expect("snapshot");
    assert_eq!(snapshot.context().headlamp.state, LightingState::On);
    assert!(snapshot.context().headlamp.ack_pending_since.is_none());
}

#[tokio::test]
async fn controller_fsm_front_headlamp_nack_path() {
    let runtime_options = VehicleControllerRuntimeOptions::default();
    let (controller, _join) = VehicleController::install_and_start_with_options(
        "E2E-FRONT-HEADLAMP-NACK-01".to_string(),
        runtime_options,
    )
    .await
    .expect("controller start");

    controller.send_power_on().await.expect("power on");
    controller
        .submit_physical_car_event(PhysicalCarVocabulary::TelemetryUpdate(VssSignal::AmbientLux(
            20,
        )))
        .await
        .expect("low lux event");
    controller
        .submit_physical_car_event(PhysicalCarVocabulary::FrontHeadlampCommandRejected {
            on_command: true,
        })
        .await
        .expect("nack reject event");

    let snapshot = controller
        .get_snapshot(Some(Duration::from_millis(300)))
        .await
        .expect("snapshot");
    assert_eq!(snapshot.context().headlamp.state, LightingState::Off);
    assert!(snapshot.context().headlamp.ack_pending_since.is_none());
}

#[tokio::test]
async fn controller_fsm_front_headlamp_no_response_timeout_path() {
    let runtime_options = VehicleControllerRuntimeOptions::default();
    let (controller, _join) = VehicleController::install_and_start_with_options(
        "E2E-FRONT-HEADLAMP-TIMEOUT-01".to_string(),
        runtime_options,
    )
    .await
    .expect("controller start");

    controller.send_power_on().await.expect("power on");
    controller
        .submit_physical_car_event(PhysicalCarVocabulary::TelemetryUpdate(VssSignal::AmbientLux(
            20,
        )))
        .await
        .expect("low lux event");

    // No ACK/NACK event sent: emulate silence, then drive TimerTick past ACK wait deadline.
    tokio::time::sleep(FRONT_HEADLAMP_ON_ACK_WAIT + Duration::from_millis(25)).await;
    controller
        .submit_physical_car_event(PhysicalCarVocabulary::TimerTick)
        .await
        .expect("timer tick");

    let snapshot = controller
        .get_snapshot(Some(Duration::from_millis(300)))
        .await
        .expect("snapshot");
    assert_eq!(snapshot.context().headlamp.state, LightingState::Off);
    assert!(snapshot.context().headlamp.ack_pending_since.is_none());
}
