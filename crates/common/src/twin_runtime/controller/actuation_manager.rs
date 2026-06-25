use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::digital_twin::DigitalTwinCar;
use crate::twin_runtime::controller::{ActuationCommand, CorrelationId};
use crate::fsm::DomainAction;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActuationError {
    UnsupportedAction(&'static str),
}

#[async_trait]
pub trait ActuationManager: Send + Sync {
    async fn execute(
        &self,
        action: &DomainAction,
        twin: &DigitalTwinCar,
    ) -> Result<(), ActuationError>;
}

pub struct DefaultActuationManager {
    source_id: Option<String>,
    session_id: u64,
    next_sequence_no: AtomicU64,
    actuation_command_tx: Option<tokio::sync::mpsc::Sender<ActuationCommand>>,
}

impl std::fmt::Debug for DefaultActuationManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultActuationManager")
            .field("source_id", &self.source_id)
            .field("session_id", &self.session_id)
            .field("next_sequence_no", &self.next_sequence_no)
            .field("actuation_command_tx", &self.actuation_command_tx)
            .finish()
    }
}

impl DefaultActuationManager {
    pub fn with_command_channel(
        source_id: String,
        session_id: u64,
        actuation_command_tx: tokio::sync::mpsc::Sender<ActuationCommand>,
    ) -> Self {
        Self {
            source_id: Some(source_id),
            session_id,
            next_sequence_no: AtomicU64::new(1),
            actuation_command_tx: Some(actuation_command_tx),
        }
    }

    fn next_correlation_id(&self) -> Option<CorrelationId> {
        let source_id = self.source_id.as_ref()?.clone();
        let sequence_no = self.next_sequence_no.fetch_add(1, Ordering::Relaxed);
        Some(CorrelationId {
            source_id,
            session_id: self.session_id,
            sequence_no,
        })
    }
}

impl Default for DefaultActuationManager {
    fn default() -> Self {
        Self {
            source_id: None,
            session_id: 0,
            next_sequence_no: AtomicU64::new(1),
            actuation_command_tx: None,
        }
    }
}

#[async_trait]
impl ActuationManager for DefaultActuationManager {
    async fn execute(
        &self,
        action: &DomainAction,
        _twin: &DigitalTwinCar,
    ) -> Result<(), ActuationError> {
        match action {
            DomainAction::StartBuzzer => {
                // TODO(actuation-child-actor): offload connector I/O to a child actor
                // and keep parent actor loop non-blocking under slow transports.
            }
            DomainAction::StopBuzzer => {
                // TODO(actuation-child-actor): offload connector I/O to a child actor
                // and keep parent actor loop non-blocking under slow transports.
            }
            DomainAction::PublishStateSync => {
                // TODO(actuation-egress): publish through an injected egress connector
                // (CAN/Zenoh/uProtocol) instead of default stdout logging.
            }
            DomainAction::LogWarning(_msg) => {
                // No-op: LogWarning is observability, not actuation. The actor routes it to
                // the diagnostic sink (WI-5 / Q5), so it never reaches here in practice; this
                // arm exists only for `DomainAction` match exhaustiveness.
            }
            DomainAction::RequestFrontHeadlampOn => {
                // TODO(actuation-child-actor): move actuator command execution to a
                // dedicated actuation child actor for robust ordering/backpressure.
                if let (Some(tx), Some(correlation_id)) =
                    (&self.actuation_command_tx, self.next_correlation_id())
                {
                    let _ = tx
                        .send(ActuationCommand::SwitchFrontHeadlampOn { correlation_id })
                        .await;
                }
            }
            DomainAction::RequestFrontHeadlampOff => {
                // TODO(actuation-child-actor): move actuator command execution to a
                // dedicated actuation child actor for robust ordering/backpressure.
                if let (Some(tx), Some(correlation_id)) =
                    (&self.actuation_command_tx, self.next_correlation_id())
                {
                    let _ = tx
                        .send(ActuationCommand::SwitchFrontHeadlampOff { correlation_id })
                        .await;
                }
            }
            DomainAction::RequestWiperStart => {
                if let Some(tx) = &self.actuation_command_tx {
                    let _ = tx.send(ActuationCommand::StartWiper).await;
                }
            }
            DomainAction::RequestWiperStop => {
                if let Some(tx) = &self.actuation_command_tx {
                    let _ = tx.send(ActuationCommand::StopWiper).await;
                }
            }
            // StartAssemblies / StopAssemblies are intercepted by `apply_committed_quiescence`
            // in `virtual_car_actor.rs` before they reach the actuation manager, so this arm
            // is unreachable in production.  It remains for `DomainAction` match exhaustiveness.
            DomainAction::StartAssemblies(_) | DomainAction::StopAssemblies(_) => {}
        }

        Ok(())
    }
}
