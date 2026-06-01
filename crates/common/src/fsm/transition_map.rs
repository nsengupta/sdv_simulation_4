use super::machineries::{FsmAction, FsmEvent, FsmState};
use crate::vehicle_state::VehicleContext;
use crate::vehicle_physics::{
    extreme_operation_active, speed_threshold_exceeded, EXTREME_OPERATION_WARNING_MESSAGE,
    RPM_DRIVING_THRESHOLD, RPM_STRESS_DURATION_THRESHOLD_SECS, SPEED_THRESHOLD_WARNING_MESSAGE,
};
use std::time::{Duration, Instant};

/// Operational mode transition table (Off / Idle / Driving / ExtremeOperationWarning).
///
/// Human table:
/// - Off + PowerOn(healthy ctx) -> Idle
/// - Idle + PowerOff -> Off
/// - Idle + UpdateRpm(rpm > [`RPM_DRIVING_THRESHOLD`]) -> Driving
/// - Driving + derived ctx.powertrain.speed_kph == 0 -> Idle (any event, after kinematic refresh in `step`)
/// - Driving + speed > 160 km/h **or** (speed > 160 and RPM > 5500) -> ExtremeOperationWarning(now)
/// - ExtremeOperationWarning + TimerTick + cooldown + all signals cleared -> Driving/Idle
/// - Everything else -> stay in current state
///
/// # Purity
///
/// Pure decision function: no I/O, no logging. If a transition (or non-transition) is noteworthy,
/// a [`TransitionNote`] is returned so the caller can decide how to handle it.
#[derive(Debug, Clone, PartialEq)]
pub enum TransitionNote {
    /// A non-transition the caller may want to log as a warning.
    RejectedPowerOff,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransitionResult {
    pub next_state: FsmState,
    /// An optional note about a noteworthy non-transition.
    /// `None` means the outcome is routine / by design.
    pub note: Option<TransitionNote>,
}

pub fn transition(
    current_state: &FsmState,
    event: &FsmEvent,
    current_ctx: &VehicleContext,
    now: Instant,
) -> TransitionResult {
    use FsmEvent::*;
    use FsmState::*;

    match current_state {
        Off => match event {
            PowerOn if current_ctx.is_healthy() => TransitionResult { next_state: Idle, note: None },
            PowerOff => TransitionResult { next_state: Off, note: Some(TransitionNote::RejectedPowerOff) },
            _ => TransitionResult { next_state: Off, note: None },
        },
        Idle => match event {
            PowerOff => TransitionResult { next_state: Off, note: None },
            UpdateRpm(rpm) if *rpm > RPM_DRIVING_THRESHOLD => {
                TransitionResult { next_state: Driving, note: None }
            }
            _ => TransitionResult { next_state: Idle, note: None },
        },
        Driving => match event {
            PowerOff => TransitionResult { next_state: Driving, note: Some(TransitionNote::RejectedPowerOff) },
            _ if current_ctx.powertrain.is_operational_warning_active() => {
                TransitionResult { next_state: ExtremeOperationWarning(now), note: None }
            }
            _ if current_ctx.powertrain.is_stationary() => TransitionResult { next_state: Idle, note: None },
            _ => TransitionResult { next_state: Driving, note: None },
        },
        ExtremeOperationWarning(began_at) => match event {
            TimerTick if operational_warning_recovery_ready(*began_at, now, current_ctx) => {
                let next_state = if current_ctx.powertrain.is_stationary() { Idle } else { Driving };
                TransitionResult { next_state, note: None }
            }
            PowerOff => TransitionResult { next_state: ExtremeOperationWarning(*began_at), note: Some(TransitionNote::RejectedPowerOff) },
            _ => TransitionResult { next_state: ExtremeOperationWarning(*began_at), note: None },
        },
    }
}

fn operational_warning_recovery_ready(began_at: Instant, now: Instant, ctx: &VehicleContext) -> bool {
    let warning_age = now
        .checked_duration_since(began_at)
        .unwrap_or(Duration::ZERO);
    warning_age >= Duration::from_secs(RPM_STRESS_DURATION_THRESHOLD_SECS)
        && !ctx.powertrain.is_operational_warning_active()
}

pub fn output(old_state: &FsmState, new_state: &FsmState, ctx: &VehicleContext) -> Vec<FsmAction> {
    use FsmAction::*;
    use FsmState::*;

    match (old_state, new_state) {
        (Driving, ExtremeOperationWarning(_)) => {
            let mut actions = vec![StartBuzzer];
            if speed_threshold_exceeded(ctx.powertrain.speed_kph) {
                actions.push(LogWarning(SPEED_THRESHOLD_WARNING_MESSAGE.to_string()));
            }
            if extreme_operation_active(ctx.powertrain.primary_rpm(), ctx.powertrain.speed_kph) {
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
