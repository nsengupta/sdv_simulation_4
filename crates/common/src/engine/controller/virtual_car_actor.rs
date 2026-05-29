//! Virtual ECU / gateway **actor** ([`ractor::Actor`](https://crates.io/crates/ractor/0.15.12)).
//!
//! ## Message layering
//! - **[`FsmEvent`](crate::fsm::FsmEvent)** — pure FSM vocabulary: `Clone`, no I/O ports.
//! - **[`DigitalTwinCarVocabulary`](crate::digital_twin::DigitalTwinCarVocabulary)** — full mailbox:
//!   wraps [`FsmEvent`](crate::fsm::FsmEvent) via [`DigitalTwinCarVocabulary::Fsm`] plus
//!   request/reply such as [`DigitalTwinCarVocabulary::GetStatus`] ([`RpcReplyPort`]).

use async_trait::async_trait;
use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort};
use std::sync::Arc;

use crate::diagnostic::{DiagnosticMessage, DiagnosticSink, TokioMpscDiagnosticSink, diag_state_transition, diag_timer_tick, diag_actuation_failure, diag_transition_sink_full, diag_transition_sink_closed};
use crate::digital_twin::{DigitalTwinCar, DigitalTwinCarVocabulary};
use crate::engine::controller::actuation_manager::{
    ActuationManager, DefaultActuationManager,
};
use crate::engine::controller::vehicle_controller::VehicleControllerRuntimeOptions;
use crate::fsm::{self, ActorModeHintFromDomain, DomainAction, FsmEvent, FsmState, VehicleContext};
use crate::transition_sink::{PublishedTransitionRecord, TokioMpscTransitionRecordSink, TransitionRecordSink, TransitionSinkError};

/// The Digital Twin Actor
pub struct VirtualCarActor;

#[derive(Debug, Clone)]
pub struct VirtualCarActorArgs {
    pub identity: String,
    pub runtime_options: VehicleControllerRuntimeOptions,
}

impl From<String> for VirtualCarActorArgs {
    fn from(identity: String) -> Self {
        Self {
            identity,
            runtime_options: VehicleControllerRuntimeOptions::default(),
        }
    }
}

impl From<&str> for VirtualCarActorArgs {
    fn from(identity: &str) -> Self {
        Self::from(identity.to_string())
    }
}

pub struct VirtualCarRuntimeState {
    twin_car: DigitalTwinCar,
    next_sequence_no: u64,
    runtime_options: VehicleControllerRuntimeOptions,
    actuation_manager: Arc<dyn ActuationManager>,
    diagnostic_sink: Option<Arc<dyn DiagnosticSink>>,
    transition_sink: Option<Arc<dyn TransitionRecordSink>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActorMode {
    Normal,
    Transitioning,
}

impl Default for VirtualCarActor {
    fn default() -> Self {
        Self
    }
}

impl VirtualCarActor {
    #[allow(dead_code)]
    pub fn with_transition_sink(_transition_sink: Arc<dyn TransitionRecordSink>) -> Self {
        Self
    }
}

#[async_trait]
impl Actor for VirtualCarActor {
    type Msg = DigitalTwinCarVocabulary;
    type State = VirtualCarRuntimeState;
    type Arguments = VirtualCarActorArgs;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        let identity = args.identity.clone();

        // Wrap optional diagnostic TX channel into a DiagnosticSink.
        let diagnostic_sink: Option<Arc<dyn DiagnosticSink>> = args
            .runtime_options
            .diagnostic_tx
            .clone()
            .map(|tx| Arc::new(TokioMpscDiagnosticSink::new(tx)) as Arc<dyn DiagnosticSink>);

        // Wrap optional transition TX channel into a TransitionRecordSink.
        let transition_sink: Option<Arc<dyn TransitionRecordSink>> = args
            .runtime_options
            .transition_tx
            .clone()
            .map(|tx| Arc::new(TokioMpscTransitionRecordSink::new(tx)) as Arc<dyn TransitionRecordSink>);

        // Emit init message if sink is available.
        if let Some(sink) = &diagnostic_sink {
            let _ = sink.try_emit(DiagnosticMessage::info(
                "VirtualCarActor",
                format!("Physical Car name: {identity}, initializing its Digital Twin ..."),
            ));
        }

        let actuation_manager: Arc<dyn ActuationManager> =
            if let Some(tx) = args.runtime_options.actuation_command_tx.clone() {
                let session_id = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0);
                let manager = DefaultActuationManager::with_command_channel(
                    identity.clone(),
                    session_id,
                    tx,
                );
                Arc::new(manager)
            } else {
                let manager = DefaultActuationManager::default();
                Arc::new(manager)
            };

        Ok(VirtualCarRuntimeState {
            twin_car: DigitalTwinCar {
                identity,
                current_state: FsmState::Off,
                context: VehicleContext::default(),
            },
            next_sequence_no: 1,
            runtime_options: args.runtime_options,
            actuation_manager,
            diagnostic_sink,
            transition_sink,
        })
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        runtime_state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        use DigitalTwinCarVocabulary::{Fsm, GetStatus};

        match message {
            Fsm(evt) => {
                if matches!(evt, FsmEvent::TimerTick) && runtime_state.runtime_options.log_timer_tick {
                    // TODO: rate-limit once structured logging is introduced.
                    if let Some(sink) = &runtime_state.diagnostic_sink {
                        let _ = sink.try_emit(diag_timer_tick(&runtime_state.twin_car.identity));
                    }
                }
                let result =
                    fsm::step(&runtime_state.twin_car.current_state, &runtime_state.twin_car.context, &evt, std::time::Instant::now());
                let old_state = runtime_state.twin_car.current_state.clone();
                let mut mode = ActorMode::Normal;

                // Persist actor state first (non-negotiable ordering before transition log emit).
                runtime_state.twin_car.current_state = result.next_state.clone();
                runtime_state.twin_car.context = result.modified_ctx;

                Self::try_emit_transition_record(runtime_state, result.transition_record);

                for action in result.actions {
                    match action {
                        DomainAction::EnterMode(hint) => {
                            mode = match hint {
                                ActorModeHintFromDomain::Normal => ActorMode::Normal,
                                ActorModeHintFromDomain::Transitioning => ActorMode::Transitioning,
                            };
                        }
                        other_action => {
                            if let Err(err) = runtime_state
                                .actuation_manager
                                .execute(&other_action, &runtime_state.twin_car)
                                .await
                            {
                                if let Some(sink) = &runtime_state.diagnostic_sink {
                                    let _ = sink.try_emit(diag_actuation_failure(
                                        &runtime_state.twin_car.identity,
                                        &format!("{:?}", other_action),
                                        &format!("{:?}", err),
                                    ));
                                }
                            }
                        }
                    }
                }

                if runtime_state.twin_car.current_state != old_state {
                    if let Some(sink) = &runtime_state.diagnostic_sink {
                        let _ = sink.try_emit(diag_state_transition(
                            &runtime_state.twin_car.identity,
                            &runtime_state.twin_car.current_state,
                        ));
                    }
                }
                let _ = mode;
                Ok(())
            }
            GetStatus(reply) => Self::reply_get_status(reply, &runtime_state.twin_car),
        }
    }
}

impl VirtualCarActor {
    fn try_emit_transition_record(
        runtime_state: &mut VirtualCarRuntimeState,
        transition_record: fsm::RawTransitionRecord,
    ) {
        let Some(sink) = &runtime_state.transition_sink else {
            return;
        };

        let sequence_no = runtime_state.next_sequence_no;
        runtime_state.next_sequence_no = runtime_state.next_sequence_no.saturating_add(1);

        let raw = PublishedTransitionRecord {
            car_identity: runtime_state.twin_car.identity.clone(),
            sequence_no,
            transition: transition_record,
        };

        if let Err(err) = sink.try_emit(raw) {
            let diag_sink = &runtime_state.diagnostic_sink;
            match err {
                TransitionSinkError::Full => {
                    if let Some(sink) = diag_sink {
                        let _ = sink.try_emit(diag_transition_sink_full(&runtime_state.twin_car.identity));
                    }
                }
                TransitionSinkError::Closed => {
                    if let Some(sink) = diag_sink {
                        let _ = sink.try_emit(diag_transition_sink_closed(&runtime_state.twin_car.identity));
                    }
                }
            }
        }
    }

    fn reply_get_status(
        reply: RpcReplyPort<DigitalTwinCar>,
        twin_car: &DigitalTwinCar,
    ) -> Result<(), ActorProcessingErr> {
        if reply.is_closed() {
            return Ok(());
        }
        reply
            .send(twin_car.clone())
            .map_err(|e| std::io::Error::other(format!("GetStatus reply: {e:?}")))?;
        Ok(())
    }
}
