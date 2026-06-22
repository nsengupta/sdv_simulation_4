//! Reorder-buffer (ROB) barrier for one in-flight FSM turn (Phase 4).
//!
//! Every [`FsmEvent`] processed by the brain actor immediately gets a `TurnBarrier` pushed
//! onto the back of `VirtualCarRuntimeState::barrier_queue`.  Barriers are committed
//! strictly in **arrival order** from the front of the queue: the drain loop advances only
//! while the front barrier's `pending` set is empty (`is_complete`).
//!
//! ## Lifecycle
//!
//! ```text
//! begin_fsm_turn   ─── new() ───► [pending = {Headlamp}]
//!                                        │
//!         ZoneReady  ───────────► act_on_zone_reply()
//!                                        │  aborts live timer
//!         ZoneTellBackTimeout ──► act_on_zone_timeout()
//!                                        │  removes spent timer
//!                                        ├── Retry → store_retry_timer()
//!                                        └── GaveUp → caller: act_on_zone_reply(synthetic)
//!                                        │
//!         is_complete() == true ─────────► drain loop → into_resolved_turn()
//! ```

use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use ractor::concurrency::JoinHandle;
use ractor::MessagingErr;

use crate::digital_twin::{DigitalTwinCarVocabulary, ZoneReply};
use crate::fsm::{FsmEvent, ZoneId};
use crate::twin_runtime::twin_turn::ResolvedTurn;
use crate::twin_runtime::zone_replies::ZoneReplies;
use crate::twin_runtime::zone_tell_back::TellBackWait;
use crate::vehicle_state::HeadlampMessage;

/// Handle to the ractor timer task that sends `ZoneTellBackTimeout` to the brain.
pub(crate) type TellBackTimer = JoinHandle<Result<(), MessagingErr<DigitalTwinCarVocabulary>>>;

// ── TimeoutOutcome ────────────────────────────────────────────────────────────

/// Decision returned by [`TurnBarrier::act_on_zone_timeout`].
pub(crate) enum TimeoutOutcome {
    /// Retry budget remains; caller must re-tell the zone and call
    /// [`TurnBarrier::store_retry_timer`] with the fresh handle.
    Retry { next_attempt: u32 },
    /// All retries exhausted; caller must generate a synthetic reply and call
    /// [`TurnBarrier::act_on_zone_reply`] to close the zone's pending slot.
    GaveUp,
}

// ── TurnBarrier ──────────────────────────────────────────────────────────────

/// One FSM turn awaiting zone tell-back(s) before the drain loop may commit it.
///
/// All fields are private.  Identity (`turn_id`) is assigned at construction via
/// `VirtualCarRuntimeState::alloc_turn_id` and is immutable thereafter.  Read-only
/// getters expose the three "header" fields to the actor; all mutation goes through
/// the dedicated methods below.
pub(crate) struct TurnBarrier {
    /// Monotonically increasing identity assigned by `VirtualCarRuntimeState::alloc_turn_id`;
    /// used by zone twinlets to correlate `ZoneReady` / `ZoneTellBackTimeout` messages back
    /// to the correct brain turn.  Immutable after construction.
    turn_id: u64,
    /// The ingress event that opened this turn; forwarded unchanged to `ResolvedTurn`.
    /// Immutable after construction.
    event: FsmEvent,
    /// Wall-clock stamp captured when the event arrived; forwarded to `ResolvedTurn`
    /// so that zone tells during retries carry the *original* arrival time.
    /// Immutable after construction.
    now: Instant,

    /// Zones for which a reply has been requested but not yet received.
    /// `BTreeSet` gives deterministic iteration order across zones (important for
    /// future multi-zone turns where Phase 7 adds more assemblies).
    pending: BTreeSet<ZoneId>,
    /// Correlation state per zone: `tell_attempt` advances on each retry so that
    /// late-arriving replies from a superseded attempt are discarded as stale.
    zone_waits: HashMap<ZoneId, TellBackWait>,
    /// Live timer handles; `abort()` is called when a real reply arrives first,
    /// preventing a spurious `ZoneTellBackTimeout` from firing afterwards.
    zone_timers: HashMap<ZoneId, TellBackTimer>,
    /// Zone replies collected so far; handed to `into_resolved_turn` for commit.
    zone_replies: HashMap<ZoneId, ZoneReply>,
    /// Original zone message per zone, kept so the correct payload is re-sent on
    /// each retry without the actor having to reconstruct it from the event.
    zone_messages: HashMap<ZoneId, HeadlampMessage>,
}

impl TurnBarrier {
    /// Create a barrier for a turn that needs at least one zone tell-back.
    pub fn new(turn_id: u64, event: FsmEvent, now: Instant) -> Self {
        Self {
            turn_id,
            event,
            now,
            pending: BTreeSet::new(),
            zone_waits: HashMap::new(),
            zone_timers: HashMap::new(),
            zone_replies: HashMap::new(),
            zone_messages: HashMap::new(),
        }
    }

    /// Create a barrier for one assembly zone's lifecycle tell (`BecomeOn` / `BecomeOff`).
    ///
    /// Encapsulates the Phase 5 coupling: `zone_id` names both the pending slot and
    /// the `AssemblyZoneReady(zone_id)` event committed when the barrier drains.
    /// This keeps the two uses of `zone_id` in one place rather than duplicating them
    /// at every call site (Q3 resolution).
    pub fn new_for_assembly_zone(
        turn_id: u64,
        zone_id: ZoneId,
        message: HeadlampMessage,
        wait: TellBackWait,
        timer: TellBackTimer,
        now: Instant,
    ) -> Self {
        let mut barrier = Self::new(turn_id, FsmEvent::AssemblyZoneReady(zone_id), now);
        barrier.add_pending_zone(zone_id, message, wait, timer);
        barrier
    }

    // ── read-only header accessors ────────────────────────────────────────────

    /// The monotonic turn identity; used by the actor to correlate incoming zone messages.
    pub fn turn_id(&self) -> u64 {
        self.turn_id
    }

    /// The original event arrival timestamp; re-used on retries and forwarded to `ResolvedTurn`.
    pub fn now(&self) -> Instant {
        self.now
    }

    // ── query ────────────────────────────────────────────────────────────────

    /// `true` when all registered zones have replied; drain loop may commit this barrier.
    pub fn is_complete(&self) -> bool {
        self.pending.is_empty()
    }

    /// Whether the stored `tell_attempt` for `zone_id` matches the incoming attempt number.
    /// Used in `on_zone_ready` and `on_zone_timeout` to discard stale / mismatched messages.
    pub fn tell_attempt_matches(&self, zone_id: ZoneId, tell_attempt: u32) -> bool {
        self.zone_waits
            .get(&zone_id)
            .map_or(false, |w| w.tell_attempt == tell_attempt)
    }

    /// The original zone message stored for `zone_id`; needed to re-tell on timeout retry.
    pub fn zone_message(&self, zone_id: ZoneId) -> Option<HeadlampMessage> {
        self.zone_messages.get(&zone_id).copied()
    }

    // ── mutation ─────────────────────────────────────────────────────────────

    /// Register one zone as pending.  Called once per zone in `begin_fsm_turn`.
    /// Stores the message for retry, the correlation wait, and the live timer handle.
    pub fn add_pending_zone(
        &mut self,
        zone_id: ZoneId,
        message: HeadlampMessage,
        wait: TellBackWait,
        timer: TellBackTimer,
    ) {
        self.pending.insert(zone_id);
        self.zone_messages.insert(zone_id, message);
        self.zone_waits.insert(zone_id, wait);
        self.zone_timers.insert(zone_id, timer);
    }

    /// Store a fresh timer handle after a retry.  Does NOT abort the old one (already spent).
    pub fn store_retry_timer(&mut self, zone_id: ZoneId, timer: TellBackTimer) {
        self.zone_timers.insert(zone_id, timer);
    }

    /// Apply a received zone reply: remove from `pending`, store the reply, abort the live timer.
    pub fn act_on_zone_reply(&mut self, zone_id: ZoneId, reply: ZoneReply) {
        self.pending.remove(&zone_id);
        self.zone_replies.insert(zone_id, reply);
        // Timer is still live → must abort to prevent a spurious ZoneTellBackTimeout.
        if let Some(timer) = self.zone_timers.remove(&zone_id) {
            timer.abort();
        }
    }

    /// Handle a fired timer: remove the **spent** handle (no abort — it already fired),
    /// then decide retry vs. give-up.
    ///
    /// The `tell_attempt` guard prevents a timeout that was superseded by a real reply
    /// (and thus had its timer aborted in `act_on_zone_reply`) from being double-processed
    /// if a stale message still reaches the actor's mailbox between abort and draining.
    ///
    /// On `Retry`: caller must re-tell and call `store_retry_timer`.
    /// On `GaveUp`: caller must synthesise a reply and call `act_on_zone_reply`.
    pub fn act_on_zone_timeout(&mut self, zone_id: ZoneId, tell_attempt: u32) -> TimeoutOutcome {
        // Timer has already fired — drop the stale handle, no abort() needed.
        let _ = self.zone_timers.remove(&zone_id);

        let Some(wait) = self.zone_waits.get_mut(&zone_id) else {
            // No wait state means this zone was already resolved; treat as give-up.
            return TimeoutOutcome::GaveUp;
        };

        if wait.tell_attempt == tell_attempt && wait.retries_remaining > 0 {
            // Advance attempt counter so later replies from the old attempt are stale.
            wait.tell_attempt = wait.tell_attempt.saturating_add(1);
            wait.retries_remaining -= 1;
            TimeoutOutcome::Retry {
                next_attempt: wait.tell_attempt,
            }
        } else {
            // Attempt mismatch (stale) or no retries left.
            TimeoutOutcome::GaveUp
        }
    }

    /// Abort all live timers.  Called in `post_stop` during actor teardown.
    pub fn abort_all_timers(&mut self) {
        for (_, timer) in self.zone_timers.drain() {
            timer.abort();
        }
    }

    // ── drain ────────────────────────────────────────────────────────────────

    /// Consuming decomposition for the drain loop: packages `event`, `now`, and the
    /// collected zone replies into a [`ResolvedTurn`] ready for `commit_resolved_turn`.
    ///
    /// Called only after `is_complete()` returns `true` so that all zone replies
    /// are guaranteed to be present (either real or synthetic).
    pub fn into_resolved_turn(self) -> ResolvedTurn {
        let headlamp_reply = self.zone_replies.into_values().find_map(|r| match r {
            ZoneReply::Headlamp(hl) => Some(hl),
        });

        let zone_replies = headlamp_reply
            .map(ZoneReplies::with_headlamp_ingress)
            .unwrap_or_default();

        ResolvedTurn {
            ingress: self.event,
            now: self.now,
            zone_replies,
        }
    }
}

// ── PassthroughBarrier ────────────────────────────────────────────────────────

/// A barrier that carries no pending zone slots and is drainable immediately.
///
/// Distinct from [`TurnBarrier`] so that the type system prevents accidentally
/// calling `add_pending_zone` on a passthrough turn — the method simply does not
/// exist on this type.  Used for pure brain-state transitions (e.g. `PowerOn`,
/// `UpdateRpm`) where no zone message is emitted.
pub(crate) struct PassthroughBarrier {
    turn_id: u64,
    event: FsmEvent,
    now: Instant,
}

impl PassthroughBarrier {
    /// Create a passthrough barrier.  `is_complete()` is always `true`.
    pub fn new(turn_id: u64, event: FsmEvent, now: Instant) -> Self {
        Self { turn_id, event, now }
    }

    pub fn turn_id(&self) -> u64 { self.turn_id }

    pub fn is_complete(&self) -> bool { true }

    /// Consuming decomposition into a [`ResolvedTurn`] (no zone replies).
    pub fn into_resolved_turn(self) -> ResolvedTurn {
        ResolvedTurn {
            ingress: self.event,
            now: self.now,
            zone_replies: ZoneReplies::default(),
        }
    }
}

// ── BarrierEntry ─────────────────────────────────────────────────────────────

/// Either a zone-waiting [`TurnBarrier`] or an immediately-drainable [`PassthroughBarrier`].
///
/// The drain loop (`try_drain_barrier_queue`) holds a `VecDeque<BarrierEntry>` and commits
/// from the front in arrival order.
pub(crate) enum BarrierEntry {
    Waiting(TurnBarrier),
    Passthrough(PassthroughBarrier),
}

impl BarrierEntry {
    pub fn turn_id(&self) -> u64 {
        match self {
            BarrierEntry::Waiting(b) => b.turn_id(),
            BarrierEntry::Passthrough(b) => b.turn_id(),
        }
    }

    pub fn is_complete(&self) -> bool {
        match self {
            BarrierEntry::Waiting(b) => b.is_complete(),
            BarrierEntry::Passthrough(b) => b.is_complete(),
        }
    }

    pub fn into_resolved_turn(self) -> ResolvedTurn {
        match self {
            BarrierEntry::Waiting(b) => b.into_resolved_turn(),
            BarrierEntry::Passthrough(b) => b.into_resolved_turn(),
        }
    }

    /// Delegate to the inner [`TurnBarrier`] for zone-reply correlation.
    /// Returns `None` for passthrough entries (they have no pending zones).
    pub fn as_waiting_mut(&mut self) -> Option<&mut TurnBarrier> {
        match self {
            BarrierEntry::Waiting(b) => Some(b),
            BarrierEntry::Passthrough(_) => None,
        }
    }

    pub fn as_waiting(&self) -> Option<&TurnBarrier> {
        match self {
            BarrierEntry::Waiting(b) => Some(b),
            BarrierEntry::Passthrough(_) => None,
        }
    }

    pub fn abort_all_timers(&mut self) {
        if let BarrierEntry::Waiting(b) = self {
            b.abort_all_timers();
        }
    }
}
