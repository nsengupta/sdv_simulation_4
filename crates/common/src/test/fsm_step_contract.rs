//! Unit tests for the FSM step contract (`step`).

use crate::digital_twin::DigitalTwinCar;
use crate::fsm::{step, DomainAction, FsmEvent, FsmState, VehicleContext};
use crate::vehicle_constants::{
    EXTREME_OPERATION_WARNING_MESSAGE, SPEED_THRESHOLD_WARNING_MESSAGE,
};
use std::time::{Duration, Instant};

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
fn test_step_derive_ctx_and_warning_flow() {
    let mut current_ctx = valid_twin_context();
    let mut current_state = FsmState::Idle;

    let warmup = step(
        &current_state,
        &current_ctx,
        &FsmEvent::UpdateRpm(1200),
        Instant::now(),
    );
    assert_eq!(warmup.next_state, FsmState::Driving);
    assert_eq!(warmup.modified_ctx.rpm, 1200);

    current_state = warmup.next_state;
    current_ctx = warmup.modified_ctx;

    let warning = step(
        &current_state,
        &current_ctx,
        &FsmEvent::UpdateRpm(5600),
        Instant::now(),
    );
    assert_eq!(warning.modified_ctx.rpm, 5600);
    assert!(matches!(
        warning.next_state,
        FsmState::ExtremeOperationWarning(_)
    ));
    assert!(warning.actions.contains(&DomainAction::StartBuzzer));
    assert!(warning.actions.contains(&DomainAction::LogWarning(
        SPEED_THRESHOLD_WARNING_MESSAGE.to_string()
    )));
    assert!(warning.actions.contains(&DomainAction::LogWarning(
        EXTREME_OPERATION_WARNING_MESSAGE.to_string()
    )));
}

#[test]
fn test_step_high_speed_below_rpm_threshold_still_warns_on_speed() {
    let current_ctx = valid_twin_context();
    let current_state = FsmState::Driving;

    let result = step(
        &current_state,
        &current_ctx,
        &FsmEvent::UpdateRpm(3600),
        Instant::now(),
    );
    assert_eq!(result.modified_ctx.rpm, 3600);
    assert!(matches!(
        result.next_state,
        FsmState::ExtremeOperationWarning(_)
    ));
    assert!(result.actions.contains(&DomainAction::LogWarning(
        SPEED_THRESHOLD_WARNING_MESSAGE.to_string()
    )));
    assert!(!result
        .actions
        .contains(&DomainAction::LogWarning(EXTREME_OPERATION_WARNING_MESSAGE.to_string())));
}

#[test]
fn test_step_standard_commute_flow() {
    let mut car = DigitalTwinCar {
        identity: "NASHIK-VC-001".to_string(),
        current_state: FsmState::Off,
        context: valid_twin_context(),
    };

    let sequence = vec![
        (FsmEvent::PowerOn, FsmState::Idle),
        (FsmEvent::UpdateRpm(1500), FsmState::Driving),
        (FsmEvent::UpdateRpm(1300), FsmState::Driving),
        (FsmEvent::UpdateRpm(0), FsmState::Idle),
        (FsmEvent::PowerOff, FsmState::Off),
    ];

    for (event, expected_state) in sequence {
        let result = step(&car.current_state, &car.context, &event, Instant::now());
        car.current_state = result.next_state;
        car.context = result.modified_ctx;
        assert_eq!(car.current_state, expected_state, "event={event:?}");
    }
}

#[test]
fn test_step_warning_recovery_on_tick_uses_passed_time() {
    let base = Instant::now();
    let ctx = {
        let mut c = valid_twin_context();
        c.rpm = 1000;
        crate::vehicle_kinematics::refresh_context_speed(&mut c);
        c
    };

    let warning_state = FsmState::ExtremeOperationWarning(base);

    let early = step(
        &warning_state,
        &ctx,
        &FsmEvent::TimerTick,
        base + Duration::from_secs(2),
    );
    assert!(matches!(
        early.next_state,
        FsmState::ExtremeOperationWarning(_)
    ));

    let recovered = step(
        &warning_state,
        &ctx,
        &FsmEvent::TimerTick,
        base + Duration::from_secs(6),
    );
    assert_eq!(recovered.next_state, FsmState::Driving);
    assert!(recovered.actions.contains(&DomainAction::StopBuzzer));
}
