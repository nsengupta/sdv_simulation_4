//! Behavioral contract tests for lighting sub-state behavior.

use crate::fsm::{
    FrontHeadlampIncompleteCause, FrontHeadlampSwitchDirection, DomainAction, FsmEvent, FsmState,
    HeadlampState,
};
use crate::twin_runtime::twin_turn;
use crate::vehicle_state::VehicleContext;
use crate::vehicle_physics::{
    FRONT_HEADLAMP_OFF_ACK_WAIT, FRONT_HEADLAMP_ON_ACK_WAIT, LUX_OFF_THRESHOLD, LUX_ON_THRESHOLD,
};
use std::time::Instant;

fn valid_twin_context() -> VehicleContext {
    VehicleContext::default()
}

fn ctx_with_headlamp_state(state: HeadlampState) -> VehicleContext {
    let mut ctx = valid_twin_context();
    ctx.headlamp.state = state;
    ctx
}

fn ctx_with_pending_headlamp(state: HeadlampState, since: Instant, ambient_lux: u16) -> VehicleContext {
    let mut ctx = valid_twin_context();
    ctx.headlamp.state = state;
    ctx.headlamp.ack_pending_since = Some(since);
    ctx.visibility.ambient_lux = ambient_lux;
    ctx
}

#[test]
fn given_lights_off_when_lux_below_on_threshold_then_requests_front_headlamp_on() {
    let current_state = FsmState::Idle;
    // Assembly is active (Ready) but physical lamp is off; low lux should command it on.
    let current_ctx = ctx_with_headlamp_state(HeadlampState::Ready);

    // Dim side of emulator jitter band (~815) is below LUX_ON_THRESHOLD (840).
    let result = twin_turn(
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
    let mut current_ctx = ctx_with_headlamp_state(HeadlampState::OnRequested);
    current_ctx.visibility.ambient_lux = 20;

    let result = twin_turn(
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
    let mut current_ctx = ctx_with_headlamp_state(HeadlampState::On);
    current_ctx.visibility.ambient_lux = 50;

    let result = twin_turn(
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
    // Assembly active (Ready): lux at the exact ON threshold must trigger the ON request.
    let result = twin_turn(
        &FsmState::Idle,
        &ctx_with_headlamp_state(HeadlampState::Ready),
        &FsmEvent::UpdateAmbientLux(LUX_ON_THRESHOLD),
        Instant::now(),
    );

    assert!(result
        .actions
        .contains(&DomainAction::RequestFrontHeadlampOn));
    assert_eq!(result.modified_ctx.headlamp.state, HeadlampState::OnRequested);
    assert!(result.modified_ctx.headlamp.ack_pending_since.is_some());
}

#[test]
fn given_lights_off_when_lux_in_deadband_then_does_not_request_front_headlamp_on() {
    // Assembly active (Ready) but lux is above ON threshold — no ON command should fire.
    let result = twin_turn(
        &FsmState::Idle,
        &ctx_with_headlamp_state(HeadlampState::Ready),
        &FsmEvent::UpdateAmbientLux(LUX_ON_THRESHOLD + 10),
        Instant::now(),
    );

    assert!(!result
        .actions
        .contains(&DomainAction::RequestFrontHeadlampOn));
    assert_eq!(result.modified_ctx.headlamp.state, HeadlampState::Ready);
}

#[test]
fn given_lights_on_when_lux_at_off_threshold_then_requests_front_headlamp_off() {
    let current_ctx = ctx_with_headlamp_state(HeadlampState::On);
    let result = twin_turn(
        &FsmState::Driving,
        &current_ctx,
        &FsmEvent::UpdateAmbientLux(LUX_OFF_THRESHOLD),
        Instant::now(),
    );

    assert!(result
        .actions
        .contains(&DomainAction::RequestFrontHeadlampOff));
    assert_eq!(result.modified_ctx.headlamp.state, HeadlampState::OffRequested);
    assert!(result.modified_ctx.headlamp.ack_pending_since.is_some());
}

#[test]
fn given_lights_on_when_lux_in_deadband_then_does_not_request_front_headlamp_off() {
    let current_ctx = ctx_with_headlamp_state(HeadlampState::On);
    let result = twin_turn(
        &FsmState::Driving,
        &current_ctx,
        &FsmEvent::UpdateAmbientLux(LUX_ON_THRESHOLD + 10),
        Instant::now(),
    );

    assert!(!result
        .actions
        .contains(&DomainAction::RequestFrontHeadlampOff));
    assert_eq!(result.modified_ctx.headlamp.state, HeadlampState::On);
}

#[test]
fn given_lights_on_requested_when_low_lux_arrives_then_does_not_emit_duplicate_on_request() {
    let mut current_ctx = ctx_with_headlamp_state(HeadlampState::OnRequested);
    current_ctx.visibility.ambient_lux = 20;
    let result = twin_turn(
        &FsmState::Idle,
        &current_ctx,
        &FsmEvent::UpdateAmbientLux(20),
        Instant::now(),
    );

    assert!(!result
        .actions
        .contains(&DomainAction::RequestFrontHeadlampOn));
    assert_eq!(result.modified_ctx.headlamp.state, HeadlampState::OnRequested);
}

#[test]
fn given_lights_off_requested_when_high_lux_arrives_then_does_not_emit_duplicate_off_request() {
    let mut current_ctx = ctx_with_headlamp_state(HeadlampState::OffRequested);
    current_ctx.visibility.ambient_lux = 50;
    let result = twin_turn(
        &FsmState::Driving,
        &current_ctx,
        &FsmEvent::UpdateAmbientLux(LUX_OFF_THRESHOLD),
        Instant::now(),
    );

    assert!(!result
        .actions
        .contains(&DomainAction::RequestFrontHeadlampOff));
    assert_eq!(result.modified_ctx.headlamp.state, HeadlampState::OffRequested);
}

#[test]
fn given_on_requested_when_ack_on_then_transitions_to_on() {
    let current_ctx = ctx_with_headlamp_state(HeadlampState::OnRequested);
    let result = twin_turn(
        &FsmState::Driving,
        &current_ctx,
        &FsmEvent::FrontHeadlampOnAck,
        Instant::now(),
    );
    assert_eq!(result.modified_ctx.headlamp.state, HeadlampState::On);
    assert!(result.modified_ctx.headlamp.ack_pending_since.is_none());
}

#[test]
fn given_off_requested_when_ack_off_then_transitions_to_ready() {
    // AckOff returns to Ready (assembly active, physical lamp dark) not Off (assembly stopped).
    let current_ctx = ctx_with_headlamp_state(HeadlampState::OffRequested);
    let result = twin_turn(
        &FsmState::Driving,
        &current_ctx,
        &FsmEvent::FrontHeadlampOffAck,
        Instant::now(),
    );
    assert_eq!(result.modified_ctx.headlamp.state, HeadlampState::Ready);
    assert!(result.modified_ctx.headlamp.ack_pending_since.is_none());
}

/// Elapsed time is half of `FRONT_HEADLAMP_ON_ACK_WAIT`, so `>=` deadline is false — no timeout.
#[test]
fn given_on_requested_when_timer_tick_before_ack_deadline_then_stays_pending() {
    let t0 = Instant::now();
    let current_ctx = ctx_with_pending_headlamp(HeadlampState::OnRequested, t0, 20);
    let result = twin_turn(
        &FsmState::Idle,
        &current_ctx,
        &FsmEvent::TimerTick,
        t0 + FRONT_HEADLAMP_ON_ACK_WAIT / 2,
    );
    assert_eq!(result.modified_ctx.headlamp.state, HeadlampState::OnRequested);
    assert!(!result
        .actions
        .iter()
        .any(|a| matches!(a, DomainAction::LogWarning(_))));
}

/// Elapsed time is half of `FRONT_HEADLAMP_OFF_ACK_WAIT`, so `>=` deadline is false — no timeout.
#[test]
fn given_off_requested_when_timer_tick_before_ack_deadline_then_stays_pending() {
    let t0 = Instant::now();
    let current_ctx = ctx_with_pending_headlamp(HeadlampState::OffRequested, t0, 50);
    let result = twin_turn(
        &FsmState::Driving,
        &current_ctx,
        &FsmEvent::TimerTick,
        t0 + FRONT_HEADLAMP_OFF_ACK_WAIT / 2,
    );
    assert_eq!(result.modified_ctx.headlamp.state, HeadlampState::OffRequested);
    assert!(!result
        .actions
        .iter()
        .any(|a| matches!(a, DomainAction::LogWarning(_))));
}

/// `now - since == FRONT_HEADLAMP_ON_ACK_WAIT` satisfies `>=` in `step` — timeout fires.
#[test]
fn given_on_requested_when_timer_tick_at_exact_ack_wait_then_times_out_to_ready() {
    // Timeout on an ON attempt: lamp stays dark but assembly is active → Ready.
    let t0 = Instant::now();
    let current_ctx = ctx_with_pending_headlamp(HeadlampState::OnRequested, t0, 20);
    let result = twin_turn(
        &FsmState::Idle,
        &current_ctx,
        &FsmEvent::TimerTick,
        t0 + FRONT_HEADLAMP_ON_ACK_WAIT,
    );
    assert_eq!(result.modified_ctx.headlamp.state, HeadlampState::Ready);
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
    let current_ctx = ctx_with_pending_headlamp(HeadlampState::OffRequested, t0, 50);
    let result = twin_turn(
        &FsmState::Driving,
        &current_ctx,
        &FsmEvent::TimerTick,
        t0 + FRONT_HEADLAMP_OFF_ACK_WAIT,
    );
    assert_eq!(result.modified_ctx.headlamp.state, HeadlampState::On);
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
    let ctx_pending = ctx_with_pending_headlamp(HeadlampState::OnRequested, t0, 20);
    let deadline = t0 + FRONT_HEADLAMP_ON_ACK_WAIT;
    let after_timeout = twin_turn(
        &FsmState::Idle,
        &ctx_pending,
        &FsmEvent::TimerTick,
        deadline,
    );
    assert_eq!(after_timeout.modified_ctx.headlamp.state, HeadlampState::Ready);
    assert_eq!(
        after_timeout
            .actions
            .iter()
            .filter(|a| matches!(a, DomainAction::LogWarning(_)))
            .count(),
        1
    );

    let second_tick = twin_turn(
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
    let ctx_pending = ctx_with_pending_headlamp(HeadlampState::OffRequested, t0, 50);
    let deadline = t0 + FRONT_HEADLAMP_OFF_ACK_WAIT;
    let after_timeout = twin_turn(
        &FsmState::Driving,
        &ctx_pending,
        &FsmEvent::TimerTick,
        deadline,
    );
    assert_eq!(after_timeout.modified_ctx.headlamp.state, HeadlampState::On);
    assert_eq!(
        after_timeout
            .actions
            .iter()
            .filter(|a| matches!(a, DomainAction::LogWarning(_)))
            .count(),
        1
    );

    let second_tick = twin_turn(
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
fn given_on_requested_when_actuation_incomplete_timed_out_then_recover_to_ready() {
    // Failed ON attempt: lamp dark but assembly still active → Ready, not Off.
    let t0 = Instant::now();
    let current_ctx = ctx_with_pending_headlamp(HeadlampState::OnRequested, t0, 100);
    let result = twin_turn(
        &FsmState::Idle,
        &current_ctx,
        &FsmEvent::FrontHeadlampActuationIncomplete {
            direction: FrontHeadlampSwitchDirection::On,
            cause: FrontHeadlampIncompleteCause::TimedOut,
        },
        t0,
    );
    assert_eq!(result.modified_ctx.headlamp.state, HeadlampState::Ready);
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
    let current_ctx = ctx_with_pending_headlamp(HeadlampState::OffRequested, t0, 50);
    let result = twin_turn(
        &FsmState::Driving,
        &current_ctx,
        &FsmEvent::FrontHeadlampActuationIncomplete {
            direction: FrontHeadlampSwitchDirection::Off,
            cause: FrontHeadlampIncompleteCause::TimedOut,
        },
        t0,
    );
    assert_eq!(result.modified_ctx.headlamp.state, HeadlampState::On);
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
    let current_ctx = ctx_with_pending_headlamp(HeadlampState::OnRequested, t0, 20);
    let result = twin_turn(
        &FsmState::Idle,
        &current_ctx,
        &FsmEvent::FrontHeadlampActuationIncomplete {
            direction: FrontHeadlampSwitchDirection::Off,
            cause: FrontHeadlampIncompleteCause::TimedOut,
        },
        t0,
    );
    assert_eq!(result.modified_ctx.headlamp.state, HeadlampState::OnRequested);
    assert_eq!(result.modified_ctx.headlamp.ack_pending_since, Some(t0));
    assert!(!result
        .actions
        .iter()
        .any(|a| matches!(a, DomainAction::LogWarning(_))));
}

#[test]
fn given_off_requested_when_actuation_incomplete_wrong_direction_then_no_op() {
    let t0 = Instant::now();
    let current_ctx = ctx_with_pending_headlamp(HeadlampState::OffRequested, t0, 50);
    let result = twin_turn(
        &FsmState::Driving,
        &current_ctx,
        &FsmEvent::FrontHeadlampActuationIncomplete {
            direction: FrontHeadlampSwitchDirection::On,
            cause: FrontHeadlampIncompleteCause::TimedOut,
        },
        t0,
    );
    assert_eq!(result.modified_ctx.headlamp.state, HeadlampState::OffRequested);
    assert_eq!(result.modified_ctx.headlamp.ack_pending_since, Some(t0));
    assert!(!result
        .actions
        .iter()
        .any(|a| matches!(a, DomainAction::LogWarning(_))));
}

#[test]
fn given_lights_off_when_actuation_incomplete_on_then_no_recovery() {
    let result = twin_turn(
        &FsmState::Idle,
        &valid_twin_context(),
        &FsmEvent::FrontHeadlampActuationIncomplete {
            direction: FrontHeadlampSwitchDirection::On,
            cause: FrontHeadlampIncompleteCause::TimedOut,
        },
        Instant::now(),
    );
    assert_eq!(result.modified_ctx.headlamp.state, HeadlampState::Off);
    assert!(!result
        .actions
        .iter()
        .any(|a| matches!(a, DomainAction::LogWarning(_))));
}

#[test]
fn given_idle_on_requested_when_power_off_then_primary_off_and_lighting_cleared() {
    // Phase 1: PowerOff now moves to PreparingToStop (not Off directly). The BecomeOff
    // coordination barrier — and with it the headlamp reset — is wired in Phase 5. Until
    // then, the headlamp stays in its current state while the FSM holds in PreparingToStop.
    let t0 = Instant::now();
    let current_ctx = ctx_with_pending_headlamp(HeadlampState::OnRequested, t0, 100);
    let result = twin_turn(
        &FsmState::Idle,
        &current_ctx,
        &FsmEvent::PowerOff,
        t0,
    );
    assert!(matches!(result.next_state, FsmState::PreparingToStop { .. }));
    // Headlamp reset via BecomeOff is Phase 5 work; in Phase 1 the state is unchanged.
    assert_eq!(result.modified_ctx.headlamp.state, HeadlampState::OnRequested);
}
