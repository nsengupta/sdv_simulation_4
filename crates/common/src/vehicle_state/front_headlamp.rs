//! Front-headlamp zone (L1): alphabet + context + behavior.
//!
//! **ADR-5 alphabet:** [`HeadlampState`], [`HeadlampMessage`], [`HeadlampOutcome`].
//! L1 pattern: [`HeadlampContext::on_receiving_message`] → [`HeadlampZoneReply`]; L4 demux maps outcomes.

use std::time::{Duration, Instant};

use crate::front_headlamp_log;
use crate::vehicle_physics::{
    FRONT_HEADLAMP_OFF_ACK_WAIT, FRONT_HEADLAMP_ON_ACK_WAIT, LUX_OFF_THRESHOLD, LUX_ON_THRESHOLD,
};

// --- L1 alphabet (ADR-5) ---

/// Snapshot — what the headlamp zone **IS**.
///
/// Lifecycle:
/// - `Off`          — assembly not started; ignores all lux events.
/// - `Ready`        — assembly active, physical lamp dark; lux triggers `OnRequested`.
/// - `OnRequested`  — ON command in flight; waiting for `AckOn`.
/// - `On`           — physical lamp confirmed on.
/// - `OffRequested` — OFF command in flight; waiting for `AckOff`.
///
/// `BecomeOn` drives `Off → Ready`; `BecomeOff` drives `Ready | On → Off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlampState {
    Off,
    Ready,
    OnRequested,
    On,
    OffRequested,
}

/// Inputs — L4 demux feeds these (from [`crate::fsm::FsmEvent`] today; `TwinIngress` per ADR-6 later).
///
/// Lifecycle messages (Phase 2):
/// - `BecomeOn`  — Brain tells the assembly to start; drives `Off → Ready`.
/// - `BecomeOff` — Brain tells the assembly to stop;  drives `Ready | On → Off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlampMessage {
    BecomeOn,
    BecomeOff,
    AmbientLux(u16),
    AckOn,
    AckOff,
    ActuationIncomplete {
        direction: FrontHeadlampSwitchDirection,
        cause: FrontHeadlampIncompleteCause,
    },
    TimerTick,
    ResetForIgnitionOff,
}

/// Zone twinlet reply after one [`HeadlampMessage`] — not a full FSM/brain turn (Q5).
///
/// `ctx` is the updated zone snapshot; `outcomes` are zone egress for L4 to map. The brain embeds
/// `ctx` into [`VehicleContext`](crate::vehicle_state::VehicleContext) (phase A); toward phase C
/// the embed may shrink to whatever the child still sends here.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadlampZoneReply {
    pub ctx: HeadlampContext,
    pub outcomes: Vec<HeadlampOutcome>,
}

/// Zone-local egress — L4 maps to actuation / diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub enum HeadlampOutcome {
    RequestOn,
    RequestOff,
    LogWarning(String),
}

/// Which switch path an incomplete outcome refers to (ON vs OFF request in flight).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrontHeadlampSwitchDirection {
    On,
    Off,
}

/// Why a command did not complete with a positive acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FrontHeadlampIncompleteCause {
    TimedOut,
    NegativeAck,
}

// --- Context (state + bookkeeping) ---

#[derive(Debug, Clone, PartialEq)]
pub struct HeadlampContext {
    pub state: HeadlampState,
    /// When set, we are waiting for a front-headlamp ACK for the current
    /// `OnRequested` / `OffRequested` state.
    pub ack_pending_since: Option<Instant>,
}

impl Default for HeadlampContext {
    fn default() -> Self {
        Self {
            state: HeadlampState::Off,
            ack_pending_since: None,
        }
    }
}

impl HeadlampContext {
    /// Pure L1 handler for one zone message — snapshot in, [`HeadlampZoneReply`] out (same pattern
    /// for other assemblies at this layer). Used by tests, [`HeadlampActor`] body, and local demux.
    pub fn on_receiving_message(
        &self,
        msg: HeadlampMessage,
        now: Instant,
    ) -> HeadlampZoneReply {
        let prev_state = self.state;
        let mut next = self.clone();
        let outcomes = next.apply_in_place(msg, prev_state, now);
        HeadlampZoneReply {
            ctx: next,
            outcomes,
        }
    }

    fn apply_in_place(
        &mut self,
        msg: HeadlampMessage,
        prev_state: HeadlampState,
        now: Instant,
    ) -> Vec<HeadlampOutcome> {
        let mut outcomes = Vec::new();
        match msg {
            HeadlampMessage::BecomeOn => self.apply_become_on(),
            HeadlampMessage::BecomeOff => self.apply_become_off(),
            HeadlampMessage::AmbientLux(lux) => {
                self.evaluate_lux(prev_state, lux, now, &mut outcomes);
            }
            HeadlampMessage::AckOn => self.apply_on_ack(),
            HeadlampMessage::AckOff => self.apply_off_ack(),
            HeadlampMessage::ActuationIncomplete { direction, cause } => {
                self.recover_incomplete(direction, cause, &mut outcomes);
            }
            HeadlampMessage::TimerTick => self.on_timer_tick(now, &mut outcomes),
            HeadlampMessage::ResetForIgnitionOff => self.reset_for_ignition_off(),
        }
        outcomes
    }

    fn apply_become_on(&mut self) {
        self.state = HeadlampState::Ready;
        self.ack_pending_since = None;
    }

    fn apply_become_off(&mut self) {
        self.state = HeadlampState::Off;
        self.ack_pending_since = None;
    }

    fn apply_on_ack(&mut self) {
        self.state = HeadlampState::On;
        self.ack_pending_since = None;
    }

    /// AckOff returns to `Ready` (assembly remains active, physical lamp is now dark).
    fn apply_off_ack(&mut self) {
        self.state = HeadlampState::Ready;
        self.ack_pending_since = None;
    }

    /// Lux-driven ON/OFF when crossing thresholds from a settled state (`prev_state` = pre-event).
    /// Only `Ready` (not `Off`) can trigger `OnRequested`; `Off` means the assembly is not started.
    fn evaluate_lux(
        &mut self,
        prev_state: HeadlampState,
        lux: u16,
        now: Instant,
        outcomes: &mut Vec<HeadlampOutcome>,
    ) {
        match prev_state {
            HeadlampState::Ready if lux <= LUX_ON_THRESHOLD => {
                self.state = HeadlampState::OnRequested;
                self.ack_pending_since = Some(now);
                outcomes.push(HeadlampOutcome::RequestOn);
            }
            HeadlampState::On if lux >= LUX_OFF_THRESHOLD => {
                self.state = HeadlampState::OffRequested;
                self.ack_pending_since = Some(now);
                outcomes.push(HeadlampOutcome::RequestOff);
            }
            _ => {}
        }
    }

    fn on_timer_tick(&mut self, now: Instant, outcomes: &mut Vec<HeadlampOutcome>) {
        let Some(since) = self.ack_pending_since else {
            return;
        };
        match self.state {
            HeadlampState::OnRequested
                if ack_wait_elapsed(since, now, FRONT_HEADLAMP_ON_ACK_WAIT) =>
            {
                self.recover_incomplete(
                    FrontHeadlampSwitchDirection::On,
                    FrontHeadlampIncompleteCause::TimedOut,
                    outcomes,
                );
            }
            HeadlampState::OffRequested
                if ack_wait_elapsed(since, now, FRONT_HEADLAMP_OFF_ACK_WAIT) =>
            {
                self.recover_incomplete(
                    FrontHeadlampSwitchDirection::Off,
                    FrontHeadlampIncompleteCause::TimedOut,
                    outcomes,
                );
            }
            _ => {}
        }
    }

    fn reset_for_ignition_off(&mut self) {
        self.state = HeadlampState::Off;
        self.ack_pending_since = None;
    }

    fn recover_incomplete(
        &mut self,
        direction: FrontHeadlampSwitchDirection,
        cause: FrontHeadlampIncompleteCause,
        outcomes: &mut Vec<HeadlampOutcome>,
    ) {
        let matches_pending = matches!(
            (self.state, direction),
            (HeadlampState::OnRequested, FrontHeadlampSwitchDirection::On)
                | (HeadlampState::OffRequested, FrontHeadlampSwitchDirection::Off)
        );
        if !matches_pending {
            return;
        }

        self.ack_pending_since = None;
        let warning = front_headlamp_log::alert_incomplete(direction, cause);
        match direction {
            FrontHeadlampSwitchDirection::On => {
                // ON attempt failed: lamp is still dark but assembly is active → Ready.
                self.state = HeadlampState::Ready;
                outcomes.push(HeadlampOutcome::LogWarning(warning));
            }
            FrontHeadlampSwitchDirection::Off => {
                self.state = HeadlampState::On;
                outcomes.push(HeadlampOutcome::LogWarning(warning));
            }
        }
    }
}

fn ack_wait_elapsed(since: Instant, now: Instant, wait: Duration) -> bool {
    now.saturating_duration_since(since) >= wait
}
