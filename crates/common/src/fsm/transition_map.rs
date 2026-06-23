use std::collections::BTreeSet;
use super::machineries::{ALL_ASSEMBLIES, AssemblyId, FsmAction, FsmEvent, FsmState, Operational};
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
/// - Off + PowerOn(healthy ctx) -> PreparingToStart({all assemblies})
/// - PreparingToStart({a, ...}) + AssemblyZoneReady(a) -> PreparingToStart({...}) or Idle (when set empties)
/// - PreparingToStart + anything else -> PreparingToStart (self-loop, set unchanged)
/// - Idle + PowerOff -> PreparingToStop({all assemblies})
/// - Idle + UpdateRpm(rpm > [`RPM_DRIVING_THRESHOLD`]) -> Driving
/// - Driving + derived ctx.powertrain.speed_kph == 0 -> Idle (any event, after kinematic refresh in `step`)
/// - Driving + speed > 160 km/h **or** (speed > 160 and RPM > 5500) -> ExtremeOperationWarning(now)
/// - ExtremeOperationWarning + TimerTick + cooldown + all signals cleared -> Driving/Idle
/// - PreparingToStop({a, ...}) + AssemblyZoneReady(a) -> PreparingToStop({...}) or Off (when set empties)
/// - PreparingToStop + anything else -> PreparingToStop (self-loop, set unchanged)
/// - Everything else -> stay in current state
///
/// `VehicleContext` carries no separate `remaining_assemblies` field; the `BTreeSet` embedded
/// in `PreparingToStart` / `PreparingToStop` is the sole authoritative countdown.
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
                TransitionResult {
                    next_state: PreparingToStart(ALL_ASSEMBLIES.iter().copied().collect()),
                    note: None,
                }
            }
            PowerOff => TransitionResult { next_state: Off, note: Some(TransitionNote::RejectedPowerOff) },
            _ => TransitionResult { next_state: Off, note: None },
        },
        PreparingToStart(remaining) => match event {
            AssemblyZoneReady(assembly_id) => {
                let new_remaining: BTreeSet<AssemblyId> =
                    remaining.iter().copied().filter(|a| a != assembly_id).collect();
                if new_remaining.is_empty() {
                    TransitionResult { next_state: Idle, note: None }
                } else {
                    TransitionResult { next_state: PreparingToStart(new_remaining), note: None }
                }
            }
            _ => TransitionResult { next_state: PreparingToStart(remaining.clone()), note: None },
        },
        Idle => match event {
            PowerOff => TransitionResult {
                next_state: PreparingToStop(ALL_ASSEMBLIES.iter().copied().collect()),
                note: None,
            },
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
        PreparingToStop(remaining) => match event {
            AssemblyZoneReady(assembly_id) => {
                let new_remaining: BTreeSet<AssemblyId> =
                    remaining.iter().copied().filter(|a| a != assembly_id).collect();
                if new_remaining.is_empty() {
                    TransitionResult { next_state: Off, note: None }
                } else {
                    TransitionResult { next_state: PreparingToStop(new_remaining), note: None }
                }
            }
            _ => TransitionResult { next_state: PreparingToStop(remaining.clone()), note: None },
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
        (Off, PreparingToStart(_)) => vec![StartAssemblies(ALL_ASSEMBLIES.to_vec())],
        (Idle, PreparingToStop(_)) => vec![StopAssemblies(ALL_ASSEMBLIES.to_vec())],
        // Intra-mode steps: an assembly acknowledged but peers are still pending.
        // The FSM is still in the same mode; no domain event to publish.
        (PreparingToStart(_), PreparingToStart(_)) => vec![],
        (PreparingToStop(_), PreparingToStop(_)) => vec![],
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
