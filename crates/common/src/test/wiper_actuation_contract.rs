//! Wiper actuation path contract tests.
//!
//! Covers Steps 2–5 and Step 9 (end-to-end rain ingress) of the Wiper implementation plan.

use std::time::Duration;

use crate::digital_twin::DigitalTwinCar;
use crate::fsm::FsmState;
use crate::test::{
    expect_actuation_command, install_with_actuation, power_on_to_idle,
    wiper_zone_contract::wait_wiper_state,
};
use crate::vehicle_state::{VehicleContext, WiperState};
use crate::{PhysicalCarVocabulary, VssSignal};
use crate::fsm::DomainAction;
use crate::twin_runtime::controller::actuation_contract::ActuationCommand;
use crate::twin_runtime::controller::actuation_manager::{ActuationManager, DefaultActuationManager};
use crate::twin_runtime::outcome_map::zone_outcomes_to_domain_actions;
use crate::twin_runtime::zone_turn::ZoneOutcome;
use crate::vehicle_state::WiperOutcome;

// ── Step 2: DomainAction variants ─────────────────────────────────────────────

#[test]
fn given_request_wiper_actions_when_compared_then_distinct() {
    assert_ne!(
        format!("{:?}", DomainAction::RequestWiperStart),
        format!("{:?}", DomainAction::RequestWiperStop),
        "RequestWiperStart and RequestWiperStop must be distinct variants"
    );
}

// ── Step 3: outcome_map wiper path ────────────────────────────────────────────

#[test]
fn given_start_wiping_outcome_when_mapped_then_request_wiper_start() {
    let outcomes = vec![ZoneOutcome::Wiper(WiperOutcome::StartWiping)];
    let actions = zone_outcomes_to_domain_actions(outcomes);
    assert_eq!(actions, vec![DomainAction::RequestWiperStart]);
}

#[test]
fn given_stop_wiping_outcome_when_mapped_then_request_wiper_stop() {
    let outcomes = vec![ZoneOutcome::Wiper(WiperOutcome::StopWiping)];
    let actions = zone_outcomes_to_domain_actions(outcomes);
    assert_eq!(actions, vec![DomainAction::RequestWiperStop]);
}

#[test]
fn given_wiper_log_warning_outcome_when_mapped_then_domain_log_warning() {
    let msg = "wiper unresponsive".to_string();
    let outcomes = vec![ZoneOutcome::Wiper(WiperOutcome::LogWarning(msg.clone()))];
    let actions = zone_outcomes_to_domain_actions(outcomes);
    assert_eq!(actions, vec![DomainAction::LogWarning(msg)]);
}

// ── Step 5: actuation_manager sends commands on channel ───────────────────────

fn blank_twin() -> DigitalTwinCar {
    DigitalTwinCar::new("test-wiper", FsmState::Off, VehicleContext::default())
        .expect("test twin must be constructible")
}

#[tokio::test]
async fn given_request_wiper_start_when_actuation_manager_executes_then_sends_start_wiper() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<ActuationCommand>(4);
    let mgr = DefaultActuationManager::with_command_channel("test".into(), 0, tx);
    mgr.execute(&DomainAction::RequestWiperStart, &blank_twin())
        .await
        .expect("execute must not fail");
    let cmd = rx.try_recv().expect("StartWiper command must be sent");
    assert!(matches!(cmd, ActuationCommand::StartWiper));
}

#[tokio::test]
async fn given_request_wiper_stop_when_actuation_manager_executes_then_sends_stop_wiper() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<ActuationCommand>(4);
    let mgr = DefaultActuationManager::with_command_channel("test".into(), 0, tx);
    mgr.execute(&DomainAction::RequestWiperStop, &blank_twin())
        .await
        .expect("execute must not fail");
    let cmd = rx.try_recv().expect("StopWiper command must be sent");
    assert!(matches!(cmd, ActuationCommand::StopWiper));
}

// ── Step 4: ActuationCommand variants ─────────────────────────────────────────

#[test]
fn given_wiper_actuation_commands_when_compared_then_distinct() {
    assert_ne!(
        format!("{:?}", ActuationCommand::StartWiper),
        format!("{:?}", ActuationCommand::StopWiper),
        "StartWiper and StopWiper must be distinct variants"
    );
}

// ── Step 9: end-to-end physical rain ingress ──────────────────────────────────

#[tokio::test]
async fn given_idle_wiper_ready_when_rain_detected_true_ingress_then_running_and_start_wiper_command() {
    let (controller, mut actuation_rx, _guard) = install_with_actuation("WIPER-E2E-1", 8).await;
    power_on_to_idle(&controller).await;
    wait_wiper_state(&controller, WiperState::Ready, Duration::from_millis(500)).await;

    controller
        .submit_physical_car_event(PhysicalCarVocabulary::TelemetryUpdate(
            VssSignal::RainDetected(true),
        ))
        .await
        .expect("rain ingress");

    wait_wiper_state(&controller, WiperState::Running, Duration::from_millis(500)).await;

    let cmd = expect_actuation_command(&mut actuation_rx, Duration::from_secs(1)).await;
    assert!(matches!(cmd, ActuationCommand::StartWiper), "got {cmd:?}");
}

#[tokio::test]
async fn given_wiper_running_when_rain_detected_false_ingress_then_ready_and_stop_wiper_command() {
    let (controller, mut actuation_rx, _guard) = install_with_actuation("WIPER-E2E-2", 8).await;
    power_on_to_idle(&controller).await;
    wait_wiper_state(&controller, WiperState::Ready, Duration::from_millis(500)).await;

    controller
        .submit_physical_car_event(PhysicalCarVocabulary::TelemetryUpdate(
            VssSignal::RainDetected(true),
        ))
        .await
        .expect("start rain");
    let _ = expect_actuation_command(&mut actuation_rx, Duration::from_secs(1)).await;
    wait_wiper_state(&controller, WiperState::Running, Duration::from_millis(500)).await;

    controller
        .submit_physical_car_event(PhysicalCarVocabulary::TelemetryUpdate(
            VssSignal::RainDetected(false),
        ))
        .await
        .expect("stop rain");

    wait_wiper_state(&controller, WiperState::Ready, Duration::from_millis(500)).await;
    let cmd = expect_actuation_command(&mut actuation_rx, Duration::from_secs(1)).await;
    assert!(matches!(cmd, ActuationCommand::StopWiper), "got {cmd:?}");
}
