use crate::fsm::machineries::{FsmAction, FsmEvent, FsmState, VehicleContext};
use crate::vehicle_constants::{
    extreme_operation_active, operational_warning_active, speed_threshold_exceeded,
    EXTREME_OPERATION_WARNING_MESSAGE, RPM_STRESS_DURATION_THRESHOLD_SECS,
    SPEED_THRESHOLD_WARNING_MESSAGE,
};
use std::time::{Duration, Instant};

/// Transition spec (runtime source of truth).
///
/// Human table:
/// - Off + PowerOn(healthy ctx) -> Idle
/// - Idle + PowerOff -> Off
/// - Idle + UpdateRpm(rpm > 1000) -> Driving
/// - Driving + derived ctx.speed == 0 -> Idle (any event, after kinematic refresh in `step`)
/// - Driving + speed > 160 km/h **or** (speed > 160 and RPM > 5500) -> ExtremeOperationWarning(now)
/// - ExtremeOperationWarning + TimerTick + cooldown + all signals cleared -> Driving/Idle
/// - Everything else -> stay in current state
pub fn transition(
    current_state: &FsmState,
    event: &FsmEvent,
    current_ctx: &VehicleContext,
    now: Instant,
) -> FsmState {
    use FsmEvent::*;
    use FsmState::*;

    match current_state {
        Off => match event {
            PowerOn if current_ctx.is_healthy() => Idle,
            PowerOff => {
                eprintln!("[REJECTED]: PowerOff is invalid while in state {:?}", current_state);
                Off
            }
            _ => Off,
        },
        Idle => match event {
            PowerOff => Off,
            UpdateRpm(rpm) if *rpm > 1000 => Driving,
            _ => Idle,
        },
        Driving => match event {
            PowerOff => {
                eprintln!("[REJECTED]: PowerOff is invalid while in state {:?}", current_state);
                Driving
            }
            _ if operational_warning_active(current_ctx.rpm, current_ctx.speed) => {
                ExtremeOperationWarning(now)
            }
            _ if current_ctx.speed == 0 => Idle,
            _ => Driving,
        },
        ExtremeOperationWarning(began_at) => match event {
            TimerTick if operational_warning_recovery_ready(*began_at, now, current_ctx) => {
                if current_ctx.speed == 0 {
                    Idle
                } else {
                    Driving
                }
            }
            PowerOff => {
                eprintln!("[REJECTED]: PowerOff is invalid while in state {:?}", current_state);
                ExtremeOperationWarning(*began_at)
            }
            _ => ExtremeOperationWarning(*began_at),
        },
    }
}

fn operational_warning_recovery_ready(began_at: Instant, now: Instant, ctx: &VehicleContext) -> bool {
    let warning_age = now
        .checked_duration_since(began_at)
        .unwrap_or(Duration::ZERO);
    warning_age >= Duration::from_secs(RPM_STRESS_DURATION_THRESHOLD_SECS)
        && !operational_warning_active(ctx.rpm, ctx.speed)
}

pub fn output(old_state: &FsmState, new_state: &FsmState, ctx: &VehicleContext) -> Vec<FsmAction> {
    use FsmAction::*;
    use FsmState::*;

    match (old_state, new_state) {
        (Driving, ExtremeOperationWarning(_)) => {
            let mut actions = vec![StartBuzzer];
            if speed_threshold_exceeded(ctx.speed) {
                actions.push(LogWarning(SPEED_THRESHOLD_WARNING_MESSAGE.to_string()));
            }
            if extreme_operation_active(ctx.rpm, ctx.speed) {
                actions.push(LogWarning(EXTREME_OPERATION_WARNING_MESSAGE.to_string()));
            }
            actions
        }
        (ExtremeOperationWarning(_), Driving) | (ExtremeOperationWarning(_), Idle) => {
            vec![StopBuzzer]
        }
        (old, new) if old != new => vec![PublishStateSync],
        _ => vec![],
    }
}
