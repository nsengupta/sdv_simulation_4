//! Unit tests for the FSM spec (`transition` / `output`).

use crate::fsm::{output, transition, FsmAction, FsmEvent, FsmState};
use crate::vehicle_state::VehicleContext;
use crate::vehicle_physics::{
    extreme_operation_active, EXTREME_OPERATION_WARNING_MESSAGE, RPM_EXTREME_OPERATION_THRESHOLD,
    SPEED_EXTREME_OPERATION_THRESHOLD_KPH, SPEED_THRESHOLD_WARNING_MESSAGE,
};
use std::time::{Duration, Instant};

fn ctx_with_rpm(rpm: u16) -> VehicleContext {
    let mut ctx = VehicleContext::default();
    ctx.powertrain.wheel_rpm.front_left = rpm;
    ctx.powertrain.wheel_rpm.front_right = rpm;
    ctx.powertrain.wheel_rpm.rear_left = rpm;
    ctx.powertrain.wheel_rpm.rear_right = rpm;
    ctx.powertrain.refresh_speed();
    ctx
}

/// Healthy `VehicleContext` matching a valid digital twin (same values as `VehicleContext::default()`).
fn valid_twin_context() -> VehicleContext {
    VehicleContext::default()
}

#[test]
fn test_transition_and_output_extreme_operation_emits_both_signals() {
    let ctx = valid_twin_context();
    let now = Instant::now();
    let driving = transition(&FsmState::Idle, &FsmEvent::UpdateRpm(1200), &ctx, now);
    assert_eq!(driving.next_state, FsmState::Driving);

    let overspeed_ctx = ctx_with_rpm(5600);
    assert!(extreme_operation_active(
        overspeed_ctx.powertrain.wheel_rpm.front_left,
        overspeed_ctx.powertrain.speed_kph
    ));

    let warning = transition(&driving.next_state, &FsmEvent::UpdateRpm(5600), &overspeed_ctx, now);
    assert!(matches!(warning.next_state, FsmState::ExtremeOperationWarning(_)));

    let actions = output(&FsmState::Driving, &warning.next_state, &overspeed_ctx);
    assert!(actions.contains(&FsmAction::StartBuzzer));
    assert!(actions.contains(&FsmAction::LogWarning(
        SPEED_THRESHOLD_WARNING_MESSAGE.to_string()
    )));
    assert!(actions.contains(&FsmAction::LogWarning(
        EXTREME_OPERATION_WARNING_MESSAGE.to_string()
    )));
}

#[test]
fn test_transition_high_speed_alone_emits_speed_threshold_signal_only() {
    let now = Instant::now();
    let ctx = valid_twin_context();
    let driving = transition(&FsmState::Idle, &FsmEvent::UpdateRpm(1200), &ctx, now);

    let rpm = 3600u16;
    let fast_ctx = ctx_with_rpm(rpm);
    assert!(fast_ctx.powertrain.speed_kph > SPEED_EXTREME_OPERATION_THRESHOLD_KPH);
    assert!(fast_ctx.powertrain.wheel_rpm.front_left <= RPM_EXTREME_OPERATION_THRESHOLD);
    assert!(!extreme_operation_active(fast_ctx.powertrain.wheel_rpm.front_left, fast_ctx.powertrain.speed_kph));

    let warning = transition(&driving.next_state, &FsmEvent::UpdateRpm(rpm), &fast_ctx, now);
    assert!(matches!(warning.next_state, FsmState::ExtremeOperationWarning(_)));

    let actions = output(&FsmState::Driving, &warning.next_state, &fast_ctx);
    assert!(actions.contains(&FsmAction::LogWarning(
        SPEED_THRESHOLD_WARNING_MESSAGE.to_string()
    )));
    assert!(!actions.contains(&FsmAction::LogWarning(
        EXTREME_OPERATION_WARNING_MESSAGE.to_string()
    )));
}

#[test]
fn test_transition_standard_commute_flow() {
    let ctx = valid_twin_context();
    let now = Instant::now();
    let mut state = transition(&FsmState::Off, &FsmEvent::PowerOn, &ctx, now);
    assert_eq!(state.next_state, FsmState::Idle);

    let driving_ctx = ctx_with_rpm(1500);
    state = transition(&state.next_state, &FsmEvent::UpdateRpm(1500), &driving_ctx, now);
    assert_eq!(state.next_state, FsmState::Driving);

    // Stay below speed threshold (160 km/h): ~1300 RPM → ~148 km/h.
    state = transition(&state.next_state, &FsmEvent::UpdateRpm(1300), &ctx_with_rpm(1300), now);
    assert_eq!(state.next_state, FsmState::Driving);

    let stopped_ctx = ctx_with_rpm(0);
    state = transition(&state.next_state, &FsmEvent::UpdateRpm(0), &stopped_ctx, now);
    assert_eq!(state.next_state, FsmState::Idle);

    state = transition(&state.next_state, &FsmEvent::PowerOff, &stopped_ctx, now);
    assert_eq!(state.next_state, FsmState::Off);
}

#[test]
fn test_transition_illegal_shutdown_attempt() {
    let ctx = ctx_with_rpm(3000);
    let state = transition(&FsmState::Driving, &FsmEvent::PowerOff, &ctx, Instant::now());
    assert_eq!(state.next_state, FsmState::Driving);
}

#[test]
fn test_warning_recovery_requires_cooldown_and_cleared_thresholds() {
    let base = Instant::now();
    let warning = FsmState::ExtremeOperationWarning(base);
    let ctx = ctx_with_rpm(1000);

    let early = transition(
        &warning,
        &FsmEvent::TimerTick,
        &ctx,
        base + Duration::from_secs(2),
    );
    assert!(matches!(early.next_state, FsmState::ExtremeOperationWarning(_)));

    let still_extreme_ctx = ctx_with_rpm(6200);
    assert!(extreme_operation_active(
        still_extreme_ctx.powertrain.wheel_rpm.front_left,
        still_extreme_ctx.powertrain.speed_kph
    ));
    let still_warning = transition(
        &warning,
        &FsmEvent::TimerTick,
        &still_extreme_ctx,
        base + Duration::from_secs(6),
    );
    assert!(matches!(still_warning.next_state, FsmState::ExtremeOperationWarning(_)));

    let recovered = transition(
        &warning,
        &FsmEvent::TimerTick,
        &ctx,
        base + Duration::from_secs(6),
    );
    assert_eq!(recovered.next_state, FsmState::Driving);

    let actions = output(&warning, &recovered.next_state, &ctx);
    assert!(actions.contains(&FsmAction::StopBuzzer));
}

#[test]
fn test_warning_recovers_to_idle_when_stationary() {
    let base = Instant::now();
    let warning = FsmState::ExtremeOperationWarning(base);
    let ctx = ctx_with_rpm(0);

    let recovered = transition(
        &warning,
        &FsmEvent::TimerTick,
        &ctx,
        base + Duration::from_secs(6),
    );
    assert_eq!(recovered.next_state, FsmState::Idle);

    let actions = output(&warning, &recovered.next_state, &ctx);
    assert!(actions.contains(&FsmAction::StopBuzzer));
}
