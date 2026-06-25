//! Brain↔zone tell-back wait: retry, synthetic embed on exhaustion (ADR-7 step 6+).

use crate::vehicle_state::{HeadlampContext, HeadlampOutcome, HeadlampZoneReply};
use crate::vehicle_state::{WiperContext, WiperOutcome, WiperZoneReply};
use crate::twin_runtime::constants::{ZONE_TELL_BACK_ATTEMPT_COUNT, ZONE_TELL_BACK_MAX_RETRIES};

/// One in-flight tell-back wait (correlation for reply vs timeout).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TellBackWait {
    pub turn_id: u64,
    pub tell_attempt: u32,
    pub retries_remaining: u8,
}

impl TellBackWait {
    pub fn new(turn_id: u64) -> Self {
        Self {
            turn_id,
            tell_attempt: 0,
            retries_remaining: ZONE_TELL_BACK_MAX_RETRIES,
        }
    }

    pub fn matches(&self, turn_id: u64, tell_attempt: u32) -> bool {
        self.turn_id == turn_id && self.tell_attempt == tell_attempt
    }
}

/// Outcome when a tell-back deadline fires.
#[derive(Debug, Clone, PartialEq)]
pub enum TellBackTimeoutOutcome {
    /// Re-tell the same zone message; `wait` carries the next attempt id.
    Retry(TellBackWait),
    /// All attempts exhausted — commit with a synthetic zone reply.
    Exhausted(HeadlampZoneReply),
}

/// Decide retry vs synthetic embed after one tell-back timeout.
pub fn on_tell_back_timeout(headlamp_ctx: &HeadlampContext, wait: TellBackWait) -> TellBackTimeoutOutcome {
    if wait.retries_remaining > 0 {
        TellBackTimeoutOutcome::Retry(TellBackWait {
            turn_id: wait.turn_id,
            tell_attempt: wait.tell_attempt.saturating_add(1),
            retries_remaining: wait.retries_remaining - 1,
        })
    } else {
        TellBackTimeoutOutcome::Exhausted(synthetic_unresponsive_headlamp_reply(headlamp_ctx))
    }
}

/// Synthetic embed when the headlamp twinlet never tell-backed (ledger-visible warning).
pub fn synthetic_unresponsive_headlamp_reply(headlamp_ctx: &HeadlampContext) -> HeadlampZoneReply {
    HeadlampZoneReply {
        ctx: headlamp_ctx.clone(),
        outcomes: vec![HeadlampOutcome::LogWarning(format!(
            "headlamp tell-back unresponsive after {ZONE_TELL_BACK_ATTEMPT_COUNT} tell attempts"
        ))],
    }
}

/// Synthetic embed when the wiper twinlet never tell-backed (surfaces as a `LogWarning`
/// on the diagnostic stream; wiper has no intermediate actuation state to recover).
pub fn synthetic_unresponsive_wiper_reply(wiper_ctx: &WiperContext) -> WiperZoneReply {
    WiperZoneReply {
        ctx: wiper_ctx.clone(),
        outcomes: vec![WiperOutcome::LogWarning(format!(
            "wiper tell-back unresponsive after {ZONE_TELL_BACK_ATTEMPT_COUNT} tell attempts"
        ))],
    }
}
