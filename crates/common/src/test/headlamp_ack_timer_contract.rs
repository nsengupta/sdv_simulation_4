//! Item 2 — headlamp twinlet owns ACK wait via ractor `send_after`; brain commits on
//! [`DigitalTwinCarVocabulary::HeadlampZoneSpontaneous`], not gateway `TimerTick`.

use std::time::Duration;

use crate::fsm::{FsmEvent, FsmState, HeadlampState};
use crate::published::{
    PublishedDomainAction, PublishedFsmEvent, PublishedFsmState, PublishedOperational,
};
use crate::test::{power_on_to_idle, submit_daylight_ambient, wait_fsm_state, wait_headlamp_state, ActorGuard};
use crate::twin_runtime::controller::vehicle_controller::VehicleControllerRuntimeOptions;
use crate::vehicle_physics::{FRONT_HEADLAMP_ON_ACK_WAIT, RPM_DRIVING_THRESHOLD};
use crate::{PhysicalCarVocabulary, VehicleController, VssSignal};
use tokio::sync::mpsc;

#[tokio::test]
async fn given_actor_driving_in_dark_when_ack_wait_elapses_without_timer_tick_then_two_ledger_rows_and_driving_dangerously(
) {
    let (transition_tx, mut rx) = mpsc::channel(16);
    let runtime_options = VehicleControllerRuntimeOptions {
        transition_tx: Some(transition_tx),
        ..VehicleControllerRuntimeOptions::default()
    };

    let (controller, handle) = VehicleController::install_and_start_with_options(
        "ACK-TIMER-01".to_string(),
        runtime_options,
    )
    .await
    .expect("start actor");
    let _guard = ActorGuard {
        addr: controller.get_actor_ref().clone(),
        handle,
    };

    // Phase 1: bridge to Idle, then drain the two startup ledger rows.
    power_on_to_idle(&controller).await;
    let _ = rx.recv().await.expect("power on → preparing row");
    let _ = rx.recv().await.expect("assemblies ready → idle row");

    submit_daylight_ambient(&controller).await;
    let _ = rx.recv().await.expect("bright lux row");

    controller
        .submit_fsm_event(FsmEvent::UpdateRpm(RPM_DRIVING_THRESHOLD + 200))
        .await
        .expect("rpm");
    let _ = rx.recv().await.expect("rpm row");

    controller
        .submit_physical_car_event(PhysicalCarVocabulary::TelemetryUpdate(VssSignal::AmbientLux(
            20,
        )))
        .await
        .expect("low lux");
    let lux_row = rx.recv().await.expect("lux row");
    assert_eq!(lux_row.next_state, PublishedFsmState::Driving);
    wait_headlamp_state(&controller, HeadlampState::OnRequested, Duration::from_secs(1)).await;

    tokio::time::sleep(FRONT_HEADLAMP_ON_ACK_WAIT + Duration::from_millis(25)).await;

    let hop1 = rx.recv().await.expect("spontaneous incomplete hop ledger row");
    let hop2 = rx.recv().await.expect("internal hop ledger row");

    assert!(
        matches!(
            hop1.event,
            PublishedFsmEvent::FrontHeadlampActuationIncomplete { .. }
        ),
        "ACK timeout must ledger as actuation incomplete, not TimerTick, got {:?}",
        hop1.event
    );
    assert_eq!(hop1.next_state, PublishedFsmState::Driving);
    assert!(matches!(
        hop2.event,
        PublishedFsmEvent::Internal(PublishedOperational::LightingUnsafe)
    ));
    assert_eq!(hop2.next_state, PublishedFsmState::DrivingDangerously);
    assert!(
        hop2
            .actions
            .iter()
            .any(|a| matches!(a, PublishedDomainAction::StartBuzzer)),
        "internal hop row must carry StartBuzzer, got {:?}",
        hop2.actions
    );

    wait_fsm_state(
        &controller,
        FsmState::DrivingDangerously,
        Duration::from_secs(1),
    )
    .await;
    wait_headlamp_state(&controller, HeadlampState::Off, Duration::from_secs(1)).await;
}

#[tokio::test]
async fn given_actor_on_requested_when_ack_before_deadline_then_no_spontaneous_incomplete_row() {
    let (transition_tx, mut rx) = mpsc::channel(16);
    let (actuation_tx, mut actuation_rx) = mpsc::channel(8);
    let runtime_options = VehicleControllerRuntimeOptions {
        transition_tx: Some(transition_tx),
        actuation_command_tx: Some(actuation_tx),
        ..VehicleControllerRuntimeOptions::default()
    };

    let (controller, handle) = VehicleController::install_and_start_with_options(
        "ACK-TIMER-02".to_string(),
        runtime_options,
    )
    .await
    .expect("start actor");
    let _guard = ActorGuard {
        addr: controller.get_actor_ref().clone(),
        handle,
    };

    // Phase 1: bridge to Idle, drain startup rows.
    power_on_to_idle(&controller).await;
    let _ = rx.recv().await.expect("power on → preparing row");
    let _ = rx.recv().await.expect("assemblies ready → idle row");

    controller
        .submit_physical_car_event(PhysicalCarVocabulary::TelemetryUpdate(VssSignal::AmbientLux(
            20,
        )))
        .await
        .expect("low lux");
    let _ = rx.recv().await.expect("lux row");
    wait_headlamp_state(&controller, HeadlampState::OnRequested, Duration::from_secs(1)).await;

    let command = crate::test::expect_actuation_command(
        &mut actuation_rx,
        Duration::from_secs(1),
    )
    .await;
    crate::test::inject_matching_ack(&controller, &command).await;
    let ack_row = rx.recv().await.expect("ack row");
    assert!(
        matches!(ack_row.event, PublishedFsmEvent::FrontHeadlampOnAck),
        "expected on-ack row, got {:?}",
        ack_row.event
    );

    tokio::time::sleep(FRONT_HEADLAMP_ON_ACK_WAIT + Duration::from_millis(25)).await;

    assert!(
        rx.try_recv().is_err(),
        "no spontaneous incomplete row after successful ACK"
    );
    wait_headlamp_state(&controller, HeadlampState::On, Duration::from_secs(1)).await;
}
