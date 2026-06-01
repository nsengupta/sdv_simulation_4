//! Front-headlamp assembly: lighting state + ACK-wait bookkeeping.
//!
//! Owns its own data and the rules over it (lux-driven request, ACK handling,
//! ACK-wait timeout, incomplete recovery, ignition-off reset). The *when* is
//! still decided by the orchestrator ([`crate::fsm::step`]); this assembly is
//! the *how*.
//!
//! Step 1 note: effects are appended to a `&mut Vec<DomainAction>` out-param
//! (same shape as the previous free functions). Step 2 swaps this for a
//! returned outcome the enclosing actor maps to messages.

use std::time::{Duration, Instant};

use crate::front_headlamp_log;
use crate::fsm::machineries::{
    DomainAction, FrontHeadlampIncompleteCause, FrontHeadlampSwitchDirection, LightingState,
};
use crate::vehicle_physics::{
    FRONT_HEADLAMP_OFF_ACK_WAIT, FRONT_HEADLAMP_ON_ACK_WAIT, LUX_OFF_THRESHOLD, LUX_ON_THRESHOLD,
};

#[derive(Debug, Clone, PartialEq)]
pub struct HeadlampContext {
    pub state: LightingState,
    /// When set, we are waiting for a front-headlamp ACK for the current
    /// `OnRequested` / `OffRequested` state.
    pub ack_pending_since: Option<Instant>,
}

impl Default for HeadlampContext {
    fn default() -> Self {
        Self {
            state: LightingState::Off,
            ack_pending_since: None,
        }
    }
}

impl HeadlampContext {
    /// Positive ACK for an ON command: lamp is now on, stop waiting.
    pub fn apply_on_ack(&mut self) {
        self.state = LightingState::On;
        self.ack_pending_since = None;
    }

    /// Positive ACK for an OFF command: lamp is now off, stop waiting.
    pub fn apply_off_ack(&mut self) {
        self.state = LightingState::Off;
        self.ack_pending_since = None;
    }

    /// Lux-driven ON/OFF request when crossing thresholds from a settled state.
    ///
    /// `prev_state` is the lighting state *before* this event (Step 1 semantics:
    /// the request decision is made against the pre-event state).
    pub fn evaluate_lux(
        &mut self,
        prev_state: LightingState,
        lux: u16,
        now: Instant,
        actions: &mut Vec<DomainAction>,
    ) {
        match prev_state {
            LightingState::Off if lux <= LUX_ON_THRESHOLD => {
                self.state = LightingState::OnRequested;
                self.ack_pending_since = Some(now);
                actions.push(DomainAction::RequestFrontHeadlampOn);
            }
            LightingState::On if lux >= LUX_OFF_THRESHOLD => {
                self.state = LightingState::OffRequested;
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
            LightingState::OnRequested if ack_wait_elapsed(since, now, FRONT_HEADLAMP_ON_ACK_WAIT) => {
                self.recover_incomplete(
                    FrontHeadlampSwitchDirection::On,
                    FrontHeadlampIncompleteCause::TimedOut,
                    actions,
                );
            }
            LightingState::OffRequested
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
        self.state = LightingState::Off;
        self.ack_pending_since = None;
    }

    /// Recover from a failed front-headlamp command when `direction` matches the
    /// pending request.
    fn recover_incomplete(
        &mut self,
        direction: FrontHeadlampSwitchDirection,
        cause: FrontHeadlampIncompleteCause,
        actions: &mut Vec<DomainAction>,
    ) {
        let matches_pending = matches!(
            (self.state, direction),
            (LightingState::OnRequested, FrontHeadlampSwitchDirection::On)
                | (LightingState::OffRequested, FrontHeadlampSwitchDirection::Off)
        );
        if !matches_pending {
            return;
        }

        self.ack_pending_since = None;
        let warning = front_headlamp_log::alert_incomplete(direction, cause);
        match direction {
            FrontHeadlampSwitchDirection::On => {
                self.state = LightingState::Off;
                actions.push(DomainAction::LogWarning(warning));
            }
            FrontHeadlampSwitchDirection::Off => {
                self.state = LightingState::On;
                actions.push(DomainAction::LogWarning(warning));
            }
        }
    }
}

fn ack_wait_elapsed(since: Instant, now: Instant, wait: Duration) -> bool {
    now.saturating_duration_since(since) >= wait
}
