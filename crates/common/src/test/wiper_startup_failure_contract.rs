//! Contract: when the wiper twinlet never replies (simulated by `test_silent_wiper`),
//! the tell-back timeout exhausts, a synthetic `WiperOutcome::LogWarning` is emitted,
//! and the actor's diagnostic stream receives a warning message.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::diagnostic::DiagnosticMessage;
use crate::fsm::FsmState;
use crate::test::{wait_fsm_state, ActorGuard};
use crate::twin_runtime::constants::{ZONE_TELL_BACK_ATTEMPT_COUNT, ZONE_TELL_BACK_WAIT};
use crate::twin_runtime::controller::vehicle_controller::VehicleControllerRuntimeOptions;
use crate::digital_twin::DigitalTwinCarVocabulary;
use crate::VehicleController;

/// Total time for one tell-back cycle to exhaust (initial + all retries).
fn full_exhaustion_budget() -> Duration {
    ZONE_TELL_BACK_WAIT * (ZONE_TELL_BACK_ATTEMPT_COUNT + 1)
}

/// Drain the diagnostic channel, returning all messages received within `window`.
async fn drain_diagnostics(
    rx: &mut mpsc::UnboundedReceiver<DiagnosticMessage>,
    window: Duration,
) -> Vec<DiagnosticMessage> {
    let mut msgs = vec![];
    let deadline = tokio::time::Instant::now() + window;
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(m)) => msgs.push(m),
            _ => break,
        }
    }
    msgs
}

#[tokio::test]
async fn given_silent_wiper_when_startup_tell_back_exhausted_then_warning_on_diagnostic_stream() {
    let (diag_tx, mut diag_rx) = mpsc::unbounded_channel::<DiagnosticMessage>();
    let opts = VehicleControllerRuntimeOptions {
        diagnostic_tx: Some(diag_tx),
        test_silent_wiper: true,
        ..Default::default()
    };
    let (controller, handle) =
        VehicleController::install_and_start_with_options("WIPER-FAIL-01".to_string(), opts)
            .await
            .expect("install actor");
    let _guard = ActorGuard::<DigitalTwinCarVocabulary> {
        addr: controller.get_actor_ref().clone(),
        handle,
    };

    // Power on → starts the startup assembly barriers; wiper stays silent.
    controller.send_power_on().await.expect("power on");

    // FSM must reach Idle once the headlamp barrier drains (wiper will use synthetic reply).
    wait_fsm_state(&controller, FsmState::Idle, full_exhaustion_budget() * 3).await;

    // Collect all diagnostics that arrived by now.
    let messages = drain_diagnostics(&mut diag_rx, Duration::from_millis(50)).await;

    let has_wiper_warning = messages.iter().any(|m| {
        m.message.to_lowercase().contains("wiper") && m.level == crate::diagnostic::DiagnosticLevel::Warning
    });
    assert!(
        has_wiper_warning,
        "expected a Warn-level diagnostic mentioning 'wiper' after tell-back exhaustion, got: {messages:#?}"
    );
}
