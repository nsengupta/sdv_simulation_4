//! Virtual ECU / gateway **actor** ([`ractor::Actor`](https://crates.io/crates/ractor/0.15.12)).
//!
//! ## Message layering
//! - **[`FsmEvent`](crate::fsm::FsmEvent)** — pure FSM vocabulary: `Clone`, no I/O ports.
//! - **[`DigitalTwinCarVocabulary`](crate::digital_twin::DigitalTwinCarVocabulary)** — full mailbox:
//!   wraps [`FsmEvent`](crate::fsm::FsmEvent) via [`DigitalTwinCarVocabulary::Fsm`] plus
//!   request/reply such as [`DigitalTwinCarVocabulary::GetStatus`] ([`RpcReplyPort`]).
//!
//! ## Phase 4 — reorder buffer
//!
//! Every `Fsm` message immediately creates a [`TurnBarrier`] at the **back** of
//! `barrier_queue`.  The drain loop (`try_drain_barrier_queue`) commits completed barriers
//! strictly from the **front**, preserving event-arrival order regardless of the order in
//! which zone replies arrive.
//!
//! ## Phase 8 — FSM-embedded assembly topology
//!
//! `MANAGED_ASSEMBLIES` is deleted.  The `StartAssemblies`/`StopAssemblies` `DomainAction`
//! variants now carry a `&'static [AssemblyId]` payload derived from `ALL_ASSEMBLIES`
//! inside `machineries.rs`.  The actor reads the list directly from the action payload,
//! eliminating the out-of-band constant.  Zone-dispatch helpers remain unchanged.
//! `handle()` still has exactly **four arms**: `Fsm`, `ZoneReady`, `ZoneTellBackTimeout`, `GetStatus`.

use async_trait::async_trait;
use ractor::concurrency::Duration as RactorDuration;
use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use crate::diagnostic::{
    DiagnosticMessage, DiagnosticSink, TokioMpscDiagnosticSink, diag_front_headlamp_confirmed,
    diag_state_transition, diag_timer_tick, diag_actuation_failure, diag_warning,
    diag_transition_sink_full, diag_transition_sink_closed,
};
use crate::digital_twin::{CarSnapshot, DigitalTwinCar, DigitalTwinCarVocabulary, ZoneMessage, ZoneReply};
use crate::twin_runtime::constants::ZONE_TELL_BACK_WAIT;
use crate::twin_runtime::controller::actuation_manager::{
    ActuationManager, DefaultActuationManager,
};
use crate::twin_runtime::controller::vehicle_controller::VehicleControllerRuntimeOptions;
use crate::fsm::{
    self, AssemblyId, DomainAction, FrontHeadlampSwitchDirection, FsmEvent, FsmState, HeadlampState,
};
use crate::twin_runtime::headlamp_actor::{
    tell_headlamp_zone, HeadlampActor, HeadlampActorMsg, HeadlampActorState,
};
use crate::twin_runtime::wiper_actor::{
    tell_wiper_zone, WiperActor, WiperActorMsg, WiperActorState,
};
use crate::twin_runtime::twin_turn::{commit_resolved_turn as resolve_quiescence, ResolvedTurn};
use crate::twin_runtime::ZoneReplies;
use crate::twin_runtime::zone_tell_back::{synthetic_unresponsive_headlamp_reply, TellBackWait};
use crate::twin_runtime::zone_turn::zone_message_for_event;
use crate::twin_runtime::turn_barrier::{
    BarrierEntry, PassthroughBarrier, TellBackTimer, TimeoutOutcome, TurnBarrier,
};
use crate::vehicle_state::{HeadlampMessage, WiperMessage, VehicleContext};
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


/// Mutable state of the virtual car actor, held across `handle` calls.
pub struct VirtualCarRuntimeState {
    twin_car: DigitalTwinCar,
    headlamp_actor: ActorRef<HeadlampActorMsg>,
    wiper_actor: ActorRef<WiperActorMsg>,
    /// Stable self-reference used to arm timers and send `ZoneTellBackTimeout` messages.
    /// Captured in `pre_start` via `myself.clone()`; idiomatic actor self-ref pattern.
    self_ref: ActorRef<DigitalTwinCarVocabulary>,
    next_turn_id: u64,
    /// Reorder-buffer: every in-flight FSM turn occupies one slot.
    /// The drain loop commits from the front in strict arrival order.
    barrier_queue: VecDeque<BarrierEntry>,
    next_record_seq: u64,
    /// Monotonic↔wall anchor for this run; the sole source of wall-clock stamps on published
    /// records and of the actuation `session_id`.
    session_epoch: SessionEpoch,
    runtime_options: VehicleControllerRuntimeOptions,
    actuation_manager: Arc<dyn ActuationManager>,
    diagnostic_sink: Option<Arc<dyn DiagnosticSink>>,
    transition_sink: Option<Arc<dyn TransitionRecordSink>>,
}

impl VirtualCarRuntimeState {
    /// Consume and return the next monotonic turn ID, advancing the counter atomically.
    ///
    /// This is the **only** place `next_turn_id` is read; coupling the advance with the
    /// read prevents the counter from drifting out of sync with `TurnBarrier` creation.
    fn alloc_turn_id(&mut self) -> u64 {
        let id = self.next_turn_id;
        self.next_turn_id = self.next_turn_id.saturating_add(1);
        id
    }
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
        myself: ActorRef<Self::Msg>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        let identity = args.identity.clone();

        let diagnostic_sink: Option<Arc<dyn DiagnosticSink>> = args
            .runtime_options
            .diagnostic_tx
            .clone()
            .map(|tx| Arc::new(TokioMpscDiagnosticSink::new(tx)) as Arc<dyn DiagnosticSink>);

        let transition_sink: Option<Arc<dyn TransitionRecordSink>> = args
            .runtime_options
            .transition_tx
            .clone()
            .map(|tx| Arc::new(TokioMpscTransitionRecordSink::new(tx)) as Arc<dyn TransitionRecordSink>);

        if let Some(sink) = &diagnostic_sink {
            let _ = sink.try_emit(DiagnosticMessage::info(
                "VirtualCarActor",
                format!("Physical Car name: {identity}, initializing its Digital Twin ..."),
            ));
        }

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
                Arc::new(DefaultActuationManager::default())
            };

        let (headlamp_actor, _) = ractor::spawn::<HeadlampActor>(HeadlampActorState::new(
            Default::default(),
            args.runtime_options.test_silent_headlamp,
        ))
        .await?;

        let (wiper_actor, _) = ractor::spawn::<WiperActor>(WiperActorState::new(
            Default::default(),
            args.runtime_options.test_silent_wiper,
        ))
        .await?;

        Ok(VirtualCarRuntimeState {
            twin_car: DigitalTwinCar::new(identity, FsmState::Off, VehicleContext::default())?,
            headlamp_actor,
            wiper_actor,
            self_ref: myself.clone(),
            next_turn_id: 1,
            barrier_queue: VecDeque::new(),
            next_record_seq: 1,
            session_epoch,
            runtime_options: args.runtime_options,
            actuation_manager,
            diagnostic_sink,
            transition_sink,
        })
    }

    /// Main message dispatch — exactly **four** arms (unchanged from Phase 6).
    ///
    /// Adding the Wiper assembly required zero new arms: zone routing is handled inside
    /// `begin_fsm_turn` and the `StartAssemblies`/`StopAssemblies` loop.
    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        runtime_state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        use DigitalTwinCarVocabulary::{Fsm, GetStatus, ZoneReady, ZoneSpontaneous, ZoneTellBackTimeout};

        match message {
            Fsm(evt_arrived) => {
                if matches!(evt_arrived, FsmEvent::TimerTick) && runtime_state.runtime_options.log_timer_tick {
                    if let Some(sink) = &runtime_state.diagnostic_sink {
                        let _ = sink.try_emit(diag_timer_tick(runtime_state.twin_car.identity()));
                    }
                }
                let now = Instant::now();
                Self::begin_fsm_turn(&myself, runtime_state, evt_arrived, now).await?;
                Self::try_drain_barrier_queue(runtime_state).await
            }
            ZoneReady {
                zone_id,
                turn_id,
                tell_attempt,
                reply,
            } => {
                Self::on_zone_ready(runtime_state, zone_id, turn_id, tell_attempt, reply).await?;
                Self::try_drain_barrier_queue(runtime_state).await
            }
            ZoneTellBackTimeout {
                zone_id,
                turn_id,
                tell_attempt,
            } => {
                Self::on_zone_timeout(&myself, runtime_state, zone_id, turn_id, tell_attempt)
                    .await?;
                Self::try_drain_barrier_queue(runtime_state).await
            }
            ZoneSpontaneous { zone_id, event } => {
                Self::on_zone_spontaneous(runtime_state, zone_id, event).await?;
                Self::try_drain_barrier_queue(runtime_state).await
            }
            GetStatus(reply) => Self::reply_get_status(
                reply,
                &runtime_state.twin_car,
                runtime_state.next_record_seq.saturating_sub(1),
            ),
        }
    }

    /// Abort all in-flight timers and stop both twinlets.
    async fn post_stop(
        &self,
        _myself: ActorRef<Self::Msg>,
        runtime_state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        for entry in &mut runtime_state.barrier_queue {
            entry.abort_all_timers();
        }
        runtime_state.barrier_queue.clear();
        runtime_state.headlamp_actor.stop(None);
        runtime_state.wiper_actor.stop(None);
        Ok(())
    }
}

impl VirtualCarActor {
    // ── zone-dispatch helpers (D11) ────────────────────────────────────────────

    fn become_on_message_for(assembly_id: AssemblyId) -> ZoneMessage {
        match assembly_id {
            AssemblyId::Headlamp => ZoneMessage::Headlamp(HeadlampMessage::BecomeOn),
            AssemblyId::Wiper    => ZoneMessage::Wiper(WiperMessage::BecomeOn),
        }
    }

    fn become_off_message_for(assembly_id: AssemblyId) -> ZoneMessage {
        match assembly_id {
            AssemblyId::Headlamp => ZoneMessage::Headlamp(HeadlampMessage::BecomeOff),
            AssemblyId::Wiper    => ZoneMessage::Wiper(WiperMessage::BecomeOff),
        }
    }

    fn tell_zone(
        runtime_state: &VirtualCarRuntimeState,
        brain: &ActorRef<DigitalTwinCarVocabulary>,
        _assembly_id: AssemblyId,
        message: &ZoneMessage,
        turn_id: u64,
        tell_attempt: u32,
        now: Instant,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            ZoneMessage::Headlamp(m) => tell_headlamp_zone(
                &runtime_state.headlamp_actor, brain, turn_id, tell_attempt, *m, now,
            ),
            ZoneMessage::Wiper(m) => tell_wiper_zone(
                &runtime_state.wiper_actor, brain, turn_id, tell_attempt, *m, now,
            ),
        }
    }

    fn synthetic_reply_for(ctx: &VehicleContext, assembly_id: AssemblyId) -> ZoneReply {
        match assembly_id {
            AssemblyId::Headlamp => ZoneReply::Headlamp(
                synthetic_unresponsive_headlamp_reply(&ctx.headlamp)
            ),
            AssemblyId::Wiper => ZoneReply::Wiper(crate::vehicle_state::WiperZoneReply {
                ctx: ctx.wiper.clone(),
                outcomes: vec![],
            }),
        }
    }

    // ── timer helper ──────────────────────────────────────────────────────────

    fn arm_tell_back_timer(
        brain: &ActorRef<DigitalTwinCarVocabulary>,
        zone_id: AssemblyId,
        turn_id: u64,
        tell_attempt: u32,
    ) -> TellBackTimer {
        brain.send_after(
            RactorDuration::from(ZONE_TELL_BACK_WAIT),
            move || DigitalTwinCarVocabulary::ZoneTellBackTimeout {
                zone_id,
                turn_id,
                tell_attempt,
            },
        )
    }

    // ── FSM turn entry ────────────────────────────────────────────────────────

    /// Assign a turn ID, create a barrier entry, tell any required zone(s), and push
    /// the entry onto the back of `barrier_queue`.
    ///
    /// Two mutually exclusive paths:
    ///
    /// 1. **Zone-directed** — `zone_message_for_event` returns `Some((zone_id, message))`.
    ///    A [`TurnBarrier`] with the relevant zone pending is created; the zone gets a tell
    ///    and a timer.
    ///
    /// 2. **Passthrough** — `zone_message_for_event` returns `None`.  The
    ///    [`PassthroughBarrier`] is instantly drainable and keeps the queue ordered.
    ///    This covers events with no zone mapping (e.g. `PowerOn`, `TimerTick`) AND
    ///    user events arriving during `PreparingToStart` or `PreparingToStop`.
    async fn begin_fsm_turn(
        brain: &ActorRef<DigitalTwinCarVocabulary>,
        runtime_state: &mut VirtualCarRuntimeState,
        event: FsmEvent,
        now: Instant,
    ) -> Result<(), ActorProcessingErr> {
        let turn_id = runtime_state.alloc_turn_id();

        if let Some((zone_id, message)) =
            zone_message_for_event(&event, runtime_state.twin_car.current_state())
        {
            let wait = TellBackWait::new(turn_id);
            Self::tell_zone(runtime_state, brain, zone_id, &message, turn_id, 0, now)?;
            let timer = Self::arm_tell_back_timer(brain, zone_id, turn_id, 0);
            let mut barrier = TurnBarrier::new(turn_id, event, now);
            barrier.add_pending_zone(zone_id, message, wait, timer);
            runtime_state.barrier_queue.push_back(BarrierEntry::Waiting(barrier));
            return Ok(());
        }

        // Passthrough: no zone message for this event in the current state.
        let passthrough = PassthroughBarrier::new(turn_id, event, now);
        runtime_state.barrier_queue.push_back(BarrierEntry::Passthrough(passthrough));
        Ok(())
    }

    // ── zone reply handlers ───────────────────────────────────────────────────

    /// Handle a `ZoneReady` from a zone twinlet.
    ///
    /// Finds the barrier by `turn_id`; validates `tell_attempt`; stores the reply.
    /// No assembly-specific unpacking — `reply` is stored as-is (`ZoneReply` enum).
    async fn on_zone_ready(
        runtime_state: &mut VirtualCarRuntimeState,
        zone_id: AssemblyId,
        turn_id: u64,
        tell_attempt: u32,
        reply: ZoneReply,
    ) -> Result<(), ActorProcessingErr> {
        let Some(entry) = runtime_state
            .barrier_queue
            .iter_mut()
            .find(|e| e.turn_id() == turn_id)
        else {
            return Ok(());
        };
        let Some(barrier) = entry.as_waiting_mut() else {
            return Ok(());
        };

        if !barrier.tell_attempt_matches(zone_id, tell_attempt) {
            return Ok(());
        }

        barrier.act_on_zone_reply(zone_id, reply);
        Ok(())
    }

    /// Handle a `ZoneTellBackTimeout`.
    ///
    /// Validates the attempt, decides retry vs. give-up, re-tells or synthesises a reply.
    async fn on_zone_timeout(
        brain: &ActorRef<DigitalTwinCarVocabulary>,
        runtime_state: &mut VirtualCarRuntimeState,
        zone_id: AssemblyId,
        turn_id: u64,
        tell_attempt: u32,
    ) -> Result<(), ActorProcessingErr> {
        let outcome = {
            let Some(entry) = runtime_state
                .barrier_queue
                .iter_mut()
                .find(|e| e.turn_id() == turn_id)
            else {
                return Ok(());
            };
            let Some(barrier) = entry.as_waiting_mut() else {
                return Ok(());
            };
            barrier.act_on_zone_timeout(zone_id, tell_attempt)
        };

        match outcome {
            TimeoutOutcome::Retry { next_attempt } => {
                let (msg, barrier_now) = runtime_state
                    .barrier_queue
                    .iter()
                    .find(|e| e.turn_id() == turn_id)
                    .and_then(|e| e.as_waiting())
                    .and_then(|b| b.zone_message(zone_id).map(|m| (m, b.now())))
                    .ok_or_else(|| {
                        ActorProcessingErr::from(std::io::Error::other(
                            "timeout retry: no zone message stored",
                        ))
                    })?;

                Self::tell_zone(runtime_state, brain, zone_id, &msg, turn_id, next_attempt, barrier_now)?;
                let new_timer = Self::arm_tell_back_timer(brain, zone_id, turn_id, next_attempt);

                if let Some(entry) = runtime_state
                    .barrier_queue
                    .iter_mut()
                    .find(|e| e.turn_id() == turn_id)
                {
                    if let Some(barrier) = entry.as_waiting_mut() {
                        barrier.store_retry_timer(zone_id, new_timer);
                    }
                }
            }
            TimeoutOutcome::GaveUp => {
                // Synthesise a reply from current context (no intermediate mutable borrow).
                let synthetic = Self::synthetic_reply_for(runtime_state.twin_car.context(), zone_id);
                if let Some(entry) = runtime_state
                    .barrier_queue
                    .iter_mut()
                    .find(|e| e.turn_id() == turn_id)
                {
                    if let Some(barrier) = entry.as_waiting_mut() {
                        barrier.act_on_zone_reply(zone_id, synthetic);
                    }
                }
            }
        }

        Ok(())
    }

    /// Handle a spontaneous zone event (ACK timer, future assembly deadlines).
    ///
    /// These are not correlated to a brain `turn_id`, so they do not interact with
    /// `barrier_queue`.  The event is committed directly; the drain loop runs afterwards.
    async fn on_zone_spontaneous(
        runtime_state: &mut VirtualCarRuntimeState,
        _assembly_id: AssemblyId,   // headlamp-only; wiper has no spontaneous events
        event: crate::digital_twin::ZoneSpontaneousEvent,
    ) -> Result<(), ActorProcessingErr> {
        let crate::digital_twin::ZoneSpontaneousEvent::Headlamp {
            direction,
            cause,
            reply,
        } = event;
        Self::commit_resolved_turn(
            runtime_state,
            ResolvedTurn {
                ingress: FsmEvent::FrontHeadlampActuationIncomplete { direction, cause },
                now: Instant::now(),
                zone_replies: ZoneReplies::with_reply(AssemblyId::Headlamp, ZoneReply::Headlamp(reply)),
            },
        )
        .await
    }

    // ── drain loop ────────────────────────────────────────────────────────────

    /// Commit all complete entries from the front of `barrier_queue`.
    ///
    /// **Head-of-buffer (HOB) invariant**: only the front entry may be committed.
    async fn try_drain_barrier_queue(
        runtime_state: &mut VirtualCarRuntimeState,
    ) -> Result<(), ActorProcessingErr> {
        loop {
            let Some(front) = runtime_state.barrier_queue.front() else {
                break;
            };
            if !front.is_complete() {
                break;
            }
            let committed = runtime_state.barrier_queue.pop_front().expect("checked above");
            let resolved = committed.into_resolved_turn();
            Self::commit_resolved_turn(runtime_state, resolved).await?;
        }
        Ok(())
    }

    // ── quiescence & apply ────────────────────────────────────────────────────

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

    async fn apply_committed_quiescence(
        runtime_state: &mut VirtualCarRuntimeState,
        quiescent: crate::twin_runtime::twin_turn::QuiescentResult,
    ) -> Result<(), ActorProcessingErr> {
        let old_state = runtime_state.twin_car.current_state().clone();
        let headlamp_before = runtime_state.twin_car.context().headlamp.state;
        let final_step = quiescent.final_step();
        let headlamp_after = final_step.modified_ctx.headlamp.state;

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
                DomainAction::LogWarning(message) => {
                    if let Some(sink) = &runtime_state.diagnostic_sink {
                        let _ = sink.try_emit(diag_warning(
                            runtime_state.twin_car.identity(),
                            &message,
                        ));
                    }
                }
                DomainAction::StartAssemblies(assemblies) => {
                    let now = Instant::now();
                    let brain = runtime_state.self_ref.clone();
                    for &assembly_id in assemblies.iter() {
                        let turn_id = runtime_state.alloc_turn_id();
                        let msg = Self::become_on_message_for(assembly_id);
                        let wait = TellBackWait::new(turn_id);
                        Self::tell_zone(runtime_state, &brain, assembly_id, &msg, turn_id, 0, now)?;
                        let timer = Self::arm_tell_back_timer(&brain, assembly_id, turn_id, 0);
                        let barrier = TurnBarrier::new_for_assembly_zone(turn_id, assembly_id, msg, wait, timer, now);
                        runtime_state.barrier_queue.push_back(BarrierEntry::Waiting(barrier));
                    }
                }
                DomainAction::StopAssemblies(assemblies) => {
                    let now = Instant::now();
                    let brain = runtime_state.self_ref.clone();
                    for &assembly_id in assemblies.iter() {
                        let turn_id = runtime_state.alloc_turn_id();
                        let msg = Self::become_off_message_for(assembly_id);
                        let wait = TellBackWait::new(turn_id);
                        Self::tell_zone(runtime_state, &brain, assembly_id, &msg, turn_id, 0, now)?;
                        let timer = Self::arm_tell_back_timer(&brain, assembly_id, turn_id, 0);
                        let barrier = TurnBarrier::new_for_assembly_zone(turn_id, assembly_id, msg, wait, timer, now);
                        runtime_state.barrier_queue.push_back(BarrierEntry::Waiting(barrier));
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

        if let Some(direction) = front_headlamp_confirmed_direction(headlamp_before, headlamp_after) {
            if let Some(sink) = &runtime_state.diagnostic_sink {
                let _ = sink.try_emit(diag_front_headlamp_confirmed(
                    runtime_state.twin_car.identity(),
                    direction,
                ));
            }
        }

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
        (HeadlampState::OffRequested, HeadlampState::Ready) => Some(FrontHeadlampSwitchDirection::Off),
        _ => None,
    }
}
