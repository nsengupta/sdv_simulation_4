use crate::digital_twin::{CarSnapshot, DigitalTwinCarVocabulary};
use crate::twin_runtime::controller::actuation_contract::ActuationCommand;
use crate::twin_runtime::connectors::{PhysicalToDigitalProjector, Projector};
use crate::fsm::FsmEvent;
use crate::transition_sink::PublishedTransitionRecord;
use crate::PhysicalCarVocabulary;
use ractor::rpc::CallResult;
use ractor::{ActorRef, MessagingErr, SpawnErr};
use super::virtual_car_actor::{VirtualCarActor, VirtualCarActorArgs};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct VehicleController {
    actor: ActorRef<DigitalTwinCarVocabulary>,
    projector: PhysicalToDigitalProjector,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VehicleControllerError {
    Projection(String),
    Messaging(String),
    Timeout,
    ReplyDropped,
}

#[derive(Debug, Clone)]
pub struct VehicleControllerRuntimeOptions {
    pub log_timer_tick: bool,
    pub actuation_command_tx: Option<tokio::sync::mpsc::Sender<ActuationCommand>>,
    pub diagnostic_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::diagnostic::DiagnosticMessage>>,
    pub transition_tx: Option<tokio::sync::mpsc::Sender<PublishedTransitionRecord>>,
    /// Contract tests: headlamp twinlet ignores tells (exercises tell-back timeout path).
    #[doc(hidden)]
    pub test_silent_headlamp: bool,
}

impl Default for VehicleControllerRuntimeOptions {
    fn default() -> Self {
        Self {
            log_timer_tick: false,
            actuation_command_tx: None,
            diagnostic_tx: None,
            transition_tx: None,
            test_silent_headlamp: false,
        }
    }
}

impl VehicleController {
    pub async fn install_and_start(
        identity: String,
    ) -> Result<(Self, ractor::concurrency::JoinHandle<()>), SpawnErr> {
        Self::install_and_start_with_options(identity, VehicleControllerRuntimeOptions::default())
            .await
    }

    pub async fn install_and_start_with_options(
        identity: String,
        runtime_options: VehicleControllerRuntimeOptions,
    ) -> Result<(Self, ractor::concurrency::JoinHandle<()>), SpawnErr> {
        let args = VirtualCarActorArgs {
            identity,
            runtime_options,
        };
        let (actor, handle) = ractor::spawn::<VirtualCarActor>(args).await?;
        Ok((Self::new(actor), handle))
    }

    pub fn new(actor: ActorRef<DigitalTwinCarVocabulary>) -> Self {
        Self {
            actor,
            projector: PhysicalToDigitalProjector,
        }
    }

    /// Expose the underlying actor reference for direct message access (used in tests).
    pub fn get_actor_ref(&self) -> &ActorRef<DigitalTwinCarVocabulary> {
        &self.actor
    }

    /// Lifecycle: primary FSM enters powered operation (not representable as `PhysicalCarVocabulary` today).
    pub async fn send_power_on(&self) -> Result<(), VehicleControllerError> {
        self.actor
            .send_message(FsmEvent::PowerOn.into())
            .map_err(|e| VehicleControllerError::Messaging(format!("{e}")))?;
        Ok(())
    }

    /// Lifecycle: request primary FSM shutdown to `Off` when legal (`Idle` → `Off` in current rules).
    ///
    /// From non-`Idle` powered states the FSM rejects `PowerOff` (see strategy); the message is still delivered.
    pub async fn send_power_off(&self) -> Result<(), VehicleControllerError> {
        self.actor
            .send_message(FsmEvent::PowerOff.into())
            .map_err(|e| VehicleControllerError::Messaging(format!("{e}")))?;
        Ok(())
    }

    /// Public ingress path: physical car vocabulary enters via projector boundary.
    pub async fn submit_physical_car_event(
        &self,
        event: PhysicalCarVocabulary,
    ) -> Result<(), VehicleControllerError> {
        let msg = self
            .projector
            .project(event)
            .map_err(|e| VehicleControllerError::Projection(format!("{e:?}")))?;
        self.actor
            .send_message(msg)
            .map_err(|e| VehicleControllerError::Messaging(format!("{e}")))?;
        Ok(())
    }

    /// Internal/testing bypass for already-derived FSM events.
    #[allow(dead_code)]
    pub(crate) async fn submit_fsm_event(
        &self,
        event: FsmEvent,
    ) -> Result<(), VehicleControllerError> {
        self.actor
            .send_message(event.into())
            .map_err(|e| VehicleControllerError::Messaging(format!("{e}")))?;
        Ok(())
    }

    /// Read-only snapshot API for external observers. The returned [`CarSnapshot`] is stamped with
    /// `as_of_seq` — the ledger sequence of the last FSM event it reflects (Q3 / WI-4) — so callers
    /// can reason about staleness and reconcile against the transition ledger.
    pub async fn get_snapshot(
        &self,
        timeout: Option<Duration>,
    ) -> Result<CarSnapshot, VehicleControllerError> {
        let result: Result<CallResult<CarSnapshot>, MessagingErr<DigitalTwinCarVocabulary>> =
            self.actor
                .call(|port| DigitalTwinCarVocabulary::GetStatus(port), timeout)
                .await;

        match result.map_err(|e| VehicleControllerError::Messaging(format!("{e}")))? {
            CallResult::Success(snapshot) => Ok(snapshot),
            CallResult::Timeout => Err(VehicleControllerError::Timeout),
            CallResult::SenderError => Err(VehicleControllerError::ReplyDropped),
        }
    }
}
