//! L4 turn: [`zone_turn`] then L2 [`step`], merge zone outcomes into [`DomainAction`].
//!
//! Actor commit uses [`commit_resolved_turn`] → [`run_to_quiescence`]. See `docs/adr-007-fsm-quiescence-and-cut.md`.

use std::time::Instant;

use crate::fsm::{step, DomainAction, FsmEvent, FsmState, StepResult};
use crate::twin_runtime::detectors::detect_internal_after_hop;
use crate::twin_runtime::outcome_map::headlamp_outcomes_to_domain_actions;
use crate::twin_runtime::zone_replies::ZoneReplies;
use crate::twin_runtime::zone_turn::zone_turn;
use crate::vehicle_state::{HeadlampMessage, VehicleContext};

const MAX_QUIESCENCE_HOPS: usize = 8;

/// One ledger row inside a quiescent turn.
#[derive(Debug, Clone, PartialEq)]
pub struct HopRecord {
    pub event: FsmEvent,
    pub result: StepResult,
}

/// Full turn after 0+ internal hops (ADR-7).
#[derive(Debug, Clone, PartialEq)]
pub struct QuiescentResult {
    pub hops: Vec<HopRecord>,
}

impl QuiescentResult {
    pub fn final_step(&self) -> &StepResult {
        self.hops
            .last()
            .map(|h| &h.result)
            .expect("quiescence requires at least one hop")
    }

    pub fn merged_actions(&self) -> Vec<DomainAction> {
        self.hops
            .iter()
            .flat_map(|h| h.result.actions.clone())
            .collect()
    }
}

/// Full deterministic turn (pure tests — zones applied locally via [`ZoneReplies::simulate_locally`]).
pub fn twin_turn(
    current_state: &FsmState,
    current_ctx: &VehicleContext,
    event: &FsmEvent,
    now: Instant,
) -> StepResult {
    apply_external_hop(
        current_state,
        current_ctx,
        event,
        now,
        &ZoneReplies::simulate_locally(),
    )
}

/// Inputs complete after zone tell-back(s) — moved into [`commit_resolved_turn`], not stored on actor.
#[must_use]
#[derive(Debug, Clone)]
pub struct ResolvedTurn {
    pub ingress: FsmEvent,
    pub now: Instant,
    pub zone_replies: ZoneReplies,
}

/// Mandatory quiescence at commit boundary (ADR-7).
pub fn commit_resolved_turn(
    initial_state: &FsmState,
    initial_ctx: &VehicleContext,
    resolved: ResolvedTurn,
) -> QuiescentResult {
    run_to_quiescence(
        initial_state,
        initial_ctx,
        &resolved.ingress,
        resolved.now,
        &resolved.zone_replies,
    )
}

/// Mandatory quiescence loop (ADR-7): external ingress + detector-synthesized internal hops.
pub fn run_to_quiescence(
    initial_state: &FsmState,
    initial_ctx: &VehicleContext,
    ingress: &FsmEvent,
    now: Instant,
    zone_replies: &ZoneReplies,
) -> QuiescentResult {
    let mut queue = vec![ingress.clone()];
    let mut state = initial_state.clone();
    let mut ctx = initial_ctx.clone();
    let mut hops = Vec::new();

    while let Some(event) = queue.first().cloned() {
        if hops.len() >= MAX_QUIESCENCE_HOPS {
            break;
        }
        queue.remove(0);

        let is_first = hops.is_empty();
        let hop_replies = if is_first {
            zone_replies
        } else {
            &ZoneReplies::default()
        };

        let result = apply_single_hop(&state, &ctx, &event, now, hop_replies);

        if let Some(internal) = detect_internal_after_hop(&result.next_state, &result.modified_ctx) {
            queue.push(internal);
        }

        state = result.next_state.clone();
        ctx = result.modified_ctx.clone();
        hops.push(HopRecord { event, result });
    }

    QuiescentResult { hops }
}

/// Whether this event **enters** [`FsmState::Off`] from a powered state after zone demux.
///
/// Returns `false` unconditionally for [`FsmEvent::AssemblyZoneReady`]: the headlamp zone
/// reply was already embedded in the assembly barrier (`BecomeOff` → `HeadlampState::Off`),
/// so no additional ignition-off reset tell is needed.  This keeps the `IgnitionOffReset`
/// barrier phase unreachable after Phase 5.
pub(crate) fn fsm_step_lands_off(
    current_state: &FsmState,
    current_ctx: &VehicleContext,
    event: &FsmEvent,
    now: Instant,
    zone_replies: &ZoneReplies,
) -> bool {
    if *current_state == FsmState::Off {
        return false;
    }
    // Assembly zone replies carry an embedded BecomeOff that already resets the headlamp;
    // no ignition-off reset tell is required.
    if matches!(event, FsmEvent::AssemblyZoneReady(_)) {
        return false;
    }
    if matches!(event, FsmEvent::Internal(_)) {
        return matches!(
            step(current_state, current_ctx, event, now).next_state,
            FsmState::Off
        );
    }
    let zone = zone_turn(current_ctx, event, current_state, now, zone_replies);
    matches!(
        step(current_state, &zone.ctx, event, now).next_state,
        FsmState::Off
    )
}

fn apply_single_hop(
    current_state: &FsmState,
    current_ctx: &VehicleContext,
    event: &FsmEvent,
    now: Instant,
    zone_replies: &ZoneReplies,
) -> StepResult {
    if matches!(event, FsmEvent::Internal(_)) {
        apply_internal_hop(current_state, current_ctx, event, now)
    } else {
        apply_external_hop(current_state, current_ctx, event, now, zone_replies)
    }
}

fn apply_internal_hop(
    current_state: &FsmState,
    current_ctx: &VehicleContext,
    event: &FsmEvent,
    now: Instant,
) -> StepResult {
    step(current_state, current_ctx, event, now)
}

/// One external FSM hop: zone merge (tell-back or local L1) then L2 [`step`].
fn apply_external_hop(
    current_state: &FsmState,
    current_ctx: &VehicleContext,
    event: &FsmEvent,
    now: Instant,
    zone_replies: &ZoneReplies,
) -> StepResult {
    let zone = zone_turn(current_ctx, event, current_state, now, zone_replies);
    let mut result = step(current_state, &zone.ctx, event, now);

    let mut headlamp_outcomes = zone.headlamp_outcomes;
    if matches!(result.next_state, FsmState::Off) {
        let zone_reply = zone_replies.headlamp.ignition_off_reset.clone().unwrap_or_else(|| {
            result.modified_ctx.headlamp.on_receiving_message(
                HeadlampMessage::ResetForIgnitionOff,
                now,
            )
        });
        result.modified_ctx.headlamp = zone_reply.ctx;
        headlamp_outcomes.extend(zone_reply.outcomes);
    }

    let zone_actions = headlamp_outcomes_to_domain_actions(headlamp_outcomes);
    result.actions = zone_actions
        .into_iter()
        .chain(result.actions)
        .collect();

    let recorded_actions: Vec<DomainAction> = result
        .actions
        .iter()
        .filter(|action| !matches!(action, DomainAction::EnterMode(_)))
        .cloned()
        .collect();

    result.transition_record.actions = recorded_actions;
    result.transition_record.current_ctx = result.modified_ctx.clone();

    result
}
