//! FSM Step Contract (authoritative vocabulary)
//!
//! `step(current_state, current_ctx, event, now) -> StepResult` runs the **operational FSM only**.
//! L1 zone updates run in [`crate::twin_runtime::zone_turn`] first; the actor and tests use
//! [`crate::twin_runtime::twin_turn`] for a full turn.
//!
//! Canonical input model:
//! - `current_ctx` is the snapshot **after** zone_turn.
//! - `modified_ctx` equals `current_ctx` on output (FSM does not mutate assemblies here).

use std::collections::BTreeSet;
use std::time::Instant;

use crate::vehicle_state::VehicleContext;
use super::machineries::{DomainAction, FsmAction, FsmEvent, FsmState, ZoneId};
use super::transition_map::{output, transition, TransitionNote};

#[derive(Debug, Clone, PartialEq)]
pub struct RawTransitionRecord {
    pub at: Instant,
    pub event: FsmEvent,
    pub old_state: FsmState,
    pub next_state: FsmState,
    pub old_ctx: VehicleContext,
    pub current_ctx: VehicleContext,
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

    let transition_result = transition(current_state, event, &modified_ctx, now);
    let next_state = transition_result.next_state.clone();

    // FSM-owned context mutations for `pending_assemblies`.
    // These do NOT touch assembly state (headlamp, powertrain, etc.) — only the
    // lifecycle bookkeeping field that the transition table uses for counting.
    match event {
        FsmEvent::PowerOn
            if *current_state == FsmState::Off && next_state == FsmState::PreparingToStart =>
        {
            // Phase 7: both managed assemblies must report ZoneReady before the FSM
            // exits PreparingToStart.  Keep in sync with MANAGED_ASSEMBLIES.
            modified_ctx.pending_assemblies =
                BTreeSet::from([ZoneId::Headlamp, ZoneId::Wiper]);
        }
        FsmEvent::PowerOff
            if *current_state == FsmState::Idle && next_state == FsmState::PreparingToStop =>
        {
            modified_ctx.pending_assemblies =
                BTreeSet::from([ZoneId::Headlamp, ZoneId::Wiper]);
        }
        FsmEvent::AssemblyZoneReady(zone_id) => {
            modified_ctx.pending_assemblies.remove(zone_id);
        }
        _ => {}
    }
    let mut actions: Vec<DomainAction> = output(current_state, &next_state, &modified_ctx)
        .into_iter()
        .filter_map(map_fsm_action)
        .collect();

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

    // Ledger record: drop internal coordination signals (StartAssemblies, StopAssemblies).
    // These are control hints consumed by the actor, not domain intents.
    // `AssemblyZoneReady` is also excluded — it is a zone-reply correlation signal,
    // not a domain intent, and its effect is already captured by the resulting state change.
    let recorded_actions: Vec<DomainAction> = actions
        .iter()
        .filter(|action| !matches!(
            action,
            DomainAction::StartAssemblies | DomainAction::StopAssemblies
        ))
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
        FsmAction::StartAssemblies => Some(DomainAction::StartAssemblies),
        FsmAction::StopAssemblies => Some(DomainAction::StopAssemblies),
        FsmAction::None => None,
    }
}
