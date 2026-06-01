//! Contract tests for [`crate::fsm::transition_map`] (operational mode table).

use crate::fsm::{output, transition, FsmAction, FsmEvent, FsmState};
use crate::vehicle_state::VehicleContext;
use crate::vehicle_physics::{
    EXTREME_OPERATION_WARNING_MESSAGE, SPEED_THRESHOLD_WARNING_MESSAGE,
};
use std::time::Instant;

fn valid_twin_context() -> VehicleContext {
    VehicleContext::default()
}

#[test]
fn given_driving_when_high_rpm_then_enters_extreme_operation_warning() {
    let now = Instant::now();
    let mut ctx = valid_twin_context();
    ctx.powertrain.wheel_rpm.front_left = 5600;
    ctx.powertrain.refresh_speed();

    let result = transition(&FsmState::Driving, &FsmEvent::UpdateRpm(5600), &ctx, now);

    assert!(matches!(result.next_state, FsmState::ExtremeOperationWarning(_)));
}

#[test]
fn given_driving_when_both_extreme_thresholds_exceeded_then_emits_both_warnings() {
    let now = Instant::now();
    let mut ctx = valid_twin_context();
    ctx.powertrain.wheel_rpm.front_left = 5600;
    ctx.powertrain.refresh_speed();

    let result = transition(&FsmState::Driving, &FsmEvent::UpdateRpm(5600), &ctx, now);
    assert!(matches!(result.next_state, FsmState::ExtremeOperationWarning(_)));

    let actions = output(&FsmState::Driving, &result.next_state, &ctx);
    assert!(actions.contains(&FsmAction::LogWarning(
        SPEED_THRESHOLD_WARNING_MESSAGE.to_string()
    )));
    assert!(actions.contains(&FsmAction::LogWarning(
        EXTREME_OPERATION_WARNING_MESSAGE.to_string()
    )));
}

#[test]
fn given_warning_recovery_when_transition_to_driving_then_stops_buzzer() {
    let old_state = FsmState::ExtremeOperationWarning(Instant::now());
    let new_state = FsmState::Driving;
    let ctx = valid_twin_context();

    let actions = output(&old_state, &new_state, &ctx);
    assert_eq!(actions, vec![FsmAction::StopBuzzer]);
}
