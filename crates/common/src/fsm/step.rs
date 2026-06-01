//! FSM Step Contract (authoritative vocabulary)
//!
//! This module defines the single state-transition boundary:
//! `step(current_state, current_ctx, event, now) -> StepResult`.
//!
//! Canonical input model:
//! - `event` payload is canonical input.
//! - `current_ctx` is the materialized snapshot before processing this event.
//! - `modified_ctx` is produced by this step; callers must not mutate context outside `step`.
//!
//! Output model:
//! - `next_state`: state after this event.
//! - `modified_ctx`: context after this event.
//! - `actions`: pure domain intents (no hardware/network calls).
//! - `transition_record`: audit snapshot for observability/replay.
//!
//! Orchestration only:
//! - Per-assembly data mutation + derivation lives on the assemblies
//!   (`crate::vehicle_state`); `step` decides *when* to invoke them, runs the
//!   operational FSM, and maps results into [`DomainAction`].

use crate::vehicle_state::VehicleContext;
use super::machineries::{ActorModeHintFromDomain, DomainAction, FsmAction, FsmEvent, FsmState};
use super::transition_map::{output, transition, TransitionNote};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq)]
pub struct RawTransitionRecord {
    pub at: Instant,
    pub event: FsmEvent,
    pub old_state: FsmState,
    pub next_state: FsmState,
    pub old_ctx: VehicleContext,
    pub current_ctx: VehicleContext,
    /// Domain actions this step *intended* (the observability/replay projection).
    ///
    /// These are emitted intents from the pure step (deterministic) — **not** execution
    /// outcomes (ACK / timeout / failure are separate facts). [`DomainAction::EnterMode`]
    /// is deliberately excluded: it is a runtime control hint for the actor, not a domain
    /// intent. This is an owned, filtered clone of [`StepResult::actions`]; see WI-1 in
    /// `docs/design-notes-runtime-observation.md` for why a clone (not a borrow or `Arc`).
    pub actions: Vec<DomainAction>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepResult {
    pub next_state: FsmState,
    pub modified_ctx: VehicleContext,
    pub actions: Vec<DomainAction>,
    pub transition_record: RawTransitionRecord,
}

pub fn step(
    current_state: &FsmState,
    current_ctx: &VehicleContext,
    event: &FsmEvent,
    now: Instant,
) -> StepResult {
    let mut modified_ctx = current_ctx.clone();

    // 1. Dispatch the event to the owning assembly.
    match event {
        FsmEvent::UpdateRpm(rpm) => modified_ctx.powertrain.apply_rpm(*rpm),
        FsmEvent::UpdateAmbientLux(lux) => modified_ctx.visibility.apply_lux(*lux),
        FsmEvent::FrontHeadlampOnAck => modified_ctx.headlamp.apply_on_ack(),
        FsmEvent::FrontHeadlampOffAck => modified_ctx.headlamp.apply_off_ack(),
        FsmEvent::FrontHeadlampActuationIncomplete { .. }
        | FsmEvent::PowerOn
        | FsmEvent::PowerOff
        | FsmEvent::TimerTick => {}
    }

    // 2. Powertrain derivation; ignition off holds standstill for invariants.
    modified_ctx.powertrain.refresh_speed();
    if *current_state == FsmState::Off {
        modified_ctx.powertrain.freeze_standstill();
    }

    // 3. Operational FSM.
    let transition_result = transition(current_state, event, &modified_ctx, now);
    let next_state = transition_result.next_state.clone();
    let mut actions: Vec<DomainAction> = output(current_state, &next_state, &modified_ctx)
        .into_iter()
        .filter_map(map_fsm_action)
        .collect();

    // Emit domain action for any noteworthy non-transition from the strategy layer.
    if let Some(note) = &transition_result.note {
        match note {
            TransitionNote::RejectedPowerOff => {
                actions.push(DomainAction::LogWarning(format!(
                    "[REJECTED]: PowerOff is invalid while in state {:?}",
                    current_state
                )));
            }
        }
    }

    // 4. Headlamp side-effects (logic owned by the assembly; orchestrated here).
    if let FsmEvent::UpdateAmbientLux(lux) = event {
        modified_ctx
            .headlamp
            .evaluate_lux(current_ctx.headlamp.state, *lux, now, &mut actions);
    }
    if matches!(event, FsmEvent::TimerTick) {
        modified_ctx.headlamp.on_timer_tick(now, &mut actions);
    }
    if let FsmEvent::FrontHeadlampActuationIncomplete { direction, cause } = event {
        modified_ctx
            .headlamp
            .on_incomplete(*direction, *cause, &mut actions);
    }
    if matches!(next_state, FsmState::Off) {
        modified_ctx.headlamp.reset_for_ignition_off();
    }

    // 5. Actor-mode hint from the resulting operational state.
    if matches!(next_state, FsmState::ExtremeOperationWarning(_)) {
        actions.push(DomainAction::EnterMode(ActorModeHintFromDomain::Transitioning));
    } else {
        actions.push(DomainAction::EnterMode(ActorModeHintFromDomain::Normal));
    }

    // Ledger projection of the emitted actions (WI-1). `StepResult::actions` stays the
    // unfiltered execution feed (the actor needs `EnterMode` to set its mode); the record
    // gets an owned, filtered clone. The clone is trivial (a step emits 0–3 actions) and a
    // borrow/`Arc` is intentionally avoided — see docs/design-notes-runtime-observation.md.
    let recorded_actions: Vec<DomainAction> = actions
        .iter()
        .filter(|action| !matches!(action, DomainAction::EnterMode(_)))
        .cloned()
        .collect();

    StepResult {
        next_state: next_state.clone(),
        modified_ctx: modified_ctx.clone(),
        actions,
        transition_record: RawTransitionRecord {
            at: now,
            event: event.clone(),
            old_state: current_state.clone(),
            next_state,
            old_ctx: current_ctx.clone(),
            current_ctx: modified_ctx,
            actions: recorded_actions,
        },
    }
}

fn map_fsm_action(action: FsmAction) -> Option<DomainAction> {
    match action {
        FsmAction::StartBuzzer => Some(DomainAction::StartBuzzer),
        FsmAction::StopBuzzer => Some(DomainAction::StopBuzzer),
        FsmAction::PublishStateSync => Some(DomainAction::PublishStateSync),
        FsmAction::LogWarning(msg) => Some(DomainAction::LogWarning(msg)),
        FsmAction::None => None,
    }
}
