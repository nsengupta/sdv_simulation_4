//! Transition-log sink abstraction.
//!
//! Actor runtime emits raw transition records through this interface. Any
//! formatting, enrichment, persistence, or transport mapping must happen in sink
//! implementations / receivers, not in the actor.

use crate::fsm::RawTransitionRecord;
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq)]
pub struct PublishedTransitionRecord {
    pub car_identity: String,
    pub sequence_no: u64,
    pub transition: RawTransitionRecord,
}

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
