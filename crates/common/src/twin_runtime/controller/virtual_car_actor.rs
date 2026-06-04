//! Virtual ECU / gateway **actor** ([`ractor::Actor`](https://crates.io/crates/ractor/0.15.12)).
//!
//! ## Message layering
//! - **[`FsmEvent`](crate::fsm::FsmEvent)** — pure FSM vocabulary: `Clone`, no I/O ports.
//! - **[`DigitalTwinCarVocabulary`](crate::digital_twin::DigitalTwinCarVocabulary)** — full mailbox:
//!   wraps [`FsmEvent`](crate::fsm::FsmEvent) via [`DigitalTwinCarVocabulary::Fsm`] plus
//!   request/reply such as [`DigitalTwinCarVocabulary::GetStatus`] ([`RpcReplyPort`]).

use async_trait::async_trait;
use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use crate::diagnostic::{DiagnosticMessage, DiagnosticSink, TokioMpscDiagnosticSink, diag_front_headlamp_confirmed, diag_state_transition, diag_timer_tick, diag_actuation_failure, diag_warning, diag_transition_sink_full, diag_transition_sink_closed};
use crate::digital_twin::{CarSnapshot, DigitalTwinCar, DigitalTwinCarVocabulary};
use crate::twin_runtime::controller::actuation_manager::{
    ActuationManager, DefaultActuationManager,
};
use crate::twin_runtime::controller::vehicle_controller::VehicleControllerRuntimeOptions;
use crate::fsm::{
    self, ActorModeHintFromDomain, DomainAction, FrontHeadlampSwitchDirection, FsmEvent, FsmState,
    HeadlampState, StepResult,
};
use crate::twin_runtime::headlamp_actor::{tell_headlamp_zone, HeadlampActor, HeadlampActorVocabulary};
use crate::twin_runtime::twin_turn::{commit_brain_turn, fsm_step_lands_off};
use crate::twin_runtime::zone_turn::fsm_event_headlamp_message;
use crate::vehicle_state::{HeadlampMessage, HeadlampZoneReply, VehicleContext};
use crate::published::{PublishedTransitionRecord, SessionEpoch};
use crate::transition_sink::{TokioMpscTransitionRecordSink, TransitionRecordSink, TransitionSinkError};

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

/// One FSM event awaiting headlamp tell-back(s) before [`commit_brain_turn`].
#[derive(Debug, Clone)]
enum PendingBrainTurn {
    /// Event's primary zone message was told; waiting for embed.
    PrimaryHeadlamp {
        turn_id: u64,
        event: FsmEvent,
        now: Instant,
    },
    /// Primary embed received (or skipped); waiting for ignition-off reset tell-back.
    IgnitionOffReset {
        turn_id: u64,
        event: FsmEvent,
        now: Instant,
        headlamp_reply: Option<HeadlampZoneReply>,
    },
}

pub struct VirtualCarRuntimeState {
    twin_car: DigitalTwinCar,
    headlamp_actor: ActorRef<HeadlampActorVocabulary>,
    next_turn_id: u64,
    pending_turn: Option<PendingBrainTurn>,
    fsm_backlog: VecDeque<(FsmEvent, Instant)>,
    next_record_seq: u64,
    /// Monotonic↔wall anchor for this run; the sole source of wall-clock stamps on published
    /// records and of the actuation `session_id`.
    session_epoch: SessionEpoch,
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

        // One wall-clock read per run: this anchor stamps published records *and* seeds the
        // actuation session id, so both share a single, consistent epoch.
        let session_epoch = SessionEpoch::capture();

        let actuation_manager: Arc<dyn ActuationManager> =
            if let Some(tx) = args.runtime_options.actuation_command_tx.clone() {
                let session_id = session_epoch.session_id_nanos() as u64;
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

        let initial_ctx = VehicleContext::default();
        let (headlamp_actor, _) =
            ractor::spawn::<HeadlampActor>(initial_ctx.headlamp.clone()).await?;

        Ok(VirtualCarRuntimeState {
            twin_car: DigitalTwinCar::new(identity, FsmState::Off, initial_ctx)?,
            headlamp_actor,
            next_turn_id: 1,
            pending_turn: None,
            fsm_backlog: VecDeque::new(),
            next_record_seq: 1,
            session_epoch,
            runtime_options: args.runtime_options,
            actuation_manager,
            diagnostic_sink,
            transition_sink,
        })
    }

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        runtime_state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        use DigitalTwinCarVocabulary::{Fsm, GetStatus, HeadlampZoneReady};

        match message {
            Fsm(evt) => {
                if matches!(evt, FsmEvent::TimerTick) && runtime_state.runtime_options.log_timer_tick {
                    // TODO: rate-limit once structured logging is introduced.
                    if let Some(sink) = &runtime_state.diagnostic_sink {
                        let _ = sink.try_emit(diag_timer_tick(runtime_state.twin_car.identity()));
                    }
                }
                let now = Instant::now();
                if runtime_state.pending_turn.is_some() {
                    runtime_state.fsm_backlog.push_back((evt, now));
                    return Ok(());
                }
                Self::begin_fsm_turn(&myself, runtime_state, evt, now).await?;
                Self::pump_fsm_backlog(&myself, runtime_state).await
            }
            HeadlampZoneReady { turn_id, reply } => {
                Self::on_headlamp_zone_ready(&myself, runtime_state, turn_id, reply).await?;
                Self::pump_fsm_backlog(&myself, runtime_state).await
            }
            GetStatus(reply) => Self::reply_get_status(
                reply,
                &runtime_state.twin_car,
                // as-of: the last event sequence applied so far (0 before any event).
                runtime_state.next_record_seq.saturating_sub(1),
            ),
        }
    }

    async fn post_stop(
        &self,
        _myself: ActorRef<Self::Msg>,
        runtime_state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        // Interim: brain stops the headlamp twinlet here. Target: assembly actors stop before
        // the brain (supervisor-ordered teardown), not brain-owned child stop — see milestone doc.
        runtime_state.headlamp_actor.stop(None);
        Ok(())
    }
}

impl VirtualCarActor {
    async fn begin_fsm_turn(
        brain: &ActorRef<DigitalTwinCarVocabulary>,
        runtime_state: &mut VirtualCarRuntimeState,
        event: FsmEvent,
        now: Instant,
    ) -> Result<(), ActorProcessingErr> {
        let turn_id = runtime_state.next_turn_id;
        runtime_state.next_turn_id = runtime_state.next_turn_id.saturating_add(1);

        if let Some(message) = fsm_event_headlamp_message(&event) {
            tell_headlamp_zone(
                &runtime_state.headlamp_actor,
                &brain,
                turn_id,
                message,
                now,
            )?;
            runtime_state.pending_turn = Some(PendingBrainTurn::PrimaryHeadlamp {
                turn_id,
                event,
                now,
            });
            return Ok(());
        }

        if fsm_step_lands_off(
            runtime_state.twin_car.current_state(),
            runtime_state.twin_car.context(),
            &event,
            now,
            None,
        ) {
            tell_headlamp_zone(
                &runtime_state.headlamp_actor,
                &brain,
                turn_id,
                HeadlampMessage::ResetForIgnitionOff,
                now,
            )?;
            runtime_state.pending_turn = Some(PendingBrainTurn::IgnitionOffReset {
                turn_id,
                event,
                now,
                headlamp_reply: None,
            });
            return Ok(());
        }

        let result = commit_brain_turn(
            runtime_state.twin_car.current_state(),
            runtime_state.twin_car.context(),
            &event,
            now,
            None,
            None,
        );
        Self::apply_committed_turn(runtime_state, &event, result).await
    }

    async fn on_headlamp_zone_ready(
        brain: &ActorRef<DigitalTwinCarVocabulary>,
        runtime_state: &mut VirtualCarRuntimeState,
        turn_id: u64,
        reply: HeadlampZoneReply,
    ) -> Result<(), ActorProcessingErr> {
        let Some(pending) = runtime_state.pending_turn.take() else {
            return Ok(());
        };

        match pending {
            PendingBrainTurn::PrimaryHeadlamp {
                turn_id: expected,
                event,
                now,
            } if expected == turn_id => {
                if fsm_step_lands_off(
                    runtime_state.twin_car.current_state(),
                    runtime_state.twin_car.context(),
                    &event,
                    now,
                    Some(&reply),
                ) {
                    tell_headlamp_zone(
                        &runtime_state.headlamp_actor,
                        &brain,
                        turn_id,
                        HeadlampMessage::ResetForIgnitionOff,
                        now,
                    )?;
                    runtime_state.pending_turn = Some(PendingBrainTurn::IgnitionOffReset {
                        turn_id,
                        event,
                        now,
                        headlamp_reply: Some(reply),
                    });
                    return Ok(());
                }
                let result = commit_brain_turn(
                    runtime_state.twin_car.current_state(),
                    runtime_state.twin_car.context(),
                    &event,
                    now,
                    Some(reply),
                    None,
                );
                Self::apply_committed_turn(runtime_state, &event, result).await?;
            }
            PendingBrainTurn::IgnitionOffReset {
                turn_id: expected,
                event,
                now,
                headlamp_reply,
            } if expected == turn_id => {
                let result = commit_brain_turn(
                    runtime_state.twin_car.current_state(),
                    runtime_state.twin_car.context(),
                    &event,
                    now,
                    headlamp_reply,
                    Some(reply),
                );
                Self::apply_committed_turn(runtime_state, &event, result).await?;
            }
            other => {
                runtime_state.pending_turn = Some(other);
            }
        }

        Ok(())
    }

    async fn pump_fsm_backlog(
        brain: &ActorRef<DigitalTwinCarVocabulary>,
        runtime_state: &mut VirtualCarRuntimeState,
    ) -> Result<(), ActorProcessingErr> {
        while runtime_state.pending_turn.is_none() {
            let Some((evt, now)) = runtime_state.fsm_backlog.pop_front() else {
                break;
            };
            Self::begin_fsm_turn(brain, runtime_state, evt, now).await?;
        }
        Ok(())
    }

    async fn apply_committed_turn(
        runtime_state: &mut VirtualCarRuntimeState,
        event: &FsmEvent,
        result: StepResult,
    ) -> Result<(), ActorProcessingErr> {
        let old_state = runtime_state.twin_car.current_state().clone();
        let headlamp_before = runtime_state.twin_car.context().headlamp.state;
        let headlamp_after = result.modified_ctx.headlamp.state;
        let mut mode = ActorMode::Normal;

        let record_seq = runtime_state.next_record_seq;
        runtime_state.next_record_seq = runtime_state.next_record_seq.saturating_add(1);

        runtime_state.twin_car.apply_step(result.next_state.clone(), result.modified_ctx);

        Self::try_emit_transition_record(runtime_state, record_seq, result.transition_record);

        for action in result.actions {
            match action {
                DomainAction::EnterMode(hint) => {
                    mode = match hint {
                        ActorModeHintFromDomain::Normal => ActorMode::Normal,
                        ActorModeHintFromDomain::Transitioning => ActorMode::Transitioning,
                    };
                }
                DomainAction::LogWarning(message) => {
                    if let Some(sink) = &runtime_state.diagnostic_sink {
                        let _ = sink.try_emit(diag_warning(
                            runtime_state.twin_car.identity(),
                            &message,
                        ));
                    }
                }
                other_action => {
                    if let Err(err) = runtime_state
                        .actuation_manager
                        .execute(&other_action, &runtime_state.twin_car)
                        .await
                    {
                        if let Some(sink) = &runtime_state.diagnostic_sink {
                            let _ = sink.try_emit(diag_actuation_failure(
                                runtime_state.twin_car.identity(),
                                &format!("{:?}", other_action),
                                &format!("{:?}", err),
                            ));
                        }
                    }
                }
            }
        }

        if *runtime_state.twin_car.current_state() != old_state {
            if let Some(sink) = &runtime_state.diagnostic_sink {
                let _ = sink.try_emit(diag_state_transition(
                    runtime_state.twin_car.identity(),
                    runtime_state.twin_car.current_state(),
                    runtime_state.twin_car.context(),
                ));
            }
        }

        if let Some(direction) =
            front_headlamp_confirmed_direction(headlamp_before, headlamp_after)
        {
            if let Some(sink) = &runtime_state.diagnostic_sink {
                let _ = sink.try_emit(diag_front_headlamp_confirmed(
                    runtime_state.twin_car.identity(),
                    direction,
                ));
            }
        }

        let _ = mode;
        let _ = event;
        Ok(())
    }

    fn try_emit_transition_record(
        runtime_state: &mut VirtualCarRuntimeState,
        record_seq: u64,
        transition_record: fsm::RawTransitionRecord,
    ) {
        let Some(sink) = &runtime_state.transition_sink else {
            return;
        };

        let published = PublishedTransitionRecord::project(
            &transition_record,
            runtime_state.twin_car.identity(),
            record_seq,
            &runtime_state.session_epoch,
        );

        if let Err(err) = sink.try_emit(published) {
            let diag_sink = &runtime_state.diagnostic_sink;
            match err {
                TransitionSinkError::Full => {
                    if let Some(sink) = diag_sink {
                        let _ = sink.try_emit(diag_transition_sink_full(runtime_state.twin_car.identity()));
                    }
                }
                TransitionSinkError::Closed => {
                    if let Some(sink) = diag_sink {
                        let _ = sink.try_emit(diag_transition_sink_closed(runtime_state.twin_car.identity()));
                    }
                }
            }
        }
    }

    fn reply_get_status(
        reply: RpcReplyPort<CarSnapshot>,
        twin_car: &DigitalTwinCar,
        as_of_seq: u64,
    ) -> Result<(), ActorProcessingErr> {
        if reply.is_closed() {
            return Ok(());
        }
        reply
            .send(CarSnapshot::new(twin_car.clone(), as_of_seq))
            .map_err(|e| std::io::Error::other(format!("GetStatus reply: {e:?}")))?;
        Ok(())
    }
}

/// Classify a headlamp state change as a positive ACK settle, if any.
fn front_headlamp_confirmed_direction(
    before: HeadlampState,
    after: HeadlampState,
) -> Option<FrontHeadlampSwitchDirection> {
    match (before, after) {
        (HeadlampState::OnRequested, HeadlampState::On) => Some(FrontHeadlampSwitchDirection::On),
        (HeadlampState::OffRequested, HeadlampState::Off) => Some(FrontHeadlampSwitchDirection::Off),
        _ => None,
    }
}
