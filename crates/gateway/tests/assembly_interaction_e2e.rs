//! Controller/FSM integration tests for assembly interaction outcomes.
//!
//! Scope:
//! - Uses `VehicleController` at projection boundary.
//! - Drives `PhysicalCarVocabulary` events directly.
//! - Verifies persisted context across all managed assemblies (headlamp, wiper, …).
//!
//! Non-scope:
//! - SocketCAN bus transport wiring (`vcan0`) and separate actuator process orchestration.
//!   Those are covered by runtime/manual smoke scenarios and assembly-specific CAN tests.

use std::time::Duration;

use common::facade::{
    FRONT_HEADLAMP_ON_ACK_WAIT, HeadlampState, PhysicalCarVocabulary, VehicleController,
    VehicleControllerRuntimeOptions, VssSignal, WiperState,
};

async fn wait_headlamp_state(
    controller: &VehicleController,
    expected: HeadlampState,
    timeout: Duration,
) {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Ok(snapshot) = controller.get_snapshot(Some(Duration::from_millis(50))).await {
            if snapshot.context().headlamp.state == expected {
                return;
            }
        }
        if std::time::Instant::now() >= deadline {
            panic!("timed out after {timeout:?} waiting for headlamp {expected:?}");
        }
        tokio::task::yield_now().await;
    }
}

async fn wait_wiper_state(
    controller: &VehicleController,
    expected: WiperState,
    timeout: Duration,
) {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Ok(snapshot) = controller.get_snapshot(Some(Duration::from_millis(50))).await {
            if snapshot.context().wiper.state == expected {
                return;
            }
        }
        if std::time::Instant::now() >= deadline {
            panic!("timed out after {timeout:?} waiting for wiper {expected:?}");
        }
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn headlamp_ack_path() {
    let runtime_options = VehicleControllerRuntimeOptions::default();
    let (controller, _join) = VehicleController::install_and_start_with_options(
        "E2E-FRONT-HEADLAMP-ACK-01".to_string(),
        runtime_options,
    )
    .await
    .expect("controller start");

    controller.send_power_on().await.expect("power on");
    // Both twinlets must complete startup lifecycle before sending user events.
    wait_headlamp_state(&controller, HeadlampState::Ready, Duration::from_millis(500)).await;
    wait_wiper_state(&controller, WiperState::Ready, Duration::from_millis(500)).await;
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
    wait_headlamp_state(&controller, HeadlampState::On, Duration::from_millis(500)).await;

    let snapshot = controller
        .get_snapshot(Some(Duration::from_millis(300)))
        .await
        .expect("snapshot");
    assert_eq!(snapshot.context().headlamp.state, HeadlampState::On);
    assert!(snapshot.context().headlamp.ack_pending_since.is_none());
}

#[tokio::test]
async fn headlamp_nack_path() {
    let runtime_options = VehicleControllerRuntimeOptions::default();
    let (controller, _join) = VehicleController::install_and_start_with_options(
        "E2E-FRONT-HEADLAMP-NACK-01".to_string(),
        runtime_options,
    )
    .await
    .expect("controller start");

    controller.send_power_on().await.expect("power on");
    // Both twinlets must complete startup lifecycle before sending user events.
    wait_headlamp_state(&controller, HeadlampState::Ready, Duration::from_millis(500)).await;
    wait_wiper_state(&controller, WiperState::Ready, Duration::from_millis(500)).await;
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
    // After NACK on OnRequested → ActuationIncomplete(On) → Back to Ready (assembly active).
    wait_headlamp_state(&controller, HeadlampState::Ready, Duration::from_millis(500)).await;

    let snapshot = controller
        .get_snapshot(Some(Duration::from_millis(300)))
        .await
        .expect("snapshot");
    assert_eq!(snapshot.context().headlamp.state, HeadlampState::Ready);
    assert!(snapshot.context().headlamp.ack_pending_since.is_none());
}

#[tokio::test]
async fn headlamp_no_response_timeout_path() {
    let runtime_options = VehicleControllerRuntimeOptions::default();
    let (controller, _join) = VehicleController::install_and_start_with_options(
        "E2E-FRONT-HEADLAMP-TIMEOUT-01".to_string(),
        runtime_options,
    )
    .await
    .expect("controller start");

    controller.send_power_on().await.expect("power on");
    // Both twinlets must complete startup lifecycle (FSM → Idle) before sending user events.
    // AmbientLux during PreparingToStart is a PassthroughBarrier; the headlamp would never
    // reach OnRequested and the ACK timer would never fire.
    wait_headlamp_state(&controller, HeadlampState::Ready, Duration::from_millis(500)).await;
    wait_wiper_state(&controller, WiperState::Ready, Duration::from_millis(500)).await;
    controller
        .submit_physical_car_event(PhysicalCarVocabulary::TelemetryUpdate(VssSignal::AmbientLux(
            20,
        )))
        .await
        .expect("low lux event");

    // No ACK/NACK event sent: headlamp twinlet ACK timer fires without gateway TimerTick.
    tokio::time::sleep(FRONT_HEADLAMP_ON_ACK_WAIT + Duration::from_millis(25)).await;
    // After ACK timeout → ActuationIncomplete(On) → Back to Ready (assembly active).
    wait_headlamp_state(&controller, HeadlampState::Ready, Duration::from_millis(500)).await;

    let snapshot = controller
        .get_snapshot(Some(Duration::from_millis(300)))
        .await
        .expect("snapshot");
    assert_eq!(snapshot.context().headlamp.state, HeadlampState::Ready);
    assert!(snapshot.context().headlamp.ack_pending_since.is_none());
}
