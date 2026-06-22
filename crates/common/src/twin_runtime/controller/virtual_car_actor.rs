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
use crate::digital_twin::{CarSnapshot, DigitalTwinCar, DigitalTwinCarVocabulary};
use crate::twin_runtime::constants::ZONE_TELL_BACK_WAIT;
use crate::twin_runtime::controller::actuation_manager::{
    ActuationManager, DefaultActuationManager,
};
use crate::twin_runtime::controller::vehicle_controller::VehicleControllerRuntimeOptions;
use crate::fsm::{
    self, DomainAction, FrontHeadlampSwitchDirection, FsmEvent, FsmState, HeadlampState,
};
use crate::twin_runtime::headlamp_actor::{tell_headlamp_zone, HeadlampActor, HeadlampActorMsg, HeadlampActorState};
use crate::twin_runtime::twin_turn::{commit_resolved_turn as resolve_quiescence, fsm_step_lands_off, ResolvedTurn};
use crate::twin_runtime::ZoneReplies;
use crate::twin_runtime::zone_tell_back::{synthetic_unresponsive_headlamp_reply, TellBackWait};
use crate::twin_runtime::zone_turn::user_event_to_headlamp_tell;
use crate::twin_runtime::turn_barrier::{BarrierEntry, BarrierPhase, PassthroughBarrier, TellBackTimer, TimeoutOutcome, TurnBarrier};
use crate::vehicle_state::{HeadlampMessage, VehicleContext};
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

/// Compile-time list of assemblies the brain actor coordinates.
///
/// Phase 8 replaces this constant with assembly IDs embedded directly in
/// `FsmState::PreparingToStart { assemblies }` and `PreparingToStop { assemblies }`.
/// For Phases 5–7 this constant is the sole place where `ZoneId::Headlamp`
/// (and future `ZoneId::Wiper`) is listed as a managed assembly.
const MANAGED_ASSEMBLIES: &[crate::fsm::ZoneId] = &[crate::fsm::ZoneId::Headlamp];

/// Mutable state of the virtual car actor, held across `handle` calls.
pub struct VirtualCarRuntimeState {
    twin_car: DigitalTwinCar,
    headlamp_actor: ActorRef<HeadlampActorMsg>,
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

        let mut initial_ctx = VehicleContext::default();
        if let Some(hl_ctx) = args.runtime_options.initial_headlamp_ctx.clone() {
            initial_ctx.headlamp = hl_ctx;
        }
        let (headlamp_actor, _) = ractor::spawn::<HeadlampActor>(HeadlampActorState::new(
            initial_ctx.headlamp.clone(),
            args.runtime_options.test_silent_headlamp,
        ))
        .await?;

        Ok(VirtualCarRuntimeState {
            twin_car: DigitalTwinCar::new(identity, FsmState::Off, initial_ctx)?,
            headlamp_actor,
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

    /// Main message dispatch.  Every `Fsm` event immediately gets its own [`TurnBarrier`]
    /// pushed to the back of `barrier_queue`; the drain loop commits from the front.
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
                // Every event gets its own barrier immediately (no pending_turn guard).
                Self::begin_fsm_turn(&myself, runtime_state, evt_arrived, now).await?;
                Self::try_drain_barrier_queue(runtime_state).await
            }
            ZoneReady {
                zone_id,
                turn_id,
                tell_attempt,
                reply,
            } => {
                Self::on_zone_ready(
                    &myself,
                    runtime_state,
                    zone_id,
                    turn_id,
                    tell_attempt,
                    reply,
                )
                .await?;
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

    /// Abort all in-flight timers and stop the headlamp twinlet.
    async fn post_stop(
        &self,
        _myself: ActorRef<Self::Msg>,
        runtime_state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        for entry in &mut runtime_state.barrier_queue {
            entry.abort_all_timers();
        }
        runtime_state.barrier_queue.clear();
        // Interim: brain stops the headlamp twinlet here. Target: supervisor-ordered teardown.
        runtime_state.headlamp_actor.stop(None);
        Ok(())
    }
}

impl VirtualCarActor {
    // ── timer helper ─────────────────────────────────────────────────────────

    /// Schedule a `ZoneTellBackTimeout` message after `ZONE_TELL_BACK_WAIT`.
    fn arm_tell_back_timer(
        brain: &ActorRef<DigitalTwinCarVocabulary>,
        turn_id: u64,
        tell_attempt: u32,
    ) -> TellBackTimer {
        brain.send_after(
            RactorDuration::from(ZONE_TELL_BACK_WAIT),
            move || DigitalTwinCarVocabulary::ZoneTellBackTimeout {
                zone_id: crate::fsm::ZoneId::Headlamp,
                turn_id,
                tell_attempt,
            },
        )
    }

    // ── FSM turn entry ────────────────────────────────────────────────────────

    /// Assign a turn ID, create a barrier entry, tell any required zone(s), and push
    /// the entry onto the back of `barrier_queue`.
    ///
    /// Three mutually exclusive paths, tried in priority order:
    ///
    /// 1. **Zone-directed** — the user event maps to a headlamp message (e.g. `UpdateAmbientLux`
    ///    → `AmbientLux`).  A [`TurnBarrier`] with `Headlamp` pending is created; the zone gets
    ///    a tell and a timer.
    ///
    /// 2. **Ignition-off reset** — no headlamp message, but the FSM will land on `Off` given
    ///    the current context.  A `ResetForIgnitionOff` is sent directly and the barrier enters
    ///    `IgnitionOffReset` phase.  Removed in Phase 6.
    ///
    /// 3. **Passthrough** — no zone interaction at all; the [`PassthroughBarrier`] is instantly
    ///    drainable and keeps the queue ordered.
    ///
    /// Note: `AssemblyZoneReady` events never go through `begin_fsm_turn` — assembly barriers
    /// are created in `apply_committed_quiescence` when `StartAssemblies`/`StopAssemblies` is
    /// received, and committed by the drain loop when their zone replies arrive.
    async fn begin_fsm_turn(
        brain: &ActorRef<DigitalTwinCarVocabulary>,
        runtime_state: &mut VirtualCarRuntimeState,
        event: FsmEvent,
        now: Instant,
    ) -> Result<(), ActorProcessingErr> {
        let turn_id = runtime_state.alloc_turn_id();

        // Primary headlamp zone tell (zone-directed user event).
        if let Some(message) = user_event_to_headlamp_tell(&event) {
            let wait = TellBackWait::new(turn_id);
            tell_headlamp_zone(
                &runtime_state.headlamp_actor,
                brain,
                turn_id,
                0,
                message,
                now,
            )?;
            let timer = Self::arm_tell_back_timer(brain, turn_id, 0);
            let mut barrier = TurnBarrier::new(turn_id, event, now);
            barrier.add_pending_zone(crate::fsm::ZoneId::Headlamp, message, wait, timer);
            runtime_state.barrier_queue.push_back(BarrierEntry::Waiting(barrier));
            return Ok(());
        }

        // Pure ignition-off reset: event has no headlamp message, but FSM will land on Off.
        // After Phase 5 this path is unreachable: PowerOff → PreparingToStop (not Off),
        // and AssemblyZoneReady is excluded from begin_fsm_turn.  Phase 6 removes it.
        if fsm_step_lands_off(
            runtime_state.twin_car.current_state(),
            runtime_state.twin_car.context(),
            &event,
            now,
            &ZoneReplies::simulate_locally(),
        ) {
            let msg = HeadlampMessage::ResetForIgnitionOff;
            let wait = TellBackWait::new(turn_id);
            tell_headlamp_zone(&runtime_state.headlamp_actor, brain, turn_id, 0, msg, now)?;
            let timer = Self::arm_tell_back_timer(brain, turn_id, 0);
            let mut barrier = TurnBarrier::new(turn_id, event, now);
            barrier.start_ignition_off_reset(wait, timer);
            runtime_state.barrier_queue.push_back(BarrierEntry::Waiting(barrier));
            return Ok(());
        }

        // Pure brain-state transition: no zone message emitted, no wait needed.
        // PassthroughBarrier is instantly drainable and keeps the queue ordered.
        let passthrough = PassthroughBarrier::new(turn_id, event, now);
        runtime_state.barrier_queue.push_back(BarrierEntry::Passthrough(passthrough));
        Ok(())
    }

    // ── zone reply handlers ───────────────────────────────────────────────────

    /// Handle a `ZoneReady` from a zone twinlet.
    ///
    /// Finds the barrier by `turn_id`; validates `tell_attempt`; calls
    /// `act_on_zone_reply`.  If the primary reply reveals that the FSM will land on `Off`,
    /// transitions the barrier to `IgnitionOffReset` phase instead of completing it.
    ///
    /// **Borrow-split pattern**: `twin_car` state is snapshotted *before* the mutable
    /// borrow of `barrier_queue` so that the `fsm_step_lands_off` probe can read both
    /// without violating the borrow checker.  The barrier is re-looked-up by `turn_id` in
    /// a second mutable borrow to apply the `IgnitionOffReset` transition.
    async fn on_zone_ready(
        brain: &ActorRef<DigitalTwinCarVocabulary>,
        runtime_state: &mut VirtualCarRuntimeState,
        zone_id: crate::fsm::ZoneId,
        turn_id: u64,
        tell_attempt: u32,
        reply: crate::digital_twin::ZoneReply,
    ) -> Result<(), ActorProcessingErr> {
        let reply_hl = match reply {
            crate::digital_twin::ZoneReply::Headlamp(r) => r,
        };

        // Snapshot the committed twin state before the mutable borrow of barrier_queue.
        let (current_state, current_ctx) = (
            runtime_state.twin_car.current_state().clone(),
            runtime_state.twin_car.context().clone(),
        );

        // Find the matching barrier, validate attempt, apply reply.
        let needs_ignition_off_reset: Option<Instant> = {
            let Some(entry) = runtime_state
                .barrier_queue
                .iter_mut()
                .find(|e| e.turn_id() == turn_id)
            else {
                return Ok(());
            };
            let Some(barrier) = entry.as_waiting_mut() else {
                return Ok(()); // passthrough entries have no pending zones
            };

            if !barrier.tell_attempt_matches(zone_id, tell_attempt) {
                return Ok(()); // stale or mismatched reply — discard
            }

            barrier.act_on_zone_reply(zone_id, crate::digital_twin::ZoneReply::Headlamp(reply_hl.clone()));

            // After the primary reply, check whether the FSM will land on Off.
            if matches!(barrier.phase(), BarrierPhase::Primary) && barrier.is_complete() {
                let event = barrier.event().clone();
                let barrier_now = barrier.now();
                let lands_off = fsm_step_lands_off(
                    &current_state,
                    &current_ctx,
                    &event,
                    barrier_now,
                    &ZoneReplies::with_headlamp(Some(reply_hl.clone()), None),
                );
                if lands_off { Some(barrier_now) } else { None }
            } else {
                None
            }
        }; // mutable borrow of barrier_queue released here

        // If IgnitionOffReset is needed, transition the barrier and send the reset tell.
        if let Some(barrier_now) = needs_ignition_off_reset {
            let new_wait = TellBackWait::new(turn_id);
            tell_headlamp_zone(
                &runtime_state.headlamp_actor,
                brain,
                turn_id,
                0,
                HeadlampMessage::ResetForIgnitionOff,
                barrier_now,
            )?;
            let new_timer = Self::arm_tell_back_timer(brain, turn_id, 0);
            if let Some(entry) = runtime_state
                .barrier_queue
                .iter_mut()
                .find(|e| e.turn_id() == turn_id)
            {
                if let Some(barrier) = entry.as_waiting_mut() {
                    barrier.start_ignition_off_reset(new_wait, new_timer);
                }
            }
        }

        Ok(())
    }

    /// Handle a `ZoneTellBackTimeout`.
    ///
    /// Validates the attempt, decides retry vs. give-up, re-tells or synthesises a reply.
    async fn on_zone_timeout(
        brain: &ActorRef<DigitalTwinCarVocabulary>,
        runtime_state: &mut VirtualCarRuntimeState,
        zone_id: crate::fsm::ZoneId,
        turn_id: u64,
        tell_attempt: u32,
    ) -> Result<(), ActorProcessingErr> {
        // Snapshot headlamp ctx for potential synthetic reply (before barrier borrow).
        let headlamp_ctx = runtime_state.twin_car.context().headlamp.clone();

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
                // Re-tell the zone with the next attempt number.
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

                tell_headlamp_zone(
                    &runtime_state.headlamp_actor,
                    brain,
                    turn_id,
                    next_attempt,
                    msg,
                    barrier_now,
                )?;
                let new_timer = Self::arm_tell_back_timer(brain, turn_id, next_attempt);

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
                // All retries exhausted: synthesise a reply and close the zone's slot.
                let synthetic = synthetic_unresponsive_headlamp_reply(&headlamp_ctx);
                if let Some(entry) = runtime_state
                    .barrier_queue
                    .iter_mut()
                    .find(|e| e.turn_id() == turn_id)
                {
                    if let Some(barrier) = entry.as_waiting_mut() {
                        barrier.act_on_zone_reply(
                            zone_id,
                            crate::digital_twin::ZoneReply::Headlamp(synthetic),
                        );
                    }
                }
            }
        }

        Ok(())
    }

    /// Handle a spontaneous zone event (ACK timer, future assembly deadlines).
    ///
    /// These are not correlated to a brain `turn_id`, so they do not interact with
    /// `barrier_queue`.  The event is committed directly; the drain loop runs afterwards
    /// (called from the `handle` arm).
    async fn on_zone_spontaneous(
        runtime_state: &mut VirtualCarRuntimeState,
        _zone_id: crate::fsm::ZoneId,
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
                zone_replies: ZoneReplies::with_headlamp_ingress(reply),
            },
        )
        .await
    }

    // ── drain loop ────────────────────────────────────────────────────────────

    /// Commit all complete entries from the front of `barrier_queue`.
    ///
    /// **Head-of-buffer (HOB) invariant**: only the front entry may be committed.
    /// If the front [`TurnBarrier`] is still waiting for a zone reply, later entries —
    /// even if their own zones have already replied — must wait.  This guarantees that
    /// `commit_resolved_turn` is called in strict event-arrival order regardless of the
    /// order in which zone twinlets reply.
    async fn try_drain_barrier_queue(
        runtime_state: &mut VirtualCarRuntimeState,
    ) -> Result<(), ActorProcessingErr> {
        loop {
            let Some(front) = runtime_state.barrier_queue.front() else {
                break;
            };
            if !front.is_complete() {
                // Front entry is still awaiting a zone reply; nothing can proceed.
                break;
            }
            let committed = runtime_state.barrier_queue.pop_front().expect("checked above");
            let resolved = committed.into_resolved_turn();
            Self::commit_resolved_turn(runtime_state, resolved).await?;
        }
        Ok(())
    }

    // ── quiescence & apply ────────────────────────────────────────────────────

    /// Run the quiescence pipeline on the resolved turn, then apply the result.
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
                DomainAction::EnterMode(_) => {}
                DomainAction::LogWarning(message) => {
                    if let Some(sink) = &runtime_state.diagnostic_sink {
                        let _ = sink.try_emit(diag_warning(
                            runtime_state.twin_car.identity(),
                            &message,
                        ));
                    }
                }
                DomainAction::StartAssemblies => {
                    let now = Instant::now();
                    let brain = runtime_state.self_ref.clone();
                    for &zone_id in MANAGED_ASSEMBLIES {
                        let turn_id = runtime_state.alloc_turn_id();
                        let msg = HeadlampMessage::BecomeOn;
                        let wait = TellBackWait::new(turn_id);
                        tell_headlamp_zone(&runtime_state.headlamp_actor, &brain, turn_id, 0, msg, now)?;
                        let timer = Self::arm_tell_back_timer(&brain, turn_id, 0);
                        let barrier = TurnBarrier::new_for_assembly_zone(turn_id, zone_id, msg, wait, timer, now);
                        runtime_state.barrier_queue.push_back(BarrierEntry::Waiting(barrier));
                    }
                }
                DomainAction::StopAssemblies => {
                    let now = Instant::now();
                    let brain = runtime_state.self_ref.clone();
                    for &zone_id in MANAGED_ASSEMBLIES {
                        let turn_id = runtime_state.alloc_turn_id();
                        let msg = HeadlampMessage::BecomeOff;
                        let wait = TellBackWait::new(turn_id);
                        tell_headlamp_zone(&runtime_state.headlamp_actor, &brain, turn_id, 0, msg, now)?;
                        let timer = Self::arm_tell_back_timer(&brain, turn_id, 0);
                        let barrier = TurnBarrier::new_for_assembly_zone(turn_id, zone_id, msg, wait, timer, now);
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
