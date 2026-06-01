//! Behavioral contract tests for lighting sub-state behavior.

use crate::fsm::{
    step, FrontHeadlampIncompleteCause, FrontHeadlampSwitchDirection, DomainAction, FsmEvent,
    FsmState, LightingState,
};
use crate::vehicle_state::VehicleContext;
use crate::vehicle_physics::{
    FRONT_HEADLAMP_OFF_ACK_WAIT, FRONT_HEADLAMP_ON_ACK_WAIT, LUX_OFF_THRESHOLD, LUX_ON_THRESHOLD,
};
use std::time::Instant;

fn valid_twin_context() -> VehicleContext {
    VehicleContext::default()
}

fn ctx_with_headlamp_state(state: LightingState) -> VehicleContext {
    let mut ctx = valid_twin_context();
    ctx.headlamp.state = state;
    ctx
}

fn ctx_with_pending_headlamp(state: LightingState, since: Instant, ambient_lux: u16) -> VehicleContext {
    let mut ctx = valid_twin_context();
    ctx.headlamp.state = state;
    ctx.headlamp.ack_pending_since = Some(since);
    ctx.visibility.ambient_lux = ambient_lux;
    ctx
}

#[test]
fn given_lights_off_when_lux_below_on_threshold_then_requests_front_headlamp_on() {
    let current_state = FsmState::Idle;
    let current_ctx = valid_twin_context();

    // Dim side of emulator jitter band (~815) is below LUX_ON_THRESHOLD (840).
    let result = step(
        &current_state,
        &current_ctx,
        &FsmEvent::UpdateAmbientLux(830),
        Instant::now(),
    );

    assert!(result
        .actions
        .contains(&DomainAction::RequestFrontHeadlampOn));
}

#[test]
fn given_on_requested_when_ack_on_then_no_duplicate_on_request_emitted() {
    let current_state = FsmState::Idle;
    let mut current_ctx = ctx_with_headlamp_state(LightingState::OnRequested);
    current_ctx.visibility.ambient_lux = 20;

    let result = step(
        &current_state,
        &current_ctx,
        &FsmEvent::FrontHeadlampOnAck,
        Instant::now(),
    );

    assert!(!result
        .actions
        .contains(&DomainAction::RequestFrontHeadlampOn));
}

#[test]
fn given_lights_on_when_lux_above_off_threshold_then_requests_front_headlamp_off() {
    let current_state = FsmState::Driving;
    let mut current_ctx = ctx_with_headlamp_state(LightingState::On);
    current_ctx.visibility.ambient_lux = 50;

    let result = step(
        &current_state,
        &current_ctx,
        &FsmEvent::UpdateAmbientLux(LUX_OFF_THRESHOLD),
        Instant::now(),
    );

    assert!(result
        .actions
        .contains(&DomainAction::RequestFrontHeadlampOff));
}

#[test]
fn given_lights_off_when_lux_at_on_threshold_then_requests_front_headlamp_on() {
    let result = step(
        &FsmState::Idle,
        &valid_twin_context(),
        &FsmEvent::UpdateAmbientLux(LUX_ON_THRESHOLD),
        Instant::now(),
    );

    assert!(result
        .actions
        .contains(&DomainAction::RequestFrontHeadlampOn));
    assert_eq!(result.modified_ctx.headlamp.state, LightingState::OnRequested);
    assert!(result.modified_ctx.headlamp.ack_pending_since.is_some());
}

#[test]
fn given_lights_off_when_lux_in_deadband_then_does_not_request_front_headlamp_on() {
    let result = step(
        &FsmState::Idle,
        &valid_twin_context(),
        &FsmEvent::UpdateAmbientLux(LUX_ON_THRESHOLD + 10),
        Instant::now(),
    );

    assert!(!result
        .actions
        .contains(&DomainAction::RequestFrontHeadlampOn));
    assert_eq!(result.modified_ctx.headlamp.state, LightingState::Off);
}

#[test]
fn given_lights_on_when_lux_at_off_threshold_then_requests_front_headlamp_off() {
    let current_ctx = ctx_with_headlamp_state(LightingState::On);
    let result = step(
        &FsmState::Driving,
        &current_ctx,
        &FsmEvent::UpdateAmbientLux(LUX_OFF_THRESHOLD),
        Instant::now(),
    );

    assert!(result
        .actions
        .contains(&DomainAction::RequestFrontHeadlampOff));
    assert_eq!(result.modified_ctx.headlamp.state, LightingState::OffRequested);
    assert!(result.modified_ctx.headlamp.ack_pending_since.is_some());
}

#[test]
fn given_lights_on_when_lux_in_deadband_then_does_not_request_front_headlamp_off() {
    let current_ctx = ctx_with_headlamp_state(LightingState::On);
    let result = step(
        &FsmState::Driving,
        &current_ctx,
        &FsmEvent::UpdateAmbientLux(LUX_ON_THRESHOLD + 10),
        Instant::now(),
    );

    assert!(!result
        .actions
        .contains(&DomainAction::RequestFrontHeadlampOff));
    assert_eq!(result.modified_ctx.headlamp.state, LightingState::On);
}

#[test]
fn given_lights_on_requested_when_low_lux_arrives_then_does_not_emit_duplicate_on_request() {
    let mut current_ctx = ctx_with_headlamp_state(LightingState::OnRequested);
    current_ctx.visibility.ambient_lux = 20;
    let result = step(
        &FsmState::Idle,
        &current_ctx,
        &FsmEvent::UpdateAmbientLux(20),
        Instant::now(),
    );

    assert!(!result
        .actions
        .contains(&DomainAction::RequestFrontHeadlampOn));
    assert_eq!(result.modified_ctx.headlamp.state, LightingState::OnRequested);
}

#[test]
fn given_lights_off_requested_when_high_lux_arrives_then_does_not_emit_duplicate_off_request() {
    let mut current_ctx = ctx_with_headlamp_state(LightingState::OffRequested);
    current_ctx.visibility.ambient_lux = 50;
    let result = step(
        &FsmState::Driving,
        &current_ctx,
        &FsmEvent::UpdateAmbientLux(LUX_OFF_THRESHOLD),
        Instant::now(),
    );

    assert!(!result
        .actions
        .contains(&DomainAction::RequestFrontHeadlampOff));
    assert_eq!(result.modified_ctx.headlamp.state, LightingState::OffRequested);
}

#[test]
fn given_on_requested_when_ack_on_then_transitions_to_on() {
    let current_ctx = ctx_with_headlamp_state(LightingState::OnRequested);
    let result = step(
        &FsmState::Driving,
        &current_ctx,
        &FsmEvent::FrontHeadlampOnAck,
        Instant::now(),
    );
    assert_eq!(result.modified_ctx.headlamp.state, LightingState::On);
    assert!(result.modified_ctx.headlamp.ack_pending_since.is_none());
}

#[test]
fn given_off_requested_when_ack_off_then_transitions_to_off() {
    let current_ctx = ctx_with_headlamp_state(LightingState::OffRequested);
    let result = step(
        &FsmState::Driving,
        &current_ctx,
        &FsmEvent::FrontHeadlampOffAck,
        Instant::now(),
    );
    assert_eq!(result.modified_ctx.headlamp.state, LightingState::Off);
    assert!(result.modified_ctx.headlamp.ack_pending_since.is_none());
}

/// Elapsed time is half of `FRONT_HEADLAMP_ON_ACK_WAIT`, so `>=` deadline is false — no timeout.
#[test]
fn given_on_requested_when_timer_tick_before_ack_deadline_then_stays_pending() {
    let t0 = Instant::now();
    let current_ctx = ctx_with_pending_headlamp(LightingState::OnRequested, t0, 20);
    let result = step(
        &FsmState::Idle,
        &current_ctx,
        &FsmEvent::TimerTick,
        t0 + FRONT_HEADLAMP_ON_ACK_WAIT / 2,
    );
    assert_eq!(result.modified_ctx.headlamp.state, LightingState::OnRequested);
    assert!(!result
        .actions
        .iter()
        .any(|a| matches!(a, DomainAction::LogWarning(_))));
}

/// Elapsed time is half of `FRONT_HEADLAMP_OFF_ACK_WAIT`, so `>=` deadline is false — no timeout.
#[test]
fn given_off_requested_when_timer_tick_before_ack_deadline_then_stays_pending() {
    let t0 = Instant::now();
    let current_ctx = ctx_with_pending_headlamp(LightingState::OffRequested, t0, 50);
    let result = step(
        &FsmState::Driving,
        &current_ctx,
        &FsmEvent::TimerTick,
        t0 + FRONT_HEADLAMP_OFF_ACK_WAIT / 2,
    );
    assert_eq!(result.modified_ctx.headlamp.state, LightingState::OffRequested);
    assert!(!result
        .actions
        .iter()
        .any(|a| matches!(a, DomainAction::LogWarning(_))));
}

/// `now - since == FRONT_HEADLAMP_ON_ACK_WAIT` satisfies `>=` in `step` — timeout fires.
#[test]
fn given_on_requested_when_timer_tick_at_exact_ack_wait_then_times_out_to_off() {
    let t0 = Instant::now();
    let current_ctx = ctx_with_pending_headlamp(LightingState::OnRequested, t0, 20);
    let result = step(
        &FsmState::Idle,
        &current_ctx,
        &FsmEvent::TimerTick,
        t0 + FRONT_HEADLAMP_ON_ACK_WAIT,
    );
    assert_eq!(result.modified_ctx.headlamp.state, LightingState::Off);
    assert!(result.modified_ctx.headlamp.ack_pending_since.is_none());
    assert!(result.actions.iter().any(|a| matches!(
        a,
        DomainAction::LogWarning(msg) if msg.contains("ON request")
    )));
}

/// `now - since == FRONT_HEADLAMP_OFF_ACK_WAIT` satisfies `>=` in `step` — timeout fires.
#[test]
fn given_off_requested_when_timer_tick_at_exact_ack_wait_then_times_out_to_on() {
    let t0 = Instant::now();
    let current_ctx = ctx_with_pending_headlamp(LightingState::OffRequested, t0, 50);
    let result = step(
        &FsmState::Driving,
        &current_ctx,
        &FsmEvent::TimerTick,
        t0 + FRONT_HEADLAMP_OFF_ACK_WAIT,
    );
    assert_eq!(result.modified_ctx.headlamp.state, LightingState::On);
    assert!(result.modified_ctx.headlamp.ack_pending_since.is_none());
    assert!(result.actions.iter().any(|a| matches!(
        a,
        DomainAction::LogWarning(msg) if msg.contains("OFF request")
    )));
}

/// After timeout clears pending, the next `TimerTick` must not emit another lighting warning.
#[test]
fn given_on_requested_second_timer_tick_after_timeout_does_not_double_warn() {
    let t0 = Instant::now();
    let ctx_pending = ctx_with_pending_headlamp(LightingState::OnRequested, t0, 20);
    let deadline = t0 + FRONT_HEADLAMP_ON_ACK_WAIT;
    let after_timeout = step(
        &FsmState::Idle,
        &ctx_pending,
        &FsmEvent::TimerTick,
        deadline,
    );
    assert_eq!(after_timeout.modified_ctx.headlamp.state, LightingState::Off);
    assert_eq!(
        after_timeout
            .actions
            .iter()
            .filter(|a| matches!(a, DomainAction::LogWarning(_)))
            .count(),
        1
    );

    let second_tick = step(
        &FsmState::Idle,
        &after_timeout.modified_ctx,
        &FsmEvent::TimerTick,
        deadline + FRONT_HEADLAMP_ON_ACK_WAIT,
    );
    assert!(!second_tick
        .actions
        .iter()
        .any(|a| matches!(a, DomainAction::LogWarning(_))));
}

/// Same idempotence as [`given_on_requested_second_timer_tick_after_timeout_does_not_double_warn`] for OFF pending.
#[test]
fn given_off_requested_second_timer_tick_after_timeout_does_not_double_warn() {
    let t0 = Instant::now();
    let ctx_pending = ctx_with_pending_headlamp(LightingState::OffRequested, t0, 50);
    let deadline = t0 + FRONT_HEADLAMP_OFF_ACK_WAIT;
    let after_timeout = step(
        &FsmState::Driving,
        &ctx_pending,
        &FsmEvent::TimerTick,
        deadline,
    );
    assert_eq!(after_timeout.modified_ctx.headlamp.state, LightingState::On);
    assert_eq!(
        after_timeout
            .actions
            .iter()
            .filter(|a| matches!(a, DomainAction::LogWarning(_)))
            .count(),
        1
    );

    let second_tick = step(
        &FsmState::Driving,
        &after_timeout.modified_ctx,
        &FsmEvent::TimerTick,
        deadline + FRONT_HEADLAMP_OFF_ACK_WAIT,
    );
    assert!(!second_tick
        .actions
        .iter()
        .any(|a| matches!(a, DomainAction::LogWarning(_))));
}

#[test]
fn given_on_requested_when_actuation_incomplete_timed_out_then_recover_to_off() {
    let t0 = Instant::now();
    let current_ctx = ctx_with_pending_headlamp(LightingState::OnRequested, t0, 100);
    let result = step(
        &FsmState::Idle,
        &current_ctx,
        &FsmEvent::FrontHeadlampActuationIncomplete {
            direction: FrontHeadlampSwitchDirection::On,
            cause: FrontHeadlampIncompleteCause::TimedOut,
        },
        t0,
    );
    assert_eq!(result.modified_ctx.headlamp.state, LightingState::Off);
    assert!(result.modified_ctx.headlamp.ack_pending_since.is_none());
    assert_eq!(
        result
            .actions
            .iter()
            .filter(|a| matches!(a, DomainAction::LogWarning(_)))
            .count(),
        1
    );
    assert!(result.actions.iter().any(|a| matches!(
        a,
        DomainAction::LogWarning(msg) if msg.contains("ON request")
    )));
}

#[test]
fn given_off_requested_when_actuation_incomplete_timed_out_then_recover_to_on() {
    let t0 = Instant::now();
    let current_ctx = ctx_with_pending_headlamp(LightingState::OffRequested, t0, 50);
    let result = step(
        &FsmState::Driving,
        &current_ctx,
        &FsmEvent::FrontHeadlampActuationIncomplete {
            direction: FrontHeadlampSwitchDirection::Off,
            cause: FrontHeadlampIncompleteCause::TimedOut,
        },
        t0,
    );
    assert_eq!(result.modified_ctx.headlamp.state, LightingState::On);
    assert!(result.modified_ctx.headlamp.ack_pending_since.is_none());
    assert_eq!(
        result
            .actions
            .iter()
            .filter(|a| matches!(a, DomainAction::LogWarning(_)))
            .count(),
        1
    );
    assert!(result.actions.iter().any(|a| matches!(
        a,
        DomainAction::LogWarning(msg) if msg.contains("OFF request")
    )));
}

#[test]
fn given_on_requested_when_actuation_incomplete_wrong_direction_then_no_op() {
    let t0 = Instant::now();
    let current_ctx = ctx_with_pending_headlamp(LightingState::OnRequested, t0, 20);
    let result = step(
        &FsmState::Idle,
        &current_ctx,
        &FsmEvent::FrontHeadlampActuationIncomplete {
            direction: FrontHeadlampSwitchDirection::Off,
            cause: FrontHeadlampIncompleteCause::TimedOut,
        },
        t0,
    );
    assert_eq!(result.modified_ctx.headlamp.state, LightingState::OnRequested);
    assert_eq!(result.modified_ctx.headlamp.ack_pending_since, Some(t0));
    assert!(!result
        .actions
        .iter()
        .any(|a| matches!(a, DomainAction::LogWarning(_))));
}

#[test]
fn given_off_requested_when_actuation_incomplete_wrong_direction_then_no_op() {
    let t0 = Instant::now();
    let current_ctx = ctx_with_pending_headlamp(LightingState::OffRequested, t0, 50);
    let result = step(
        &FsmState::Driving,
        &current_ctx,
        &FsmEvent::FrontHeadlampActuationIncomplete {
            direction: FrontHeadlampSwitchDirection::On,
            cause: FrontHeadlampIncompleteCause::TimedOut,
        },
        t0,
    );
    assert_eq!(result.modified_ctx.headlamp.state, LightingState::OffRequested);
    assert_eq!(result.modified_ctx.headlamp.ack_pending_since, Some(t0));
    assert!(!result
        .actions
        .iter()
        .any(|a| matches!(a, DomainAction::LogWarning(_))));
}

#[test]
fn given_lights_off_when_actuation_incomplete_on_then_no_recovery() {
    let result = step(
        &FsmState::Idle,
        &valid_twin_context(),
        &FsmEvent::FrontHeadlampActuationIncomplete {
            direction: FrontHeadlampSwitchDirection::On,
            cause: FrontHeadlampIncompleteCause::TimedOut,
        },
        Instant::now(),
    );
    assert_eq!(result.modified_ctx.headlamp.state, LightingState::Off);
    assert!(!result
        .actions
        .iter()
        .any(|a| matches!(a, DomainAction::LogWarning(_))));
}

#[test]
fn given_idle_on_requested_when_power_off_then_primary_off_and_lighting_cleared() {
    let t0 = Instant::now();
    let current_ctx = ctx_with_pending_headlamp(LightingState::OnRequested, t0, 100);
    let result = step(
        &FsmState::Idle,
        &current_ctx,
        &FsmEvent::PowerOff,
        t0,
    );
    assert_eq!(result.next_state, FsmState::Off);
    assert_eq!(result.modified_ctx.headlamp.state, LightingState::Off);
    assert!(result.modified_ctx.headlamp.ack_pending_since.is_none());
}
