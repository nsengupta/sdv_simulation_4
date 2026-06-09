//! Virtual ECU / gateway **actor** ([`ractor::Actor`](https://crates.io/crates/ractor/0.15.12)).
//!
//! ## Message layering
//! - **[`FsmEvent`](crate::fsm::FsmEvent)** — pure FSM vocabulary: `Clone`, no I/O ports.
//! - **[`DigitalTwinCarVocabulary`](crate::digital_twin::DigitalTwinCarVocabulary)** — full mailbox:
//!   wraps [`FsmEvent`](crate::fsm::FsmEvent) via [`DigitalTwinCarVocabulary::Fsm`] plus
//!   request/reply such as [`DigitalTwinCarVocabulary::GetStatus`] ([`RpcReplyPort`]).

use async_trait::async_trait;
use ractor::concurrency::{Duration as RactorDuration, JoinHandle};
use ractor::{Actor, ActorProcessingErr, ActorRef, MessagingErr, RpcReplyPort};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use crate::diagnostic::{DiagnosticMessage, DiagnosticSink, TokioMpscDiagnosticSink, diag_front_headlamp_confirmed, diag_state_transition, diag_timer_tick, diag_actuation_failure, diag_warning, diag_transition_sink_full, diag_transition_sink_closed};
use crate::digital_twin::{CarSnapshot, DigitalTwinCar, DigitalTwinCarVocabulary};
use crate::twin_runtime::constants::ZONE_TELL_BACK_WAIT;
use crate::twin_runtime::controller::actuation_manager::{
    ActuationManager, DefaultActuationManager,
};
use crate::twin_runtime::controller::vehicle_controller::VehicleControllerRuntimeOptions;
use crate::fsm::{
    self, ActorModeHintFromDomain, DomainAction, FrontHeadlampSwitchDirection, FsmEvent, FsmState,
    HeadlampState,
};
use crate::twin_runtime::headlamp_actor::{tell_headlamp_zone, HeadlampActor, HeadlampActorMsg, HeadlampActorState};
use crate::twin_runtime::twin_turn::{commit_resolved_turn as resolve_quiescence, fsm_step_lands_off, ResolvedTurn};
use crate::twin_runtime::ZoneReplies;
use crate::twin_runtime::zone_tell_back::{on_tell_back_timeout, TellBackTimeoutOutcome, TellBackWait};
use crate::twin_runtime::QuiescentResult;
use crate::twin_runtime::zone_turn::fsm_event_headlamp_message;
use crate::vehicle_state::{HeadlampMessage, HeadlampZoneReply, VehicleContext};
use crate::published::{PublishedTransitionRecord, SessionEpoch};
use crate::transition_sink::{TokioMpscTransitionRecordSink, TransitionRecordSink, TransitionSinkError};

type TellBackTimer = JoinHandle<Result<(), MessagingErr<DigitalTwinCarVocabulary>>>;

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

/// One FSM event awaiting headlamp tell-back(s) before commit.
///
/// `PrimaryHeadlamp` waits for the zone twinlet to ACK/NACK the actuation command.
/// If the FSM lands on `Off`, a second `IgnitionOffReset` wait is started to tell the
/// headlamp zone to reset its internal state for the next ignition cycle.
#[derive(Debug)]
enum PendingBrainTurn {
    /// Event's primary zone message was told; waiting for embed.
    PrimaryHeadlamp {
        wait: TellBackWait,
        timer: Option<TellBackTimer>,
        event: FsmEvent,
        now: Instant,
        message: HeadlampMessage,
    },
    /// Primary embed received (or skipped); waiting for ignition-off reset tell-back.
    IgnitionOffReset {
        wait: TellBackWait,
        timer: Option<TellBackTimer>,
        event: FsmEvent,
        now: Instant,
        headlamp_reply: Option<HeadlampZoneReply>,
    },
}

/// Mutable state of the virtual car actor, held across `handle` calls.
pub struct VirtualCarRuntimeState {
    twin_car: DigitalTwinCar,
    headlamp_actor: ActorRef<HeadlampActorMsg>,
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
        let (headlamp_actor, _) = ractor::spawn::<HeadlampActor>(HeadlampActorState::new(
            initial_ctx.headlamp.clone(),
            args.runtime_options.test_silent_headlamp,
        ))
        .await?;

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

    /// Main message dispatch: each variant delegates to a dedicated handler, then
    /// drains the FSM backlog. The `pending_turn` guard serialises turns — new FSM events
    /// arriving while a tell-back is in-flight are buffered in `fsm_backlog` instead of
    /// starting a new turn.
    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        runtime_state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        use DigitalTwinCarVocabulary::{Fsm, GetStatus, HeadlampZoneReady, HeadlampZoneSpontaneous, TellBackTimeout};

        match message {
            Fsm(evt_arrived) => {
                if matches!(evt_arrived, FsmEvent::TimerTick) && runtime_state.runtime_options.log_timer_tick {
                    // TODO: rate-limit once structured logging is introduced.
                    if let Some(sink) = &runtime_state.diagnostic_sink {
                        let _ = sink.try_emit(diag_timer_tick(runtime_state.twin_car.identity()));
                    }
                }
                let now = Instant::now();
                // If a turn is already awaiting tell-back, buffer this event instead of
                // starting a parallel turn — serialises FSM processing.
                if runtime_state.pending_turn.is_some() {
                    // Buffer the event with its arrival timestamp for later draining.
                    runtime_state.fsm_backlog.push_back((evt_arrived, now));
                    return Ok(());
                }
                // No pending turn: start a new FSM turn (assigns turn_id, may begin a
                // headlamp wait or commit directly).
                Self::begin_fsm_turn(&myself, runtime_state, evt_arrived, now).await?;
                // After the new turn consumed (or skipped headlamp wait), drain any
                // backlogged events while no turn is pending.
                Self::pump_fsm_backlog(&myself, runtime_state).await
            }
            HeadlampZoneReady {
                turn_id,
                tell_attempt,
                reply,
            } => {
                Self::on_headlamp_zone_ready(
                    &myself,
                    runtime_state,
                    turn_id,
                    tell_attempt,
                    reply,
                )
                .await?;
                Self::pump_fsm_backlog(&myself, runtime_state).await
            }
            TellBackTimeout {
                turn_id,
                tell_attempt,
            } => {
                Self::on_tell_back_timeout(&myself, runtime_state, turn_id, tell_attempt).await?;
                Self::pump_fsm_backlog(&myself, runtime_state).await
            }
            HeadlampZoneSpontaneous {
                direction,
                cause,
                reply,
            } => {
                Self::on_headlamp_zone_spontaneous(runtime_state, direction, cause, reply).await?;
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

    /// Clean up before actor exit: abort any pending tell-back timer and stop the
    /// headlamp twinlet. Target design: supervisor-ordered teardown of assembly actors
    /// (see milestone doc).
    async fn post_stop(
        &self,
        _myself: ActorRef<Self::Msg>,
        runtime_state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        if let Some(pending) = runtime_state.pending_turn.take() {
            Self::abort_pending_timer(pending);
        }
        // Interim: brain stops the headlamp twinlet here. Target: assembly actors stop before
        // the brain (supervisor-ordered teardown), not brain-owned child stop — see milestone doc.
        runtime_state.headlamp_actor.stop(None);
        Ok(())
    }
}

impl VirtualCarActor {
    /// Abort any active tell-back timer for a pending turn.
    /// Called during shutdown and when re-arming a new timer.
    fn abort_pending_timer(pending: PendingBrainTurn) {
        match pending {
            PendingBrainTurn::PrimaryHeadlamp { mut timer, .. }
            | PendingBrainTurn::IgnitionOffReset { mut timer, .. } => {
                Self::abort_tell_back_timer(&mut timer);
            }
        }
    }

    /// Cancel a tell-back timer if it is still running.
    /// Safe to call when `timer` is `None`.
    fn abort_tell_back_timer(timer: &mut Option<TellBackTimer>) {
        if let Some(handle) = timer.take() {
            handle.abort();
        }
    }

    /// Schedule (or re-schedule) a `TellBackTimeout` message after `ZONE_TELL_BACK_WAIT`.
    /// Cancels any previously running timer for this slot first.
    fn arm_tell_back_timer(
        brain: &ActorRef<DigitalTwinCarVocabulary>,
        wait: TellBackWait,
        timer: &mut Option<TellBackTimer>,
    ) {
        Self::abort_tell_back_timer(timer);
        let turn_id = wait.turn_id;
        let tell_attempt = wait.tell_attempt;
        *timer = Some(brain.send_after(
            RactorDuration::from(ZONE_TELL_BACK_WAIT),
            move || DigitalTwinCarVocabulary::TellBackTimeout {
                turn_id,
                tell_attempt,
            },
        ));
    }

    /// Send a headlamp zone message, arm the tell-back timer, and stash the pending turn.
    ///
    /// Two variants exist: `PrimaryHeadlamp` for the event's own message, and
    /// `IgnitionOffReset` (sent after the primary completes **and** the FSM lands on `Off`).
    async fn begin_headlamp_wait(
        brain: &ActorRef<DigitalTwinCarVocabulary>,
        runtime_state: &mut VirtualCarRuntimeState,
        turn_id: u64,
        message: HeadlampMessage,
        event: FsmEvent,
        now: Instant,
        headlamp_reply: Option<HeadlampZoneReply>,
    ) -> Result<(), ActorProcessingErr> {
        let wait = TellBackWait::new(turn_id);
        tell_headlamp_zone(
            &runtime_state.headlamp_actor,
            brain,
            turn_id,
            wait.tell_attempt,
            message,
            now,
        )?;
        let mut timer = None;
        Self::arm_tell_back_timer(brain, wait, &mut timer);
        runtime_state.pending_turn = Some(if message == HeadlampMessage::ResetForIgnitionOff {
            PendingBrainTurn::IgnitionOffReset {
                wait,
                timer,
                event,
                now,
                headlamp_reply,
            }
        } else {
            PendingBrainTurn::PrimaryHeadlamp {
                wait,
                timer,
                event,
                now,
                message,
            }
        });
        Ok(())
    }

    /// Start processing one FSM event: assign a turn ID, then either begin a headlamp wait
    /// (if the event carries a headlamp message, or the step lands on `Off`) or commit directly.
    async fn begin_fsm_turn(
        brain: &ActorRef<DigitalTwinCarVocabulary>,
        runtime_state: &mut VirtualCarRuntimeState,
        event: FsmEvent,
        now: Instant,
    ) -> Result<(), ActorProcessingErr> {
        let turn_id = runtime_state.next_turn_id;
        runtime_state.next_turn_id = runtime_state.next_turn_id.saturating_add(1);

        if let Some(message) = fsm_event_headlamp_message(&event) {
            return Self::begin_headlamp_wait(
                brain,
                runtime_state,
                turn_id,
                message,
                event,
                now,
                None,
            )
            .await;
        }

        if fsm_step_lands_off(
            runtime_state.twin_car.current_state(),
            runtime_state.twin_car.context(),
            &event,
            now,
            &ZoneReplies::simulate_locally(),
        ) {
            return Self::begin_headlamp_wait(
                brain,
                runtime_state,
                turn_id,
                HeadlampMessage::ResetForIgnitionOff,
                event,
                now,
                None,
            )
            .await;
        }

        Self::commit_resolved_turn(
            runtime_state,
            Self::resolved_turn(event, now, None, None),
        )
        .await
    }

    /// Handle a `HeadlampZoneReady` reply from the twinlet.
    /// Matches it against the current pending turn (by `turn_id` + `tell_attempt`):
    /// - `PrimaryHeadlamp`: if the FSM now lands on `Off`, upgrade to `IgnitionOffReset` wait;
    ///   otherwise commit the turn with the primary reply.
    /// - `IgnitionOffReset`: commit with both replies.
    /// - Mismatch: restore `pending_turn` unchanged (stale reply).
    async fn on_headlamp_zone_ready(
        brain: &ActorRef<DigitalTwinCarVocabulary>,
        runtime_state: &mut VirtualCarRuntimeState,
        turn_id: u64,
        tell_attempt: u32,
        reply: HeadlampZoneReply,
    ) -> Result<(), ActorProcessingErr> {
        let Some(pending) = runtime_state.pending_turn.take() else {
            return Ok(());
        };

        match pending {
            PendingBrainTurn::PrimaryHeadlamp {
                wait,
                mut timer,
                event,
                now,
                message: _,
            } if wait.matches(turn_id, tell_attempt) => {
                Self::abort_tell_back_timer(&mut timer);
                if fsm_step_lands_off(
                    runtime_state.twin_car.current_state(),
                    runtime_state.twin_car.context(),
                    &event,
                    now,
                    &ZoneReplies::with_headlamp(Some(reply.clone()), None),
                ) {
                    return Self::begin_headlamp_wait(
                        brain,
                        runtime_state,
                        turn_id,
                        HeadlampMessage::ResetForIgnitionOff,
                        event,
                        now,
                        Some(reply),
                    )
                    .await;
                }
                Self::commit_resolved_turn(
                    runtime_state,
                    Self::resolved_turn(event, now, Some(reply), None),
                )
                .await?;
            }
            PendingBrainTurn::IgnitionOffReset {
                wait,
                mut timer,
                event,
                now,
                headlamp_reply,
            } if wait.matches(turn_id, tell_attempt) => {
                Self::abort_tell_back_timer(&mut timer);
                Self::commit_resolved_turn(
                    runtime_state,
                    Self::resolved_turn(event, now, headlamp_reply, Some(reply)),
                )
                .await?;
            }
            other => {
                runtime_state.pending_turn = Some(other);
            }
        }

        Ok(())
    }

    /// Handle a `TellBackTimeout`: retry the tell-back if attempts remain, or
    /// forge a synthetic reply (exhausted) and commit the turn.
    /// Mirrors the same structure as `on_headlamp_zone_ready`.
    async fn on_tell_back_timeout(
        brain: &ActorRef<DigitalTwinCarVocabulary>,
        runtime_state: &mut VirtualCarRuntimeState,
        turn_id: u64,
        tell_attempt: u32,
    ) -> Result<(), ActorProcessingErr> {
        let Some(pending) = runtime_state.pending_turn.take() else {
            return Ok(());
        };

        match pending {
            PendingBrainTurn::PrimaryHeadlamp {
                wait,
                mut timer,
                event,
                now,
                message,
            } if wait.matches(turn_id, tell_attempt) => {
                Self::abort_tell_back_timer(&mut timer);
                match on_tell_back_timeout(
                    &runtime_state.twin_car.context().headlamp,
                    wait,
                ) {
                    TellBackTimeoutOutcome::Retry(next) => {
                        tell_headlamp_zone(
                            &runtime_state.headlamp_actor,
                            brain,
                            turn_id,
                            next.tell_attempt,
                            message,
                            now,
                        )?;
                        Self::arm_tell_back_timer(brain, next, &mut timer);
                        runtime_state.pending_turn = Some(PendingBrainTurn::PrimaryHeadlamp {
                            wait: next,
                            timer,
                            event,
                            now,
                            message,
                        });
                    }
                    TellBackTimeoutOutcome::Exhausted(reply) => {
                        Self::commit_resolved_turn(
                            runtime_state,
                            Self::resolved_turn(event, now, Some(reply), None),
                        )
                        .await?;
                    }
                }
            }
            PendingBrainTurn::IgnitionOffReset {
                wait,
                mut timer,
                event,
                now,
                headlamp_reply,
            } if wait.matches(turn_id, tell_attempt) => {
                Self::abort_tell_back_timer(&mut timer);
                match on_tell_back_timeout(
                    &runtime_state.twin_car.context().headlamp,
                    wait,
                ) {
                    TellBackTimeoutOutcome::Retry(next) => {
                        tell_headlamp_zone(
                            &runtime_state.headlamp_actor,
                            brain,
                            turn_id,
                            next.tell_attempt,
                            HeadlampMessage::ResetForIgnitionOff,
                            now,
                        )?;
                        Self::arm_tell_back_timer(brain, next, &mut timer);
                        runtime_state.pending_turn = Some(PendingBrainTurn::IgnitionOffReset {
                            wait: next,
                            timer,
                            event,
                            now,
                            headlamp_reply,
                        });
                    }
                    TellBackTimeoutOutcome::Exhausted(reply) => {
                        Self::commit_resolved_turn(
                            runtime_state,
                            Self::resolved_turn(event, now, headlamp_reply, Some(reply)),
                        )
                        .await?;
                    }
                }
            }
            other => {
                runtime_state.pending_turn = Some(other);
            }
        }

        Ok(())
    }

    /// Handle a spontaneous headlamp actuation completion (detected by the headlamp actuator
    /// polling during *another* zone's tell-back wait). Emitted as a synthetic FSM event.
    async fn on_headlamp_zone_spontaneous(
        runtime_state: &mut VirtualCarRuntimeState,
        direction: crate::fsm::FrontHeadlampSwitchDirection,
        cause: crate::fsm::FrontHeadlampIncompleteCause,
        reply: HeadlampZoneReply,
    ) -> Result<(), ActorProcessingErr> {
        Self::commit_resolved_turn(
            runtime_state,
            ResolvedTurn {
                ingress: FsmEvent::FrontHeadlampActuationIncomplete { direction, cause },
                now: Instant::now(),
                zone_replies: ZoneReplies::with_headlamp_ingress(reply),
            },
        )
        .await
    }

    /// Drain the FSM backlog as long as no turn is pending.
    /// Called after every handler that might have consumed a pending turn.
    async fn pump_fsm_backlog(
        brain: &ActorRef<DigitalTwinCarVocabulary>,
        runtime_state: &mut VirtualCarRuntimeState,
    ) -> Result<(), ActorProcessingErr> {
        while runtime_state.pending_turn.is_none() {
            let Some((evt, now)) = runtime_state.fsm_backlog.pop_front()
            else {
                break;
            };
            Self::begin_fsm_turn(brain, runtime_state, evt, now).await?;
        }
        Ok(())
    }

    /// Build a `ResolvedTurn` from the ingress event and optional headlamp replies.
    fn resolved_turn(
        ingress: FsmEvent,
        now: Instant,
        headlamp_ingress: Option<HeadlampZoneReply>,
        headlamp_ignition_off_reset: Option<HeadlampZoneReply>,
    ) -> ResolvedTurn {
        ResolvedTurn {
            ingress,
            now,
            zone_replies: ZoneReplies::with_headlamp(headlamp_ingress, headlamp_ignition_off_reset),
        }
    }

    /// Run the quiescence pipeline on the resolved turn, then apply the resulting
    /// actuation actions and diagnostic emissions.
    async fn commit_resolved_turn(
        runtime_state: &mut VirtualCarRuntimeState,
        resolved: ResolvedTurn,
    ) -> Result<(), ActorProcessingErr> {
        let quiescent = resolve_quiescence(
            runtime_state.twin_car.current_state(),
            runtime_state.twin_car.context(),
            resolved,
        );
        Self::apply_committed_quiescence(runtime_state, quiescent).await
    }

    /// Apply a quiescence result: step the twin, emit transition records,
    /// execute domain actions (actuation), and fire diagnostic messages.
    async fn apply_committed_quiescence(
        runtime_state: &mut VirtualCarRuntimeState,
        quiescent: QuiescentResult,
    ) -> Result<(), ActorProcessingErr> {
        let old_state = runtime_state.twin_car.current_state().clone();
        let headlamp_before = runtime_state.twin_car.context().headlamp.state;
        let final_step = quiescent.final_step();
        let headlamp_after = final_step.modified_ctx.headlamp.state;
        let mut mode = ActorMode::Normal;

        for hop in &quiescent.hops {
            let record_seq = runtime_state.next_record_seq;
            runtime_state.next_record_seq = runtime_state.next_record_seq.saturating_add(1);
            Self::try_emit_transition_record(runtime_state, record_seq, hop.result.transition_record.clone());
        }

        runtime_state.twin_car.apply_step(
            final_step.next_state.clone(),
            final_step.modified_ctx.clone(),
        );

        for action in quiescent.merged_actions() {
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
        Ok(())
    }

    /// Emit one transition record via the transition sink, falling back to the
    /// diagnostic sink on overflow or channel closure.
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

    /// Reply to a `GetStatus` RPC with the current `CarSnapshot`.
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
/// Probes the domain state before and after the step: if `OnRequested→On` or
/// `OffRequested→Off`, the actuation completed positively.
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
