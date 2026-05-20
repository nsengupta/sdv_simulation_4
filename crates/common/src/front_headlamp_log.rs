//! Front-headlamp log icons and message text for gateway / actuation manager output.
//!
//! Import icons and messages as constants, e.g.
//! `use crate::front_headlamp_log::{CMD_ON, MSG_REQUEST_ON};`
//! then `println!("[ACTION]: {CMD_ON} {MSG_REQUEST_ON}");`

use crate::fsm::{FrontHeadlampIncompleteCause, FrontHeadlampSwitchDirection};

// --- Icons (emoji pairs) ---

pub const CMD_ON: &str = "📤🔆";
pub const CMD_OFF: &str = "📤🌑";
pub const ACK_ON: &str = "✅💡";
pub const ACK_OFF: &str = "✅🌑";
pub const NACK_ON: &str = "❌🔆";
pub const NACK_OFF: &str = "❌🌑";
pub const TIMEOUT_ON: &str = "⏱️💡";
pub const TIMEOUT_OFF: &str = "⏱️🌑";

// --- Message text (no icons; pair with an icon constant in log lines) ---

pub const MSG_REQUEST_ON: &str = "Requesting front headlamp ON.";
pub const MSG_REQUEST_OFF: &str = "Requesting front headlamp OFF.";

pub const MSG_ACK_ON: &str = "Front headlamp ON confirmed.";
pub const MSG_ACK_OFF: &str = "Front headlamp OFF confirmed.";

pub const MSG_NACK_ON: &str = "Front headlamp ON rejected (NACK).";
pub const MSG_NACK_OFF: &str = "Front headlamp OFF rejected (NACK).";

pub const MSG_TIMEOUT_ON: &str = "Front headlamp ON request — no actuator response (timed out).";
pub const MSG_TIMEOUT_OFF: &str = "Front headlamp OFF request — no actuator response (timed out).";

/// Full `[ALERT]` line for incomplete actuation (timeout or NACK).
pub fn alert_incomplete(
    direction: FrontHeadlampSwitchDirection,
    cause: FrontHeadlampIncompleteCause,
) -> String {
    let (icon, msg) = match (direction, cause) {
        (FrontHeadlampSwitchDirection::On, FrontHeadlampIncompleteCause::TimedOut) => {
            (TIMEOUT_ON, MSG_TIMEOUT_ON)
        }
        (FrontHeadlampSwitchDirection::Off, FrontHeadlampIncompleteCause::TimedOut) => {
            (TIMEOUT_OFF, MSG_TIMEOUT_OFF)
        }
        (FrontHeadlampSwitchDirection::On, FrontHeadlampIncompleteCause::NegativeAck) => {
            (NACK_ON, MSG_NACK_ON)
        }
        (FrontHeadlampSwitchDirection::Off, FrontHeadlampIncompleteCause::NegativeAck) => {
            (NACK_OFF, MSG_NACK_OFF)
        }
        #[allow(unreachable_patterns)]
        (FrontHeadlampSwitchDirection::On, _) => (TIMEOUT_ON, MSG_TIMEOUT_ON),
        #[allow(unreachable_patterns)]
        (FrontHeadlampSwitchDirection::Off, _) => (TIMEOUT_OFF, MSG_TIMEOUT_OFF),
    };
    format!("{icon} {msg}")
}
