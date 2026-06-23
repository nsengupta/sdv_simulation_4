//! Actor-oriented contract tests (mailbox -> step -> persistence/emit sequencing).

use crate::digital_twin::DigitalTwinCarVocabulary;
use crate::twin_runtime::controller::vehicle_controller::VehicleControllerRuntimeOptions;
use crate::fsm::{FsmEvent, HeadlampState};
use crate::{PublishedFsmEvent, PublishedFsmState};
use crate::test::{
    expect_actuation_command, inject_matching_ack, inject_matching_nack,
    power_on_to_idle, ActorGuard,
};
use crate::{ActuationCommand, PhysicalCarVocabulary, VehicleController, VssSignal};
use ractor::concurrency::Duration;
use tokio::sync::mpsc;

/// Timeout for actor call in contract tests.
const DEFAULT_ACTOR_TIMEOUT: Duration = Duration::from_millis(250);

#[tokio::test]
async fn scenario_raw_transition_records_are_emitted_in_order() {
    // Phase 1: PowerOn → PreparingToStart (seq 1), AssembliesReady → Idle (seq 2),
    // then UpdateRpm(1500) → Driving (seq 3).
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

    // Phase 7: Off → PreparingToStart → {AssemblyZoneReady(Headlamp)} →
    //           {AssemblyZoneReady(Wiper)} → Idle produces THREE ledger rows.
    // Drain all three before queuing user events.
    power_on_to_idle(&controller).await;
    let row1 = rx.recv().await.expect("Missing row 1 (PowerOn)");
    let row2 = rx.recv().await.expect("Missing row 2 (AssemblyZoneReady Headlamp)");
    let row3 = rx.recv().await.expect("Missing row 3 (AssemblyZoneReady Wiper → Idle)");

    // Now queue lux + rpm events; actor is in Idle.
    actor_ref
        .send_message(FsmEvent::UpdateAmbientLux(crate::vehicle_physics::LUX_ON_THRESHOLD + 100).into())
        .expect("bright lux to prevent LightingUnsafe synthesis in Driving");
    actor_ref
        .send_message(FsmEvent::UpdateRpm(1500).into())
        .expect("Failed to send UpdateRpm stimulus");

    let row4 = rx.recv().await.expect("Missing row 4 (lux)");
    let row5 = rx.recv().await.expect("Missing row 5 (rpm)");

    assert_eq!(row1.record_seq, 1);
    assert_eq!(row1.event, PublishedFsmEvent::PowerOn);
    assert_eq!(row1.old_state, PublishedFsmState::Off);
    assert_eq!(row1.next_state, PublishedFsmState::PreparingToStart);

    // Row 2: AssemblyZoneReady(Headlamp) — stays in PreparingToStart (Wiper still pending).
    assert_eq!(row2.record_seq, 2);
    assert_eq!(row2.next_state, PublishedFsmState::PreparingToStart);

    // Row 3: AssemblyZoneReady(Wiper) — both assemblies ready; transitions to Idle.
    assert_eq!(row3.record_seq, 3);
    assert_eq!(row3.next_state, PublishedFsmState::Idle);

    // Lux row (seq 4) keeps the FSM in Idle (no state change from bright lux).
    assert_eq!(row4.record_seq, 4);

    // RPM row (seq 5) advances to Driving.
    assert_eq!(row5.record_seq, 5);
    assert_eq!(row5.event, PublishedFsmEvent::UpdateRpm(1500));
    assert_eq!(row5.old_state, PublishedFsmState::Idle);
    assert_eq!(row5.next_state, PublishedFsmState::Driving);
    assert_eq!(row5.current_ctx.powertrain.wheel_rpm.front_left, 1500);

    // All records share one run (session epoch) and advance monotonically in wall time.
    assert_eq!(row1.session_epoch_unix_nanos, row5.session_epoch_unix_nanos);
    assert!(row5.at_unix >= row1.at_unix);

    let twin_snapshot = actor_ref
        .call(
            |port| DigitalTwinCarVocabulary::GetStatus(port),
            Some(DEFAULT_ACTOR_TIMEOUT),
        )
        .await
        .expect("Failed to enqueue GetStatus")
        .expect("Actor failed to respond or timed out during GetStatus request");

    let ctx = twin_snapshot.context();
    assert_eq!(
        row5.current_ctx.powertrain.wheel_rpm.front_left,
        ctx.powertrain.wheel_rpm.front_left,
        "emitted current_ctx must match persisted actor context after transition"
    );
    assert_eq!(row5.current_ctx.powertrain.speed_kph, ctx.powertrain.speed_kph);
    assert_eq!(row5.current_ctx.visibility.ambient_lux, ctx.visibility.ambient_lux);
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

    // Drive Off → PreparingToStart → Idle → Driving → ExtremeOperationWarning (redline),
    // which emits the speed-threshold LogWarning intent.
    // Phase 5: startup barrier drains automatically; wait for Idle before sending events.
    power_on_to_idle(&controller).await;
    for evt in [
        FsmEvent::UpdateAmbientLux(crate::vehicle_physics::LUX_ON_THRESHOLD + 100),
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
    let (actuation_tx, mut actuation_rx) = mpsc::channel(16);
    let runtime_options = VehicleControllerRuntimeOptions {
        actuation_command_tx: Some(actuation_tx),
        ..Default::default()
    };
    let (controller, handle) =
        VehicleController::install_and_start_with_options("ACT-ACK-01".to_string(), runtime_options)
            .await
            .expect("start actor");
    let _guard = ActorGuard {
        addr: controller.get_actor_ref().clone(),
        handle,
    };

    // Phase 1: bridge to Idle before sending lux (lux in PreparingToStart is a no-op).
    power_on_to_idle(&controller).await;
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
    crate::test::wait_headlamp_state(&controller, HeadlampState::On, Duration::from_millis(250))
        .await;

    let snapshot = controller
        .get_snapshot(Some(Duration::from_millis(250)))
        .await
        .expect("snapshot");
    assert_eq!(snapshot.context().headlamp.state, HeadlampState::On);
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

    // Phase 1: bridge to Idle before sending lux.
    power_on_to_idle(&controller).await;
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
    let (actuation_tx, mut actuation_rx) = mpsc::channel(16);
    let runtime_options = VehicleControllerRuntimeOptions {
        actuation_command_tx: Some(actuation_tx),
        ..Default::default()
    };
    let (controller, handle) =
        VehicleController::install_and_start_with_options("ACT-NACK-01".to_string(), runtime_options)
            .await
            .expect("start actor");
    let _guard = ActorGuard {
        addr: controller.get_actor_ref().clone(),
        handle,
    };

    // Phase 1: bridge to Idle before sending lux.
    power_on_to_idle(&controller).await;
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
    // Phase 2: NACK on ON request → ActuationIncomplete(On) → Ready (assembly active, lamp dark).
    crate::test::wait_headlamp_state(&controller, HeadlampState::Ready, Duration::from_millis(250))
        .await;

    let snapshot = controller
        .get_snapshot(Some(Duration::from_millis(250)))
        .await
        .expect("snapshot");
    assert_eq!(snapshot.context().headlamp.state, HeadlampState::Ready);
    assert!(snapshot.context().headlamp.ack_pending_since.is_none());
}
