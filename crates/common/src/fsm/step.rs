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
//! Boundary rule:
//! - Domain emits [`ActorModeHintFromDomain`]; runtime actor owns `ActorMode` and mailbox behavior.

use super::machineries::{
    FrontHeadlampIncompleteCause, FrontHeadlampSwitchDirection, FsmAction, FsmEvent, FsmState,
    LightingState, VehicleContext,
};
use crate::engine::op_strategy::transition_map::{output, transition};
use crate::vehicle_constants::{
    FRONT_HEADLAMP_OFF_ACK_WAIT, FRONT_HEADLAMP_ON_ACK_WAIT, LUX_OFF_THRESHOLD, LUX_ON_THRESHOLD,
};
use crate::front_headlamp_log;
use crate::vehicle_kinematics::refresh_context_speed;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorModeHintFromDomain {
    Normal,
    Transitioning,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DomainAction {
    StartBuzzer,
    StopBuzzer,
    PublishStateSync,
    LogWarning(String),
    RequestFrontHeadlampOn,
    RequestFrontHeadlampOff,
    EnterMode(ActorModeHintFromDomain),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransitionRecord {
    pub at: Instant,
    pub event: FsmEvent,
    pub old_state: FsmState,
    pub next_state: FsmState,
    pub old_ctx: VehicleContext,
    pub current_ctx: VehicleContext,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepResult {
    pub next_state: FsmState,
    pub modified_ctx: VehicleContext,
    pub actions: Vec<DomainAction>,
    pub transition_record: TransitionRecord,
}

pub fn step(
    current_state: &FsmState,
    current_ctx: &VehicleContext,
    event: &FsmEvent,
    now: Instant,
) -> StepResult {
    let mut modified_ctx = current_ctx.clone();
    match event {
        FsmEvent::UpdateRpm(rpm) => modified_ctx.rpm = *rpm,
        FsmEvent::UpdateAmbientLux(lux) => modified_ctx.ambient_lux = *lux,
        FsmEvent::FrontHeadlampOnAck => {
            modified_ctx.lighting_state = LightingState::On;
            modified_ctx.lighting_ack_pending_since = None;
        }
        FsmEvent::FrontHeadlampOffAck => {
            modified_ctx.lighting_state = LightingState::Off;
            modified_ctx.lighting_ack_pending_since = None;
        }
        FsmEvent::FrontHeadlampActuationIncomplete { .. }
        | FsmEvent::PowerOn
        | FsmEvent::PowerOff
        | FsmEvent::TimerTick => {}
    }

    refresh_context_speed(&mut modified_ctx);
    // Ignition off: kinematic speed is not meaningful; keep standstill for invariants.
    if *current_state == FsmState::Off {
        modified_ctx.speed = 0;
    }

    let next_state = transition(current_state, event, &modified_ctx, now);
    let mut actions: Vec<DomainAction> = output(current_state, &next_state, &modified_ctx)
        .into_iter()
        .filter_map(map_fsm_action)
        .collect();

    match (&current_ctx.lighting_state, event) {
        (LightingState::Off, FsmEvent::UpdateAmbientLux(lux)) if *lux <= LUX_ON_THRESHOLD => {
            modified_ctx.lighting_state = LightingState::OnRequested;
            modified_ctx.lighting_ack_pending_since = Some(now);
            actions.push(DomainAction::RequestFrontHeadlampOn);
        }
        (LightingState::On, FsmEvent::UpdateAmbientLux(lux)) if *lux >= LUX_OFF_THRESHOLD => {
            modified_ctx.lighting_state = LightingState::OffRequested;
            modified_ctx.lighting_ack_pending_since = Some(now);
            actions.push(DomainAction::RequestFrontHeadlampOff);
        }
        _ => {}
    }

    if matches!(event, FsmEvent::TimerTick) {
        try_front_headlamp_ack_timeout(&mut modified_ctx, now, &mut actions);
    }

    if let FsmEvent::FrontHeadlampActuationIncomplete { direction, cause } = event {
        try_recover_front_headlamp_incomplete(&mut modified_ctx, *direction, *cause, &mut actions);
    }

    if matches!(next_state, FsmState::Off) {
        modified_ctx.lighting_state = LightingState::Off;
        modified_ctx.lighting_ack_pending_since = None;
    }

    if matches!(next_state, FsmState::ExtremeOperationWarning(_)) {
        actions.push(DomainAction::EnterMode(ActorModeHintFromDomain::Transitioning));
    } else {
        actions.push(DomainAction::EnterMode(ActorModeHintFromDomain::Normal));
    }

    StepResult {
        next_state: next_state.clone(),
        modified_ctx: modified_ctx.clone(),
        actions,
        transition_record: TransitionRecord {
            at: now,
            event: event.clone(),
            old_state: current_state.clone(),
            next_state,
            old_ctx: current_ctx.clone(),
            current_ctx: modified_ctx,
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

fn ack_wait_elapsed(since: Instant, now: Instant, wait: Duration) -> bool {
    now.saturating_duration_since(since) >= wait
}

/// If we have been waiting for an ON/OFF ACK too long, revert to a safe lighting state and log.
fn try_front_headlamp_ack_timeout(
    modified_ctx: &mut VehicleContext,
    now: Instant,
    actions: &mut Vec<DomainAction>,
) {
    let Some(since) = modified_ctx.lighting_ack_pending_since else {
        return;
    };
    match modified_ctx.lighting_state {
        LightingState::OnRequested if ack_wait_elapsed(since, now, FRONT_HEADLAMP_ON_ACK_WAIT) => {
            try_recover_front_headlamp_incomplete(
                modified_ctx,
                FrontHeadlampSwitchDirection::On,
                FrontHeadlampIncompleteCause::TimedOut,
                actions,
            );
        }
        LightingState::OffRequested
            if ack_wait_elapsed(since, now, FRONT_HEADLAMP_OFF_ACK_WAIT) =>
        {
            try_recover_front_headlamp_incomplete(
                modified_ctx,
                FrontHeadlampSwitchDirection::Off,
                FrontHeadlampIncompleteCause::TimedOut,
                actions,
            );
        }
        _ => {}
    }
}

/// Recover from a failed front-headlamp command when `direction` matches the pending request.
fn try_recover_front_headlamp_incomplete(
    modified_ctx: &mut VehicleContext,
    direction: FrontHeadlampSwitchDirection,
    cause: FrontHeadlampIncompleteCause,
    actions: &mut Vec<DomainAction>,
) {
    let matches_pending = matches!(
        (modified_ctx.lighting_state, direction),
        (LightingState::OnRequested, FrontHeadlampSwitchDirection::On)
            | (LightingState::OffRequested, FrontHeadlampSwitchDirection::Off)
    );
    if !matches_pending {
        return;
    }

    modified_ctx.lighting_ack_pending_since = None;
    let warning = front_headlamp_log::alert_incomplete(direction, cause);
    match direction {
        FrontHeadlampSwitchDirection::On => {
            modified_ctx.lighting_state = LightingState::Off;
            actions.push(DomainAction::LogWarning(warning));
        }
        FrontHeadlampSwitchDirection::Off => {
            modified_ctx.lighting_state = LightingState::On;
            actions.push(DomainAction::LogWarning(warning));
        }
    }
}
