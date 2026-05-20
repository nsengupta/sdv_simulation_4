//! Shared actuation command/feedback contracts.
//!
//! These types are intentionally runtime-agnostic so the same message model can
//! be used with in-process gateway workers, child actors, or remote transports.
//!
//! ## Vocabulary by boundary
//!
//! | Layer | Types | Terms | Role |
//! |-------|--------|-------|------|
//! | **Actor / controller runtime** | [`ActuationCommand`], [`ActuationFeedback`], [`FsmEvent`] | **ACK / NACK** | What the twin and actuation port speak |
//! | **Physical ingress** ([`PhysicalCarVocabulary`]) | `FrontHeadlampCommandConfirmed`, `FrontHeadlampCommandRejected` | **Confirmed / Rejected** | Bus decode → semantic outcome before projection |
//!
//! [`ActuationFeedback`] is **actor-side**: correlated responses on the actuation port
//! (future channel or child-actor path). Gateway CAN ingress today maps wire ACK/NACK into
//! physical **Confirmed/Rejected**, then projection maps those into [`FsmEvent::FrontHeadlampOnAck`]
//! / `OffAck` or `ActuationIncomplete { cause: NegativeAck }` — not into [`ActuationFeedback`] yet.

/// Correlation identity for command <-> feedback flows.
///
/// Use scoped identity (`source_id`, `session_id`, `sequence_no`) instead of a
/// single global counter to avoid uniqueness issues across restarts/processes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CorrelationId {
    pub source_id: String,
    pub session_id: u64,
    pub sequence_no: u64,
}

/// Outbound actuation intent emitted by controller runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActuationCommand {
    SwitchFrontHeadlampOn { correlation_id: CorrelationId },
    SwitchFrontHeadlampOff { correlation_id: CorrelationId },
}

/// Inbound actuation feedback on the controller actuation port (ACK/NACK, correlated).
///
/// Timeouts and “no response” are not represented here; the FSM infers those from
/// [`FsmEvent::TimerTick`] and [`crate::fsm::FrontHeadlampIncompleteCause::TimedOut`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActuationFeedback {
    FrontHeadlampOnAck { correlation_id: CorrelationId },
    FrontHeadlampOffAck { correlation_id: CorrelationId },
    FrontHeadlampOnNack { correlation_id: CorrelationId },
    FrontHeadlampOffNack { correlation_id: CorrelationId },
}
