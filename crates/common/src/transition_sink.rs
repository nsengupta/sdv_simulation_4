//! Transition-log sink abstraction.
//!
//! The actor projects each pure [`RawTransitionRecord`](crate::fsm::RawTransitionRecord) into a
//! serializable, `Instant`-free [`PublishedTransitionRecord`] (see [`crate::published`]) and emits
//! it through this interface. Any further formatting, enrichment, persistence, or transport
//! mapping happens in sink implementations / receivers, not in the actor.

pub use crate::published::PublishedTransitionRecord;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionSinkError {
    Full,
    Closed,
}

pub trait TransitionRecordSink: Send + Sync {
    fn try_emit(&self, record: PublishedTransitionRecord) -> Result<(), TransitionSinkError>;
}

#[derive(Clone)]
pub struct TokioMpscTransitionRecordSink {
    tx: mpsc::Sender<PublishedTransitionRecord>,
}

impl TokioMpscTransitionRecordSink {
    pub fn new(tx: mpsc::Sender<PublishedTransitionRecord>) -> Self {
        Self { tx }
    }
}

impl TransitionRecordSink for TokioMpscTransitionRecordSink {
    fn try_emit(&self, record: PublishedTransitionRecord) -> Result<(), TransitionSinkError> {
        match self.tx.try_send(record) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(TransitionSinkError::Full),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(TransitionSinkError::Closed),
        }
    }
}
