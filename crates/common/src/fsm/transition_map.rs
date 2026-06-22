use super::machineries::{FsmAction, FsmEvent, FsmState, Operational};
use crate::vehicle_state::{HeadlampState, VehicleContext};
use crate::vehicle_physics::{
    extreme_operation_active, speed_threshold_exceeded, EXTREME_OPERATION_WARNING_MESSAGE,
    LUX_ON_THRESHOLD, RPM_DRIVING_THRESHOLD, RPM_STRESS_DURATION_THRESHOLD_SECS,
    SPEED_THRESHOLD_WARNING_MESSAGE,
};
use std::time::{Duration, Instant};

/// Operational mode transition table.
///
/// Human table:
/// - Off + PowerOn(healthy ctx) -> PreparingToStart  [initialises `ctx.pending_assemblies`]
/// - PreparingToStart + AssemblyZoneReady(zone_id) -> Idle (last pending) / PreparingToStart (more pending)
/// - PreparingToStart + anything else -> PreparingToStart (self-loop, no actions)
/// - Idle + PowerOff -> PreparingToStop  [initialises `ctx.pending_assemblies`]
/// - Idle + UpdateRpm(rpm > [`RPM_DRIVING_THRESHOLD`]) -> Driving
/// - Driving + derived ctx.powertrain.speed_kph == 0 -> Idle (any event, after kinematic refresh in `step`)
/// - Driving + speed > 160 km/h **or** (speed > 160 and RPM > 5500) -> ExtremeOperationWarning(now)
/// - ExtremeOperationWarning + TimerTick + cooldown + all signals cleared -> Driving/Idle
/// - PreparingToStop + AssemblyZoneReady(zone_id) -> Off (last pending) / PreparingToStop (more pending)
/// - PreparingToStop + anything else -> PreparingToStop (self-loop, no actions)
/// - Everything else -> stay in current state
///
/// Note: `Internal(AssembliesReady)` / `Internal(AssembliesStopped)` remain in the vocabulary
/// for observer/ledger use; they no longer appear on the primary startup/shutdown path.
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
            PowerOn if current_ctx.is_healthy() => {
                TransitionResult { next_state: PreparingToStart, note: None }
            }
            PowerOff => TransitionResult { next_state: Off, note: Some(TransitionNote::RejectedPowerOff) },
            _ => TransitionResult { next_state: Off, note: None },
        },
        PreparingToStart => match event {
            AssemblyZoneReady(zone_id) => {
                // `step` removes `zone_id` from `ctx.pending_assemblies` after this call.
                // Peek at what the set will look like post-removal to decide the next state.
                let remaining_after = current_ctx.pending_assemblies.len()
                    - usize::from(current_ctx.pending_assemblies.contains(zone_id));
                if remaining_after == 0 {
                    TransitionResult { next_state: Idle, note: None }
                } else {
                    TransitionResult { next_state: PreparingToStart, note: None }
                }
            }
            _ => TransitionResult { next_state: PreparingToStart, note: None },
        },
        Idle => match event {
            PowerOff => TransitionResult { next_state: PreparingToStop, note: None },
            UpdateRpm(rpm) if *rpm > RPM_DRIVING_THRESHOLD => {
                TransitionResult { next_state: Driving, note: None }
            }
            _ => TransitionResult { next_state: Idle, note: None },
        },
        Driving => match event {
            Internal(Operational::LightingUnsafe) => TransitionResult {
                next_state: DrivingDangerously,
                note: None,
            },
            PowerOff => TransitionResult { next_state: Driving, note: Some(TransitionNote::RejectedPowerOff) },
            _ if current_ctx.powertrain.is_operational_warning_active() => {
                TransitionResult { next_state: ExtremeOperationWarning(now), note: None }
            }
            _ if current_ctx.powertrain.is_stationary() => TransitionResult { next_state: Idle, note: None },
            _ => TransitionResult { next_state: Driving, note: None },
        },
        DrivingDangerously => match event {
            PowerOff => TransitionResult {
                next_state: DrivingDangerously,
                note: Some(TransitionNote::RejectedPowerOff),
            },
            _ if current_ctx.powertrain.is_stationary() => {
                TransitionResult { next_state: Idle, note: None }
            },
            _ if current_ctx.headlamp.state == HeadlampState::On => {
                TransitionResult { next_state: Driving, note: None }
            },
            _ if current_ctx.visibility.ambient_lux > LUX_ON_THRESHOLD => {
                TransitionResult { next_state: Driving, note: None }
            },
            _ => TransitionResult {
                next_state: DrivingDangerously,
                note: None,
            },
        },
        ExtremeOperationWarning(began_at) => match event {
            TimerTick if operational_warning_recovery_ready(*began_at, now, current_ctx) => {
                let next_state = if current_ctx.powertrain.is_stationary() { Idle } else { Driving };
                TransitionResult { next_state, note: None }
            }
            PowerOff => TransitionResult { next_state: ExtremeOperationWarning(*began_at), note: Some(TransitionNote::RejectedPowerOff) },
            _ => TransitionResult { next_state: ExtremeOperationWarning(*began_at), note: None },
        },
        PreparingToStop => match event {
            AssemblyZoneReady(zone_id) => {
                let remaining_after = current_ctx.pending_assemblies.len()
                    - usize::from(current_ctx.pending_assemblies.contains(zone_id));
                if remaining_after == 0 {
                    TransitionResult { next_state: Off, note: None }
                } else {
                    TransitionResult { next_state: PreparingToStop, note: None }
                }
            }
            _ => TransitionResult { next_state: PreparingToStop, note: None },
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
        (Off, PreparingToStart) => vec![StartAssemblies],
        (Idle, PreparingToStop) => vec![StopAssemblies],
        (Driving, DrivingDangerously) => vec![StartBuzzer],
        (DrivingDangerously, Driving) | (DrivingDangerously, Idle) => vec![StopBuzzer],
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
