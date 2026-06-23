//! Phase 2 RED → GREEN contract: HeadlampState::Ready, BecomeOn/BecomeOff messages, AssemblyId.
//!
//! Verified behaviours:
//! - `BecomeOn` : Off  → Ready  (assembly activation)
//! - `BecomeOff`: Ready → Off   (assembly deactivation from idle)
//! - `BecomeOff`: On   → Off    (forced deactivation from lit state)
//! - `Off`  ignores lux (assembly not started)
//! - `Ready` triggers `OnRequested` on low lux (assembly active, lamp dark)
//! - `AckOff` lands in `Ready`, not `Off` (lamp off but assembly still active)
//! - `ActuationIncomplete(On)` recovers to `Ready`, not `Off`
//! - `AssemblyId::Headlamp` is constructible (vocabulary smoke)

use std::time::Instant;

use crate::fsm::{
    FrontHeadlampIncompleteCause, FrontHeadlampSwitchDirection, HeadlampState, AssemblyId,
};
use crate::vehicle_physics::LUX_ON_THRESHOLD;
use crate::vehicle_state::{HeadlampContext, HeadlampMessage, HeadlampOutcome};

fn ctx_in(state: HeadlampState) -> HeadlampContext {
    let mut c = HeadlampContext::default();
    c.state = state;
    c
}

// --- BecomeOn ---

#[test]
fn become_on_transitions_off_to_ready() {
    let ctx = HeadlampContext::default(); // starts in Off
    let reply = ctx.on_receiving_message(HeadlampMessage::BecomeOn, Instant::now());
    assert_eq!(reply.ctx.state, HeadlampState::Ready);
    assert!(reply.ctx.ack_pending_since.is_none());
    assert!(reply.outcomes.is_empty(), "BecomeOn emits no zone outcomes");
}

// --- BecomeOff ---

#[test]
fn become_off_transitions_ready_to_off() {
    let ctx = ctx_in(HeadlampState::Ready);
    let reply = ctx.on_receiving_message(HeadlampMessage::BecomeOff, Instant::now());
    assert_eq!(reply.ctx.state, HeadlampState::Off);
    assert!(reply.ctx.ack_pending_since.is_none());
    assert!(reply.outcomes.is_empty(), "BecomeOff emits no zone outcomes");
}

#[test]
fn become_off_from_on_forces_lamp_to_off() {
    let ctx = ctx_in(HeadlampState::On);
    let reply = ctx.on_receiving_message(HeadlampMessage::BecomeOff, Instant::now());
    assert_eq!(reply.ctx.state, HeadlampState::Off);
    assert!(reply.ctx.ack_pending_since.is_none());
}

// --- Off state ignores lux ---

#[test]
fn off_state_ignores_lux_below_threshold() {
    let ctx = HeadlampContext::default(); // Off
    let reply = ctx.on_receiving_message(HeadlampMessage::AmbientLux(LUX_ON_THRESHOLD - 1), Instant::now());
    assert_eq!(
        reply.ctx.state,
        HeadlampState::Off,
        "Off state must not react to lux — assembly is not started"
    );
    assert!(reply.outcomes.is_empty());
}

// --- Ready state reacts to lux ---

#[test]
fn ready_state_triggers_on_requested_on_low_lux() {
    let ctx = ctx_in(HeadlampState::Ready);
    let reply = ctx.on_receiving_message(HeadlampMessage::AmbientLux(LUX_ON_THRESHOLD), Instant::now());
    assert_eq!(reply.ctx.state, HeadlampState::OnRequested);
    assert!(reply.ctx.ack_pending_since.is_some());
    assert!(reply.outcomes.contains(&HeadlampOutcome::RequestOn));
}

#[test]
fn ready_state_ignores_bright_lux() {
    let ctx = ctx_in(HeadlampState::Ready);
    let reply = ctx.on_receiving_message(HeadlampMessage::AmbientLux(LUX_ON_THRESHOLD + 100), Instant::now());
    assert_eq!(reply.ctx.state, HeadlampState::Ready);
    assert!(reply.outcomes.is_empty());
}

// --- AckOff lands in Ready ---

#[test]
fn ack_off_transitions_off_requested_to_ready() {
    let mut ctx = ctx_in(HeadlampState::OffRequested);
    ctx.ack_pending_since = Some(Instant::now());
    let reply = ctx.on_receiving_message(HeadlampMessage::AckOff, Instant::now());
    assert_eq!(
        reply.ctx.state,
        HeadlampState::Ready,
        "AckOff must land in Ready (assembly still active), not Off"
    );
    assert!(reply.ctx.ack_pending_since.is_none());
}

// --- ActuationIncomplete(On) recovers to Ready ---

#[test]
fn actuation_incomplete_on_recovers_to_ready() {
    let t0 = Instant::now();
    let mut ctx = ctx_in(HeadlampState::OnRequested);
    ctx.ack_pending_since = Some(t0);
    let reply = ctx.on_receiving_message(
        HeadlampMessage::ActuationIncomplete {
            direction: FrontHeadlampSwitchDirection::On,
            cause: FrontHeadlampIncompleteCause::TimedOut,
        },
        t0,
    );
    assert_eq!(
        reply.ctx.state,
        HeadlampState::Ready,
        "ActuationIncomplete(On) must recover to Ready (assembly active), not Off"
    );
    assert!(reply.ctx.ack_pending_since.is_none());
    assert!(
        reply.outcomes.iter().any(|o| matches!(o, HeadlampOutcome::LogWarning(_))),
        "incomplete actuation must emit a LogWarning"
    );
}

// --- AssemblyId vocabulary smoke ---

#[test]
fn zone_id_headlamp_is_constructible() {
    let id = AssemblyId::Headlamp;
    assert_eq!(id, AssemblyId::Headlamp);
}
