//! Unit tests for the FSM spec (`transition` / `output`).

use crate::fsm::{output, transition, FsmAction, FsmEvent, FsmState, VehicleContext};
use crate::vehicle_constants::{
    extreme_operation_active, EXTREME_OPERATION_WARNING_MESSAGE, RPM_EXTREME_OPERATION_THRESHOLD,
    SPEED_EXTREME_OPERATION_THRESHOLD_KPH, SPEED_THRESHOLD_WARNING_MESSAGE,
};
use crate::vehicle_kinematics::refresh_context_speed;
use std::time::{Duration, Instant};

fn ctx_with_rpm(rpm: u16) -> VehicleContext {
    let mut ctx = VehicleContext {
        rpm,
        ..VehicleContext::default()
    };
    refresh_context_speed(&mut ctx);
    ctx
}

/// Healthy `VehicleContext` matching a valid digital twin (same values as `VehicleContext::default()`).
fn valid_twin_context() -> VehicleContext {
    VehicleContext {
        rpm: 0,
        speed: 0,
        fuel_level: 85,
        oil_pressure: 30,
        tyre_pressure_ok: true,
        ambient_lux: 100,
        lighting_state: crate::fsm::LightingState::Off,
        lighting_ack_pending_since: None,
    }
}

#[test]
fn test_transition_and_output_extreme_operation_emits_both_signals() {
    let ctx = valid_twin_context();
    let now = Instant::now();
    let driving = transition(&FsmState::Idle, &FsmEvent::UpdateRpm(1200), &ctx, now);
    assert_eq!(driving, FsmState::Driving);

    let overspeed_ctx = ctx_with_rpm(5600);
    assert!(extreme_operation_active(
        overspeed_ctx.rpm,
        overspeed_ctx.speed
    ));

    let warning = transition(&driving, &FsmEvent::UpdateRpm(5600), &overspeed_ctx, now);
    assert!(matches!(warning, FsmState::ExtremeOperationWarning(_)));

    let actions = output(&FsmState::Driving, &warning, &overspeed_ctx);
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
    assert!(fast_ctx.speed > SPEED_EXTREME_OPERATION_THRESHOLD_KPH);
    assert!(fast_ctx.rpm <= RPM_EXTREME_OPERATION_THRESHOLD);
    assert!(!extreme_operation_active(fast_ctx.rpm, fast_ctx.speed));

    let warning = transition(&driving, &FsmEvent::UpdateRpm(rpm), &fast_ctx, now);
    assert!(matches!(warning, FsmState::ExtremeOperationWarning(_)));

    let actions = output(&FsmState::Driving, &warning, &fast_ctx);
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
    assert_eq!(state, FsmState::Idle);

    let driving_ctx = ctx_with_rpm(1500);
    state = transition(&state, &FsmEvent::UpdateRpm(1500), &driving_ctx, now);
    assert_eq!(state, FsmState::Driving);

    // Stay below speed threshold (160 km/h): ~1300 RPM → ~148 km/h.
    state = transition(&state, &FsmEvent::UpdateRpm(1300), &ctx_with_rpm(1300), now);
    assert_eq!(state, FsmState::Driving);

    let stopped_ctx = ctx_with_rpm(0);
    state = transition(&state, &FsmEvent::UpdateRpm(0), &stopped_ctx, now);
    assert_eq!(state, FsmState::Idle);

    state = transition(&state, &FsmEvent::PowerOff, &stopped_ctx, now);
    assert_eq!(state, FsmState::Off);
}

#[test]
fn test_transition_illegal_shutdown_attempt() {
    let ctx = ctx_with_rpm(3000);
    let state = transition(&FsmState::Driving, &FsmEvent::PowerOff, &ctx, Instant::now());
    assert_eq!(state, FsmState::Driving);
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
    assert!(matches!(early, FsmState::ExtremeOperationWarning(_)));

    let still_extreme_ctx = ctx_with_rpm(6200);
    assert!(extreme_operation_active(
        still_extreme_ctx.rpm,
        still_extreme_ctx.speed
    ));
    let still_warning = transition(
        &warning,
        &FsmEvent::TimerTick,
        &still_extreme_ctx,
        base + Duration::from_secs(6),
    );
    assert!(matches!(still_warning, FsmState::ExtremeOperationWarning(_)));

    let recovered = transition(
        &warning,
        &FsmEvent::TimerTick,
        &ctx,
        base + Duration::from_secs(6),
    );
    assert_eq!(recovered, FsmState::Driving);

    let actions = output(&warning, &recovered, &ctx);
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
    assert_eq!(recovered, FsmState::Idle);

    let actions = output(&warning, &recovered, &ctx);
    assert!(actions.contains(&FsmAction::StopBuzzer));
}
