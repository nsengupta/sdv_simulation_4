//! Compatibility contract tests ensuring `fsm::engine` remains a stable shim
//! over `engine::op_strategy::transition_map`.

use crate::engine::op_strategy::transition_map;
use crate::fsm::{output, transition, FsmAction, FsmEvent, FsmState, LightingState, VehicleContext};
use crate::vehicle_constants::{
    EXTREME_OPERATION_WARNING_MESSAGE, SPEED_THRESHOLD_WARNING_MESSAGE,
};
use crate::vehicle_kinematics::refresh_context_speed;
use std::time::Instant;

fn valid_twin_context() -> VehicleContext {
    VehicleContext {
        rpm: 0,
        speed: 0,
        fuel_level: 85,
        oil_pressure: 30,
        tyre_pressure_ok: true,
        ambient_lux: 100,
        lighting_state: LightingState::Off,
        lighting_ack_pending_since: None,
    }
}

#[test]
fn given_driving_when_high_rpm_then_shim_and_strategy_transition_match() {
    let now = Instant::now();
    let mut ctx = valid_twin_context();
    ctx.rpm = 5600;
    refresh_context_speed(&mut ctx);

    let via_shim = transition(&FsmState::Driving, &FsmEvent::UpdateRpm(5600), &ctx, now);
    let via_strategy =
        transition_map::transition(&FsmState::Driving, &FsmEvent::UpdateRpm(5600), &ctx, now);

    assert_eq!(via_shim, via_strategy);
}

#[test]
fn given_driving_when_both_extreme_thresholds_exceeded_then_enters_warning() {
    let now = Instant::now();
    let mut ctx = valid_twin_context();
    ctx.rpm = 5600;
    refresh_context_speed(&mut ctx);

    let via_shim = transition(&FsmState::Driving, &FsmEvent::UpdateRpm(5600), &ctx, now);
    let via_strategy =
        transition_map::transition(&FsmState::Driving, &FsmEvent::UpdateRpm(5600), &ctx, now);

    assert_eq!(via_shim, via_strategy);
    assert!(matches!(via_shim, FsmState::ExtremeOperationWarning(_)));

    let actions = output(&FsmState::Driving, &via_shim, &ctx);
    assert!(actions.contains(&FsmAction::LogWarning(
        SPEED_THRESHOLD_WARNING_MESSAGE.to_string()
    )));
    assert!(actions.contains(&FsmAction::LogWarning(
        EXTREME_OPERATION_WARNING_MESSAGE.to_string()
    )));
}

#[test]
fn given_warning_recovery_when_transition_occurs_then_shim_and_strategy_output_match() {
    let old_state = FsmState::ExtremeOperationWarning(Instant::now());
    let new_state = FsmState::Driving;
    let ctx = valid_twin_context();

    let via_shim = output(&old_state, &new_state, &ctx);
    let via_strategy = transition_map::output(&old_state, &new_state, &ctx);

    assert_eq!(via_shim, via_strategy);
}
