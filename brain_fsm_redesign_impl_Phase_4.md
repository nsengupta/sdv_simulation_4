# Brain FSM Redesign — Phase 4 Implementation Plan
## `VecDeque<TurnBarrier>` Replaces `pending_turn`

**Status:** Draft — to be reviewed before implementation.  
**Depends on:** Phase 3 complete (generic `ZoneReady`/`ZoneSpontaneous`/`ZoneTellBackTimeout` in `DigitalTwinCarVocabulary`; handlers renamed to `on_zone_ready`, `on_zone_spontaneous`, `on_zone_timeout`).  
**Next phase:** Phase 5 — Wire `StartAssemblies`/`StopAssemblies` to `BecomeOn`/`BecomeOff` barriers.

---

## Phases 1–3 achieved so far

### Phase 1 — FSM Vocabulary

| Item | File | What changed |
|---|---|---|
| `FsmState::PreparingToStart` / `PreparingToStop` | `fsm/machineries.rs` | Two intermediate lifecycle states between `Off↔Idle` |
| `Operational::AssembliesReady` / `AssembliesStopped` | `fsm/machineries.rs` | Internal events that complete startup/shutdown sequences |
| `DomainAction::StartAssemblies` / `StopAssemblies` | `fsm/machineries.rs` | Actions emitted on `Off→PreparingToStart` and `Idle→PreparingToStop` |
| Transition table | `fsm/transition_map.rs` | `Off+PowerOn→PreparingToStart`; `PreparingToStart+AssembliesReady→Idle`; `Idle+PowerOff→PreparingToStop`; `PreparingToStop+AssembliesStopped→Off`; external events in both preparing states → self-loop |
| Domain types | `domain_types.rs`, `published.rs`, `diagnostic/mod.rs`, `gateway/transition_log.rs` | New states propagated to all projection / display layers |
| `power_on_to_idle()` / `power_off_to_off()` helpers | `test/mod.rs` | Absorb the Phase 1 shim (inject `AssembliesReady` / `AssembliesStopped`) from all actor tests |
| **RED→GREEN tests** | `test/fsm_preparation_contract.rs` | 8 new tests; 14 existing tests updated |

**Milestone tag:** `phase-1-fsm-vocabulary`

---

### Phase 2 — Headlamp Zone Alphabet

| Item | File | What changed |
|---|---|---|
| `HeadlampState::Ready` | `vehicle_state/front_headlamp.rs` | Assembly active, physical lamp dark — distinct from `Off` (not started) |
| `HeadlampMessage::BecomeOn` / `BecomeOff` | `vehicle_state/front_headlamp.rs` | Lifecycle control messages from Brain to zone |
| `ZoneId::Headlamp` | `fsm/machineries.rs` | Opaque zone identity token for future generalization |
| `PublishedHeadlampState::Ready` | `published.rs` | Ledger-serializable mirror |
| Lux guard changed `Off → Ready` | `vehicle_state/front_headlamp.rs` | `Off` now strictly means "not started"; only `Ready` responds to lux |
| `AckOff` / `ActuationIncomplete(On)` → `Ready` | `vehicle_state/front_headlamp.rs` | Assembly stays active after lamp turns off |
| `LightingUnsafe` detector fires on `Off` **or** `Ready` | `detectors/lighting_unsafe.rs` | Both states mean physical lamp is unlit |
| `OffRequested→Ready` in confirmed-direction probe | `controller/virtual_car_actor.rs` | Reflects new `AckOff` target state |
| `initial_headlamp_ctx` option | `controller/vehicle_controller.rs`, `virtual_car_actor.rs` | Phase 2–4 test shim; starts headlamp in `Ready` without the not-yet-wired `BecomeOn` |
| **RED→GREEN tests** | `test/headlamp_lifecycle_contract.rs` | 9 new tests; 7 existing tests updated across 5 files |

**Milestone tag:** `phase-2-headlamp-zone-alphabet`

---

### Phase 3 — Generic Zone Envelope in `DigitalTwinCarVocabulary`

| Item | File | What changed |
|---|---|---|
| `ZoneReply` enum | `digital_twin/mod.rs` | `ZoneReply::Headlamp(HeadlampZoneReply)` — forward-extensible wrapper |
| `ZoneSpontaneousEvent` enum | `digital_twin/mod.rs` | `ZoneSpontaneousEvent::Headlamp { direction, cause, reply }` |
| `ZoneReady { zone_id, turn_id, tell_attempt, reply }` | `digital_twin/mod.rs` | Replaces headlamp-specific `HeadlampZoneReady` |
| `ZoneSpontaneous { zone_id, event }` | `digital_twin/mod.rs` | Replaces `HeadlampZoneSpontaneous` |
| `ZoneTellBackTimeout { zone_id, turn_id, tell_attempt }` | `digital_twin/mod.rs` | Replaces `TellBackTimeout` |
| `handle()` match arms | `controller/virtual_car_actor.rs` | 5 exhaustive arms: `Fsm`, `ZoneReady`, `ZoneSpontaneous`, `ZoneTellBackTimeout`, `GetStatus` |
| Handlers renamed | `controller/virtual_car_actor.rs` | `on_zone_ready`, `on_zone_spontaneous`, `on_zone_timeout` (each unpacks zone-specific inner type before calling existing logic) |
| Headlamp actor reply updated | `twin_runtime/headlamp_actor.rs` | Sends `ZoneReady { zone_id: ZoneId::Headlamp, reply: ZoneReply::Headlamp(...) }` and `ZoneSpontaneous { zone_id, event: ZoneSpontaneousEvent::Headlamp { ... } }` |
| **No behavior change** | — | `pending_turn` still gates everything; routing changed, logic unchanged |
| **RED→GREEN tests** | `test/zone_envelope_contract.rs` | 3–4 new tests |

**Milestone tag:** `phase-3-generic-zone-envelope`

---

## What Phase 4 delivers

Replace the single `pending_turn: Option<PendingBrainTurn>` + `fsm_backlog: VecDeque<(FsmEvent, Instant)>` with a **`barrier_queue: VecDeque<TurnBarrier>`** — a reorder buffer (ROB) that preserves event arrival order regardless of zone reply order.

**Why this matters:**  
With `pending_turn`, a second FSM event arriving while a zone reply is in flight goes to `fsm_backlog` and can only commit after the first event's zone reply arrives. The current code is correct but serialized — only one event is in flight at a time.  
The `VecDeque<TurnBarrier>` design lets *N* events be in flight simultaneously. Each turn pushes its own barrier; the drain loop commits barriers in strict FIFO order from the front. A slow zone reply for turn 2 does not block turn 1 from committing.

**No new behavior** in this phase — the sequencing contract is unchanged for a single assembly. The structural change sets the foundation for Phase 5's wiring and Phase 7's concurrent multi-assembly barriers.

---

## New struct: `TurnBarrier`

Location: `crates/common/src/twin_runtime/zone_tell_back.rs` (or a new `turn_barrier.rs`).

```rust
use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use crate::fsm::{FsmEvent, ZoneId};
use crate::digital_twin::ZoneReply;
use crate::twin_runtime::zone_tell_back::{TellBackTimer, TellBackWait};

pub(crate) struct TurnBarrier {
    /// Monotone ID matching the `turn_id` carried in zone tell-back messages.
    pub turn_id: u64,
    /// The ingress event that triggered this turn.
    pub event: FsmEvent,
    /// Monotonic timestamp of the turn.
    pub now: Instant,
    /// Zones whose replies have not yet arrived.
    pub pending: BTreeSet<ZoneId>,
    /// Per-zone tell-back wait state (retry counter, etc.).
    pub zone_waits: HashMap<ZoneId, TellBackWait>,
    /// Per-zone ractor timers (armed on tell; aborted on reply or timeout).
    pub zone_timers: HashMap<ZoneId, TellBackTimer>,
    /// Zone replies received so far for this turn.
    pub zone_replies: HashMap<ZoneId, ZoneReply>,
}

impl TurnBarrier {
    /// True when all zone replies for this turn have arrived.
    pub fn is_complete(&self) -> bool {
        self.pending.is_empty()
    }

    /// Register one zone as awaited by this barrier.
    /// Atomically inserts into `pending`, `zone_waits`, and `zone_timers`.
    /// Called once per zone in `begin_fsm_turn` (single zone today; loop in Phase 7).
    pub fn add_pending_zone(
        &mut self,
        zone_id: ZoneId,
        wait: TellBackWait,
        timer: TellBackTimer,
    ) {
        self.pending.insert(zone_id);
        self.zone_waits.insert(zone_id, wait);
        self.zone_timers.insert(zone_id, timer);
    }

    /// Store a fresh timer handle after a tell-back retry.
    /// Called by `on_zone_timeout` in the `Retry` branch, after re-telling the zone
    /// and arming a new ractor timer. Keeps `zone_timers` private to `TurnBarrier`.
    pub fn store_retry_timer(&mut self, zone_id: ZoneId, timer: TellBackTimer) {
        self.zone_timers.insert(zone_id, timer);
    }

    /// Called when a zone's tell-back reply arrives (timer still live).
    /// Removes the zone from `pending`, stores the reply, and aborts the
    /// live ractor timer so it cannot fire after the reply is already processed.
    pub fn act_on_zone_reply(&mut self, zone_id: ZoneId, reply: ZoneReply) {
        self.pending.remove(&zone_id);
        self.zone_replies.insert(zone_id, reply);
        // Timer is still live — abort it before dropping the handle.
        if let Some(timer) = self.zone_timers.remove(&zone_id) {
            timer.abort();
        }
    }

    /// Called when the ractor timer for a zone fires (reply did not arrive in time).
    /// The timer is already spent, so the handle is unconditionally removed first.
    /// Returns `TimeoutOutcome` telling the caller whether to re-tell or give up.
    pub fn act_on_zone_timeout(
        &mut self,
        zone_id: ZoneId,
        tell_attempt: u32,
    ) -> TimeoutOutcome {
        // The fired timer is spent — remove the stale handle unconditionally.
        // (No abort() call: the timer has already delivered its message.)
        let _spent = self.zone_timers.remove(&zone_id);

        let wait = self.zone_waits.get_mut(&zone_id).unwrap();
        if wait.can_retry(tell_attempt) {
            TimeoutOutcome::Retry
            // Caller re-tells the zone; stores new handle via store_retry_timer().
        } else {
            // Give up: synthesize an unresponsive reply and close the zone's slot.
            self.pending.remove(&zone_id);
            self.zone_replies.insert(zone_id, ZoneReply::unresponsive(zone_id));
            TimeoutOutcome::GaveUp
        }
    }

    /// Consuming decomposition for the drain loop.
    /// Packages the fields the commit step needs; field layout stays private.
    pub fn into_resolved_turn(self) -> ResolvedTurn {
        ResolvedTurn {
            ingress: self.event,
            now: self.now,
            zone_replies: ZoneReplies::from_barrier_replies(self.zone_replies),
        }
    }
}

/// Outcome of [`TurnBarrier::act_on_zone_timeout`].
pub(crate) enum TimeoutOutcome {
    /// The wait budget allows another attempt; caller must re-tell and store the
    /// new timer handle via [`TurnBarrier::store_retry_timer`].
    Retry,
    /// Retries exhausted; a synthetic unresponsive reply has been stored in the barrier.
    GaveUp,
}
```

`TellBackWait` is **unchanged** from Phase 3 — it becomes one value per zone in `zone_waits` instead of a single field on `PendingBrainTurn`.

---

## `VirtualCarRuntimeState` changes

```rust
// Before (Phase 3):
pub struct VirtualCarRuntimeState {
    // ...
    pending_turn: Option<PendingBrainTurn>,
    fsm_backlog: VecDeque<(FsmEvent, Instant)>,
    // ...
}

// After (Phase 4):
pub struct VirtualCarRuntimeState {
    // ...
    barrier_queue: VecDeque<TurnBarrier>,
    // (pending_turn and fsm_backlog removed)
    // ...
}
```

`PendingBrainTurn` is deleted entirely along with `pump_fsm_backlog`.

---

## Drain loop

```rust
fn try_drain_barrier_queue(
    runtime_state: &mut VirtualCarRuntimeState,
    // ... actuation manager, sinks, etc.
) -> Result<(), ActorProcessingErr> {
    while let Some(front) = runtime_state.barrier_queue.front() {
        if !front.is_complete() {
            break; // front not ready — stop; preserve order
        }
        let committed = runtime_state.barrier_queue.pop_front().unwrap();
        commit_resolved_turn(
            runtime_state.twin_car.current_state(),
            runtime_state.twin_car.context(),
            committed.into_resolved_turn(),
        );
        // ... record, actuation, diagnostics
    }
    Ok(())
}
```

Called from `on_zone_ready`, `on_zone_spontaneous`, and `on_zone_timeout` after updating the barrier.

---

## Updated handler signatures

### `begin_fsm_turn` (renamed from the implicit entry point)

```rust
fn begin_fsm_turn(
    myself: &ActorRef<DigitalTwinCarVocabulary>,
    runtime_state: &mut VirtualCarRuntimeState,
    event: FsmEvent,
    now: Instant,
) -> Result<(), ActorProcessingErr> {
    let turn_id = runtime_state.next_turn_id;
    runtime_state.next_turn_id += 1;

    let zone_msg = zone_message_for_event(&event, runtime_state.twin_car.current_state());

    if let Some((zone_id, msg)) = zone_msg {
        let mut barrier = TurnBarrier::new(turn_id, event, now);
        tell_zone(zone_id, msg, turn_id, /* tell_attempt= */ 0, ...);
        let (wait, timer) = make_zone_wait_and_timer(myself, zone_id, turn_id, ...);
        barrier.add_pending_zone(zone_id, wait, timer);
        runtime_state.barrier_queue.push_back(barrier);
    } else {
        // No zone message needed: push an immediately-complete barrier
        let barrier = TurnBarrier::new_complete(turn_id, event, now);
        runtime_state.barrier_queue.push_back(barrier);
        try_drain_barrier_queue(runtime_state, ...)?;
    }
    Ok(())
}
```

### `on_zone_ready`

```rust
async fn on_zone_ready(
    myself: &ActorRef<DigitalTwinCarVocabulary>,
    runtime_state: &mut VirtualCarRuntimeState,
    zone_id: ZoneId,
    turn_id: u64,
    tell_attempt: u32,
    reply: ZoneReply,
) -> Result<(), ActorProcessingErr> {
    if let Some(barrier) = runtime_state.barrier_queue
        .iter_mut()
        .find(|b| b.turn_id == turn_id)
    {
        barrier.act_on_zone_reply(zone_id, reply);
    }
    try_drain_barrier_queue(runtime_state, ...)?;
    Ok(())
}
```

### `on_zone_timeout`

```rust
async fn on_zone_timeout(
    myself: &ActorRef<DigitalTwinCarVocabulary>,
    runtime_state: &mut VirtualCarRuntimeState,
    zone_id: ZoneId,
    turn_id: u64,
    tell_attempt: u32,
) -> Result<(), ActorProcessingErr> {
    if let Some(barrier) = runtime_state.barrier_queue
        .iter_mut()
        .find(|b| b.turn_id == turn_id)
    {
        match barrier.act_on_zone_timeout(zone_id, tell_attempt) {
            TimeoutOutcome::Retry => {
                // Re-tell the zone and store the new live timer handle.
                tell_zone(zone_id, msg, turn_id, tell_attempt + 1, ...);
                let new_timer = arm_zone_timer(myself, zone_id, turn_id, tell_attempt + 1, ...);
                barrier.store_retry_timer(zone_id, new_timer);
            }
            TimeoutOutcome::GaveUp => {
                // Synthetic reply already stored inside act_on_zone_timeout; nothing else to do.
            }
        }
    }
    try_drain_barrier_queue(runtime_state, ...)?;
    Ok(())
}
```

### `on_zone_spontaneous`

```rust
async fn on_zone_spontaneous(
    runtime_state: &mut VirtualCarRuntimeState,
    zone_id: ZoneId,
    event: ZoneSpontaneousEvent,
) -> Result<(), ActorProcessingErr> {
    // ZoneSpontaneous carries NO turn_id — it is the zone's own internal ACK
    // timer firing (e.g. headlamp AckWaitElapsed), not a brain tell-back timeout.
    // No TurnBarrier is looked up or mutated here.
    // Process the spontaneous event directly (update context, run detector, record).
    match event {
        ZoneSpontaneousEvent::Headlamp { direction, cause, reply } => {
            on_headlamp_spontaneous(runtime_state, direction, cause, reply)?;
        }
    }
    // Drain in case a barrier at the front was waiting only on this zone and
    // another reply that has since arrived; the spontaneous event itself is
    // not a barrier reply, but it may unblock ledger commits downstream.
    try_drain_barrier_queue(runtime_state, ...)?;
    Ok(())
}
```

`ZoneSpontaneous` is the only message path where **no `TurnBarrier` field is touched at all**. The three other paths (`on_zone_ready`, `on_zone_timeout`, `begin_fsm_turn`) all mutate at least one barrier via the methods above.

---

## RED tests (new file: `test/turn_barrier_contract.rs`)

### Test 1 — `two_zone_directed_events_commit_in_arrival_order`

```
Given: Brain actor with headlamp in Ready state.
When:  Inject UpdateAmbientLux(20) [turn 1], then immediately UpdateAmbientLux(100) [turn 2].
       Zone replies arrive: turn 2 reply first, then turn 1 reply.
Then:  Ledger row for turn 1 commits before turn 2 (record_seq 1 < 2).
       Ledger turn 1 old_ctx.headlamp.state == Ready; next state OnRequested.
       Ledger turn 2 old_ctx == turn 1's committed ctx (not stale).
```

This is the core ordering invariant — two events queued; slow reply for turn 1 must not let turn 2 overtake it.

### Test 2 — `backlogged_event_committed_after_barrier_drains`

```
Given: Brain actor; headlamp in Ready.
When:  Inject UpdateAmbientLux(20) [zone-directed, turn 1].
       Before zone reply arrives, inject UpdateRpm(2000) [no zone needed, turn 2].
Then:  UpdateRpm turn 2 barrier is complete immediately (pending = {}).
       UpdateRpm does NOT drain until turn 1's barrier drains first.
       After zone reply for turn 1 arrives: both barriers drain in order.
       Ledger: seq 1 = lux event, seq 2 = rpm event.
       Ledger seq 2 old_ctx.headlamp.state == OnRequested (reflects committed turn 1).
```

### Test 3 — `zone_tell_back_timeout_retries_apply_to_correct_barrier`

```
Given: Two consecutive zone-directed events in flight (turns 1 and 2).
When:  ZoneTellBackTimeout arrives for turn 1, zone_id = Headlamp.
Then:  Only turn 1's barrier is affected (retry counter incremented or synthetic reply inserted).
       Turn 2's barrier is unchanged.
       After timeout resolution, drain loop runs; barriers commit in order.
```

### Test 4 — `drain_loop_stops_at_incomplete_front_barrier`

```
Given: barrier_queue = [Barrier(turn=1, pending={Headlamp}), Barrier(turn=2, pending={})].
When:  Zone reply for turn 2 arrives (turn 1 still pending).
Then:  Turn 2 barrier is complete but drain loop stops at turn 1 (front, incomplete).
       Nothing commits. Queue length remains 2.
When:  Zone reply for turn 1 arrives.
Then:  Both barriers drain. Two ledger rows emitted in order (turn 1 then turn 2).
```

---

## Files changed

| File | Change |
|---|---|
| `twin_runtime/zone_tell_back.rs` (or new `turn_barrier.rs`) | New `TurnBarrier` struct; methods `add_pending_zone`, `act_on_zone_reply`, `act_on_zone_timeout`, `store_retry_timer`, `into_resolved_turn`, `is_complete`; `TimeoutOutcome` enum |
| `twin_runtime/controller/virtual_car_actor.rs` | Remove `pending_turn` + `fsm_backlog`; add `barrier_queue: VecDeque<TurnBarrier>`; rewrite `begin_fsm_turn`, `on_zone_ready`, `on_zone_timeout`; add `try_drain_barrier_queue`; delete `PendingBrainTurn` + `pump_fsm_backlog` |
| `test/turn_barrier_contract.rs` | **NEW** — 4 RED→GREEN ordering tests |

All other files unchanged in Phase 4.

---

## Discussion checkpoint after Phase 4

1. **All tests green.** `cargo test -p common` — existing 122+ tests plus 4 new ordering tests all pass.
2. **Zero warnings.** `PendingBrainTurn`, `pump_fsm_backlog`, and `fsm_backlog` are fully deleted with no dead-code residue.
3. **Ordering invariant manually verified.** In Test 1, inspect the ledger output: `record_seq` must be 1 for the lux event and 2 for the second event, with `old_ctx` values reflecting committed state at the time of each commit.
4. **`IgnitionOffReset` variant** is still present in the code at this point (deleted in Phase 6). Confirm it does not panic via `scenario_cold_start_get_status_shows_off`.
5. **Confirm `ZoneReplies::from_barrier_replies`** signature — should it move ownership (`HashMap<ZoneId, ZoneReply>`) or borrow? Agree before Phase 5 which calls this on every drain.
6. **Confirm `TurnBarrier` module location** — `zone_tell_back.rs` extension vs. a standalone `turn_barrier.rs` — before Phase 5 imports it.
