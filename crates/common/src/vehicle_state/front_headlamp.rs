//! Front-headlamp zone (L1): alphabet + context + behavior.
//!
//! **ADR-5 alphabet:** [`HeadlampState`], [`HeadlampMessage`], [`HeadlampOutcome`].
//! Step 1 still applies behavior via a `&mut Vec<DomainAction>` out-param; milestone 2
//! maps [`HeadlampOutcome`] at L4 and drops the L2 import here.

use std::time::{Duration, Instant};

use crate::front_headlamp_log;
use crate::fsm::DomainAction;
use crate::vehicle_physics::{
    FRONT_HEADLAMP_OFF_ACK_WAIT, FRONT_HEADLAMP_ON_ACK_WAIT, LUX_OFF_THRESHOLD, LUX_ON_THRESHOLD,
};

// --- L1 alphabet (ADR-5) ---

/// Snapshot — what the headlamp zone **IS**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlampState {
    Off,
    OnRequested,
    On,
    OffRequested,
}

/// Inputs — future child-actor mailbox vocabulary (L4 demux feeds these).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlampMessage {
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

/// Zone-local egress — L4 maps to actuation / diagnostics / summarized [`FsmEvent`](crate::fsm::FsmEvent).
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
    /// Positive ACK for an ON command: lamp is now on, stop waiting.
    pub fn apply_on_ack(&mut self) {
        self.state = HeadlampState::On;
        self.ack_pending_since = None;
    }

    /// Positive ACK for an OFF command: lamp is now off, stop waiting.
    pub fn apply_off_ack(&mut self) {
        self.state = HeadlampState::Off;
        self.ack_pending_since = None;
    }

    /// Lux-driven ON/OFF request when crossing thresholds from a settled state.
    ///
    /// `prev_state` is the lighting state *before* this event (Step 1 semantics:
    /// the request decision is made against the pre-event state).
    pub fn evaluate_lux(
        &mut self,
        prev_state: HeadlampState,
        lux: u16,
        now: Instant,
        actions: &mut Vec<DomainAction>,
    ) {
        match prev_state {
            HeadlampState::Off if lux <= LUX_ON_THRESHOLD => {
                self.state = HeadlampState::OnRequested;
                self.ack_pending_since = Some(now);
                actions.push(DomainAction::RequestFrontHeadlampOn);
            }
            HeadlampState::On if lux >= LUX_OFF_THRESHOLD => {
                self.state = HeadlampState::OffRequested;
                self.ack_pending_since = Some(now);
                actions.push(DomainAction::RequestFrontHeadlampOff);
            }
            _ => {}
        }
    }

    /// On a heartbeat, if an ACK has been pending past its deadline, recover to a
    /// safe lighting state and log.
    pub fn on_timer_tick(&mut self, now: Instant, actions: &mut Vec<DomainAction>) {
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
                    actions,
                );
            }
            HeadlampState::OffRequested
                if ack_wait_elapsed(since, now, FRONT_HEADLAMP_OFF_ACK_WAIT) =>
            {
                self.recover_incomplete(
                    FrontHeadlampSwitchDirection::Off,
                    FrontHeadlampIncompleteCause::TimedOut,
                    actions,
                );
            }
            _ => {}
        }
    }

    /// Explicit incomplete signal (negative ACK or injected timeout) from ingress.
    pub fn on_incomplete(
        &mut self,
        direction: FrontHeadlampSwitchDirection,
        cause: FrontHeadlampIncompleteCause,
        actions: &mut Vec<DomainAction>,
    ) {
        self.recover_incomplete(direction, cause, actions);
    }

    /// Ignition off: clear lighting context to a safe state.
    pub fn reset_for_ignition_off(&mut self) {
        self.state = HeadlampState::Off;
        self.ack_pending_since = None;
    }

    fn recover_incomplete(
        &mut self,
        direction: FrontHeadlampSwitchDirection,
        cause: FrontHeadlampIncompleteCause,
        actions: &mut Vec<DomainAction>,
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
                self.state = HeadlampState::Off;
                actions.push(DomainAction::LogWarning(warning));
            }
            FrontHeadlampSwitchDirection::Off => {
                self.state = HeadlampState::On;
                actions.push(DomainAction::LogWarning(warning));
            }
        }
    }
}

fn ack_wait_elapsed(since: Instant, now: Instant, wait: Duration) -> bool {
    now.saturating_duration_since(since) >= wait
}
