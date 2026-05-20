use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, Ordering};
use time::{OffsetDateTime, UtcOffset, macros::format_description};

use crate::digital_twin::DigitalTwinCar;
use crate::domain_types::VehicleState;
use crate::engine::controller::{ActuationCommand, CorrelationId};
use crate::front_headlamp_log::{CMD_OFF, CMD_ON, MSG_REQUEST_OFF, MSG_REQUEST_ON};
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

#[derive(Debug)]
pub struct DefaultActuationManager {
    source_id: Option<String>,
    session_id: u64,
    next_sequence_no: AtomicU64,
    actuation_command_tx: Option<tokio::sync::mpsc::Sender<ActuationCommand>>,
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
        twin: &DigitalTwinCar,
    ) -> Result<(), ActuationError> {
        match action {
            DomainAction::StartBuzzer => {
                // TODO(actuation-child-actor): offload connector I/O to a child actor
                // and keep parent actor loop non-blocking under slow transports.
                println!(
                    "[ACTION @ {}]: 🔊 BUZZER ON - High Stress Detected!",
                    action_timestamp()
                );
            }
            DomainAction::StopBuzzer => {
                // TODO(actuation-child-actor): offload connector I/O to a child actor
                // and keep parent actor loop non-blocking under slow transports.
                println!(
                    "[ACTION @ {}]: 🔇 BUZZER OFF - System Normal.",
                    action_timestamp()
                );
            }
            DomainAction::PublishStateSync => {
                // TODO(actuation-egress): publish through an injected egress connector
                // (CAN/Zenoh/uProtocol) instead of default stdout logging.
                let public_state = VehicleState::from(&twin.current_state);
                println!(
                    "[ACTION @ {}]: 📡 Publishing to Cloud: {:?}",
                    action_timestamp(),
                    public_state
                );
            }
            DomainAction::LogWarning(msg) => {
                // TODO(actuation-observability): route structured warnings to an
                // injected logging/event sink.
                eprintln!("[ALERT @ {}]: {}", action_timestamp(), msg);
            }
            DomainAction::RequestFrontHeadlampOn => {
                // TODO(actuation-child-actor): move actuator command execution to a
                // dedicated actuation child actor for robust ordering/backpressure.
                println!(
                    "[ACTION @ {}]: {CMD_ON} {MSG_REQUEST_ON}",
                    action_timestamp()
                );
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
                println!(
                    "[ACTION @ {}]: {CMD_OFF} {MSG_REQUEST_OFF}",
                    action_timestamp()
                );
                if let (Some(tx), Some(correlation_id)) =
                    (&self.actuation_command_tx, self.next_correlation_id())
                {
                    let _ = tx
                        .send(ActuationCommand::SwitchFrontHeadlampOff { correlation_id })
                        .await;
                }
            }
            DomainAction::EnterMode(_) => {}
        }

        Ok(())
    }
}

fn action_timestamp() -> String {
    let now = OffsetDateTime::now_utc().to_offset(UtcOffset::UTC);
    let hms = now
        .format(format_description!("[hour]:[minute]:[second]"))
        .unwrap_or_else(|_| "00:00:00".to_string());
    format!("{hms} {:09}", now.nanosecond())
}
