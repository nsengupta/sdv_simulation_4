//! Actor-oriented contract tests (mailbox -> step -> persistence/emit sequencing).

use crate::digital_twin::DigitalTwinCarVocabulary;
use crate::twin_runtime::controller::vehicle_controller::VehicleControllerRuntimeOptions;
use crate::fsm::{FsmEvent, LightingState};
use crate::{PublishedFsmEvent, PublishedFsmState};
use crate::test::{
    expect_actuation_command, inject_matching_ack, inject_matching_nack, install_with_actuation,
    ActorGuard,
};
use crate::{ActuationCommand, PhysicalCarVocabulary, VehicleController, VssSignal};
use ractor::concurrency::Duration;
use tokio::sync::mpsc;

/// Timeout for actor call in contract tests.
const DEFAULT_ACTOR_TIMEOUT: Duration = Duration::from_millis(250);

#[tokio::test]
async fn scenario_raw_transition_records_are_emitted_in_order() {
    let (tx, mut rx) = mpsc::channel(16);

    let runtime_options = VehicleControllerRuntimeOptions {
        transition_tx: Some(tx),
        ..VehicleControllerRuntimeOptions::default()
    };

    let (controller, handle) = VehicleController::install_and_start_with_options(
        "SCENARIO-LOGGING-01".to_string(),
        runtime_options,
    )
    .await
    .expect("Failed to start DigitalTwin Actor with sink");
    let actor_ref = controller.get_actor_ref().clone();
    let _guard = ActorGuard {
        addr: actor_ref.clone(),
        handle,
    };

    actor_ref
        .send_message(FsmEvent::PowerOn.into())
        .expect("Failed to send PowerOn stimulus");
    actor_ref
        .send_message(FsmEvent::UpdateRpm(1500).into())
        .expect("Failed to send UpdateRpm stimulus");

    let first = rx.recv().await.expect("Missing first transition record");
    let second = rx.recv().await.expect("Missing second transition record");

    assert_eq!(first.record_seq, 1);
    assert_eq!(first.event, PublishedFsmEvent::PowerOn);
    assert_eq!(first.old_state, PublishedFsmState::Off);
    assert_eq!(first.next_state, PublishedFsmState::Idle);
    assert_eq!(first.current_ctx.powertrain.wheel_rpm.front_left, 0);

    assert_eq!(second.record_seq, 2);
    assert_eq!(second.event, PublishedFsmEvent::UpdateRpm(1500));
    assert_eq!(second.old_state, PublishedFsmState::Idle);
    assert_eq!(second.next_state, PublishedFsmState::Driving);
    assert_eq!(second.current_ctx.powertrain.wheel_rpm.front_left, 1500);

    // Both records share one run (session epoch) and advance monotonically in wall time.
    assert_eq!(first.session_epoch_unix_nanos, second.session_epoch_unix_nanos);
    assert!(second.at_unix >= first.at_unix);

    let twin_snapshot = actor_ref
        .call(
            |port| DigitalTwinCarVocabulary::GetStatus(port),
            Some(DEFAULT_ACTOR_TIMEOUT),
        )
        .await
        .expect("Failed to enqueue GetStatus")
        .expect("Actor failed to respond or timed out during GetStatus request");

    // The published context mirrors the persisted actor context on its observable fields
    // (the projection only swaps Instant anchors for wall-clock Durations).
    let ctx = twin_snapshot.context();
    assert_eq!(
        second.current_ctx.powertrain.wheel_rpm.front_left,
        ctx.powertrain.wheel_rpm.front_left,
        "emitted current_ctx must match persisted actor context after transition"
    );
    assert_eq!(second.current_ctx.powertrain.speed_kph, ctx.powertrain.speed_kph);
    assert_eq!(second.current_ctx.visibility.ambient_lux, ctx.visibility.ambient_lux);
}

#[tokio::test]
async fn scenario_log_warning_is_routed_to_diagnostic_sink() {
    // WI-5: a LogWarning domain intent must surface on the diagnostic stream (Warning level),
    // not through the actuation path.
    let (diag_tx, mut diag_rx) = mpsc::unbounded_channel();

    let runtime_options = VehicleControllerRuntimeOptions {
        diagnostic_tx: Some(diag_tx),
        ..VehicleControllerRuntimeOptions::default()
    };

    let (controller, handle) = VehicleController::install_and_start_with_options(
        "SCENARIO-WARN-01".to_string(),
        runtime_options,
    )
    .await
    .expect("Failed to start DigitalTwin Actor with diagnostic sink");
    let actor_ref = controller.get_actor_ref().clone();
    let _guard = ActorGuard {
        addr: actor_ref.clone(),
        handle,
    };

    // Drive Off -> Idle -> Driving -> ExtremeOperationWarning (redline), which emits the
    // speed-threshold LogWarning intent.
    for evt in [
        FsmEvent::PowerOn,
        FsmEvent::UpdateRpm(2000),
        FsmEvent::UpdateRpm(7500),
    ] {
        actor_ref
            .send_message(evt.into())
            .expect("Failed to send stimulus");
    }

    let mut saw_warning = false;
    while let Ok(Some(msg)) =
        tokio::time::timeout(Duration::from_millis(250), diag_rx.recv()).await
    {
        if msg.level == crate::DiagnosticLevel::Warning
            && msg.message.contains(crate::SPEED_THRESHOLD_WARNING_MESSAGE)
        {
            saw_warning = true;
            break;
        }
    }

    assert!(
        saw_warning,
        "LogWarning intent should surface as a Warning-level diagnostic"
    );
}

#[tokio::test]
async fn scenario_actuation_ack_round_trip_via_helper() {
    // WI-6 (Q2): observe the outbound command, inject the matching ack, observe the resulting
    // transition — the harness standing in for the future actuation child actor.
    let (controller, mut actuation_rx, _guard) = install_with_actuation("ACT-ACK-01", 16).await;

    controller.send_power_on().await.expect("power on");
    controller
        .submit_physical_car_event(PhysicalCarVocabulary::TelemetryUpdate(VssSignal::AmbientLux(
            20,
        )))
        .await
        .expect("low lux event");

    let command = expect_actuation_command(&mut actuation_rx, Duration::from_millis(250)).await;
    assert!(
        matches!(command, ActuationCommand::SwitchFrontHeadlampOn { .. }),
        "low lux should request the front headlamp ON, got {command:?}"
    );

    inject_matching_ack(&controller, &command).await;

    let snapshot = controller
        .get_snapshot(Some(Duration::from_millis(250)))
        .await
        .expect("snapshot");
    assert_eq!(snapshot.context().headlamp.state, LightingState::On);
    assert!(snapshot.context().headlamp.ack_pending_since.is_none());
}

#[tokio::test]
async fn scenario_actuation_ack_surfaces_confirmation_on_diagnostic_sink() {
    // A positive headlamp ACK (settle `OnRequested -> On`) must surface on the diagnostic stream
    // as an Info-level confirmation — symmetric with the NACK/timeout Warning path, which was
    // previously the only actuation outcome visible there.
    let (diag_tx, mut diag_rx) = mpsc::unbounded_channel();
    let (actuation_tx, _actuation_rx) = mpsc::channel(16);

    let runtime_options = VehicleControllerRuntimeOptions {
        diagnostic_tx: Some(diag_tx),
        actuation_command_tx: Some(actuation_tx),
        ..VehicleControllerRuntimeOptions::default()
    };

    let (controller, handle) = VehicleController::install_and_start_with_options(
        "ACT-ACK-DIAG-01".to_string(),
        runtime_options,
    )
    .await
    .expect("Failed to start DigitalTwin Actor with diagnostic + actuation sinks");
    let _guard = ActorGuard {
        addr: controller.get_actor_ref().clone(),
        handle,
    };

    controller.send_power_on().await.expect("power on");
    controller
        .submit_physical_car_event(PhysicalCarVocabulary::TelemetryUpdate(VssSignal::AmbientLux(
            20,
        )))
        .await
        .expect("low lux event requests headlamp ON");
    controller
        .submit_physical_car_event(PhysicalCarVocabulary::FrontHeadlampCommandConfirmed {
            on_command: true,
        })
        .await
        .expect("inject matching ON ack");

    let mut saw_confirmation = false;
    while let Ok(Some(msg)) =
        tokio::time::timeout(Duration::from_millis(250), diag_rx.recv()).await
    {
        if msg.level == crate::DiagnosticLevel::Info && msg.message.contains(crate::MSG_ACK_ON) {
            saw_confirmation = true;
            break;
        }
    }

    assert!(
        saw_confirmation,
        "a positive headlamp ACK should surface as an Info-level confirmation diagnostic"
    );
}

#[tokio::test]
async fn scenario_actuation_nack_round_trip_via_helper() {
    let (controller, mut actuation_rx, _guard) = install_with_actuation("ACT-NACK-01", 16).await;

    controller.send_power_on().await.expect("power on");
    controller
        .submit_physical_car_event(PhysicalCarVocabulary::TelemetryUpdate(VssSignal::AmbientLux(
            20,
        )))
        .await
        .expect("low lux event");

    let command = expect_actuation_command(&mut actuation_rx, Duration::from_millis(250)).await;
    assert!(matches!(
        command,
        ActuationCommand::SwitchFrontHeadlampOn { .. }
    ));

    inject_matching_nack(&controller, &command).await;

    // A NACK on the ON request leaves the headlamp Off (the request did not complete).
    let snapshot = controller
        .get_snapshot(Some(Duration::from_millis(250)))
        .await
        .expect("snapshot");
    assert_eq!(snapshot.context().headlamp.state, LightingState::Off);
    assert!(snapshot.context().headlamp.ack_pending_since.is_none());
}
