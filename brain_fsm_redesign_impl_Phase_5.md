# Brain FSM Redesign — Phase 5 Implementation Plan
## Wire `StartAssemblies`/`StopAssemblies` to `BecomeOn`/`BecomeOff` Barriers

**Status:** Design reviewed and refined (2026-06-22) — ready for implementation.  
**Depends on:** Phase 4 complete (`TurnBarrier` / `barrier_queue` ROB; `alloc_turn_id`).  
**Next phase:** Phase 6 — State-aware zone routing; delete speculative execution and `IgnitionOffReset`.

---

## Phases 1–4 achieved so far

| Phase | Tag | Core deliverable |
|---|---|---|
| 1 | `phase-1-fsm-vocabulary` | `PreparingToStart`/`PreparingToStop` states; `StartAssemblies`/`StopAssemblies` actions; `AssembliesReady`/`AssembliesStopped` internal events; 8 RED→GREEN tests |
| 2 | `phase-2-headlamp-zone-alphabet` | `HeadlampState::Ready`; `HeadlampMessage::BecomeOn`/`BecomeOff`; `ZoneId::Headlamp`; 9 RED→GREEN tests |
| 3 | `phase-3-generic-zone-envelope` | `ZoneReply`, `ZoneSpontaneousEvent`, `ZoneReady`/`ZoneSpontaneous`/`ZoneTellBackTimeout` in `DigitalTwinCarVocabulary`; handler rename; 3 RED→GREEN tests |
| 4 | `phase-4-reorder-buffer-barrier-queue` | `TurnBarrier`; `barrier_queue: VecDeque<TurnBarrier>`; HOB drain loop; `alloc_turn_id`; 4 RED→GREEN tests |

---

## What Phase 5 delivers

**The startup/shutdown lifecycle is currently a manual shim.**
`power_on_to_idle` injects `FsmEvent::Internal(Operational::AssembliesReady)` by hand after
`PowerOn`; `power_off_to_off` similarly injects `AssembliesStopped`.  The FSM vocabulary
(`StartAssemblies`, `StopAssemblies`) and the assembly alphabet (`BecomeOn`, `BecomeOff`) were
wired in Phases 1 and 2, but the brain actor's `apply_committed_quiescence` still has a no-op
stub for these two actions.

**Phase 5 closes this gap:**

1. `DomainAction::StartAssemblies` — actor sends `BecomeOn` to each managed assembly and
   creates one `TurnBarrier` per assembly (waiting for `ZoneReady`). FSM stays in `PreparingToStart`.
2. As each `ZoneReady` arrives, `on_zone_ready` (unchanged) routes it through its barrier.
   When a barrier drains, `commit_resolved_turn` feeds a new external event
   `FsmEvent::AssemblyZoneReady(zone_id)` into the FSM.
3. The FSM transition table handles `(PreparingToStart, AssemblyZoneReady(zone_id))` directly:
   removes `zone_id` from `ctx.pending_assemblies`; transitions to `Idle` only when the set
   becomes empty.
4. `DomainAction::StopAssemblies` — same pattern with `BecomeOff` / `FsmState::PreparingToStop`.
5. The `power_on_to_idle` and `power_off_to_off` shims are simplified (manual inject removed).

---

## Design evolution — why not `Internal(AssembliesReady)` in the barrier event?

Three designs were considered and discarded before settling on the current one.

### Discarded: actor creates barrier with `event = Internal(AssembliesReady)`

The `TurnBarrier.event` field is the `FsmEvent` committed when the barrier drains.  If this is
`Internal(AssembliesReady)`, the actor layer names an FSM-internal event — a strict layer
violation.  Additionally, `zone_turn` explicitly ignores `Internal(_)` events (line 112,
`zone_turn.rs`), so the `BecomeOn` zone reply would be silently dropped and the headlamp
context never updated.

### Discarded: `detect_internal_after_hop` extended for assembly readiness

`detect_internal_after_hop` fires internal events on the same thread as the quiescence loop.
Adding a `PreparingToStart + headlamp.state==Ready → fire AssembliesReady` detector would avoid
actor knowledge of `AssembliesReady` — but required a new `FsmEvent::AssemblyZoneSynced` as
the neutral trigger event to get the zone reply into the context via `zone_turn`.  The
`AssemblyZoneSynced` event was artificial (no domain meaning) and added surface area with no
benefit once the third design was identified.

### Adopted: `FsmEvent::AssemblyZoneReady(ZoneId)` as a proper external event

Assembly zone replies are *external signals* — they arrive from assembly actors via the mailbox,
exactly like `FrontHeadlampOnAck` or `UpdateAmbientLux`.  Making them a proper `FsmEvent`
variant is the correct classification.  The FSM transition table — not a detector, not the actor
— counts down the pending assemblies and transitions when the set becomes empty.

**Layer boundary:**

| Knowledge | Layer |
|---|---|
| `StartAssemblies → tell BecomeOn to each MANAGED assembly` | Actor layer |
| `ZoneReady(zone_id) → begin_fsm_turn(AssemblyZoneReady(zone_id))` | Actor layer (via existing `on_zone_ready`) |
| `pending_assemblies` set and the counting logic | FSM transition table (pure computation) |
| `(PreparingToStart, AssemblyZoneReady) → Idle when pending empty` | FSM transition table |

The actor never names `AssembliesReady` or `AssembliesStopped`.

---

## Design review refinements (2026-06-22)

Five questions were raised and resolved during design review.  The answers below are binding on
the implementation; corresponding action items are tracked in the section "Additional changes
from review".

### Q1 — Why does `begin_fsm_turn` call `fsm_event_headlamp_message` at runtime?

`begin_fsm_turn` is called exclusively from the `Fsm` arm in `handle()`, which receives only
user events (`PowerOn`, `UpdateAmbientLux`, etc.).  `AssemblyZoneReady` events never go through
`begin_fsm_turn`; they enter via `commit_resolved_turn` from the drain loop.

The `fsm_event_headlamp_message` lookup is necessary because user events are heterogeneous:
some need a zone tell (`UpdateAmbientLux` → `AmbientLux` to Headlamp), some do not (`PowerOn`,
`TimerTick`).  The correct long-term fix is for the controller/gateway layer to produce a typed
`FsmTurn` (zone-directed vs. passthrough) so `handle()` and `begin_fsm_turn` receive a
pre-classified value instead of dispatching at runtime.  **This is a Phase 6 item.**

**Phase 5 interim:** rename `fsm_event_headlamp_message` → `user_event_to_headlamp_tell` to
make the scope of the function explicit in the codebase.

### Q2 — `new_passthrough` is a phantom alias of `new`

`TurnBarrier::new_passthrough` simply delegates to `Self::new(...)`.  `new` already creates an
empty `pending` set, so `is_complete()` is immediately true; the two constructors are
behaviourally identical with no enforcement at the type level.

**Resolution:** delete `new_passthrough`.  Introduce a distinct `PassthroughBarrier` type that
cannot have `add_pending_zone` called on it.  The drain loop accepts a `BarrierEntry` enum
(or a trait) covering both cases.  **Scoped to Phase 5.**

### Q3 — `TurnBarrier::new` + `add_pending_zone` repeat `zone_id` twice for assembly barriers

```rust
// Current (plan): zone_id appears in both the event and the pending call
let barrier = TurnBarrier::new(T42, FsmEvent::AssemblyZoneReady(ZoneId::Headlamp), now);
barrier.add_pending_zone(ZoneId::Headlamp, ...);
```

The two uses serve different roles (`event` = what FSM commits; `pending` = what actor waits
for), but at the call site `zone_id` is always the same value.  A dedicated constructor
encapsulates the coupling:

```rust
TurnBarrier::new_for_assembly_zone(turn_id, zone_id, message, wait, timer, now)
// internally: event = FsmEvent::AssemblyZoneReady(zone_id)
//             + add_pending_zone(zone_id, message, wait, timer)
```

**Phase 7 note:** the `message` parameter will NOT remain `HeadlampMessage`.  When Wiper and
other assemblies are added, a generic zone lifecycle message type (`ZoneLifecycleMessage` or
similar enum) is needed.  For Phase 5 `HeadlampMessage` is acceptable; Phase 7 generalises it.

### Q4 — Why `ctx.pending_assemblies` if `barrier_queue` already tracks in-flight zones?

The two structures serve different layers for different consumers:

| Structure | Layer | Consumer |
|---|---|---|
| `barrier_queue` entries (`pending: BTreeSet<ZoneId>`) | Actor | Tell/retry/timeout machinery; drain-loop |
| `ctx.pending_assemblies: BTreeSet<ZoneId>` | FSM | Pure transition decision: stay vs. advance |

The FSM is a pure function `(FsmState, VehicleContext, FsmEvent) → StepResult` with no access
to `barrier_queue`.  Without `ctx.pending_assemblies` the FSM cannot distinguish "one of two
assemblies replied" from "the only assembly replied."  The dual representation is necessary and
correct.

### Q5 — Should assembly barriers be a queue (ordered) or a set (unordered)?

Order among assembly barriers themselves is logically irrelevant — `ctx.pending_assemblies`
counts down correctly in any order.  However, the unified `barrier_queue` provides a critical
benefit for free: **user events that arrive during startup are gated behind assembly barriers
via the HOB invariant**.  A separate set would require an explicit "startup in progress" gate
to achieve the same effect.  The unified queue is the right structure.

### Additional note — HOB bypass for no-op transitions (Phase 6 item)

In Phase 5, `UpdateAmbientLux` during `PreparingToStart` is zone-directed (sends `AmbientLux`
to Headlamp) because `user_event_to_headlamp_tell` has no state awareness.  After Phase 6
introduces state-aware zone routing, that event becomes a passthrough barrier in
`PreparingToStart`.  The question arose: could a passthrough barrier skip ahead of assembly
barriers (since the FSM transition is a self-loop)?

**Decision: no.**  Allowing bypasses would violate ledger event-arrival order and couple the
drain loop to FSM transition semantics.  Passthrough barriers drain trivially fast under HOB
in order — no bypass is needed.

---

## New data structures

### `FsmEvent::AssemblyZoneReady(ZoneId)` — `crates/common/src/fsm/machineries.rs`

```rust
pub enum FsmEvent {
    // ... existing variants unchanged ...
    /// An assembly zone has acknowledged a `BecomeOn` or `BecomeOff` tell.
    /// Processed by the FSM transition table to count down `ctx.pending_assemblies`.
    /// This is an *external* event (arrives from the assembly actor mailbox),
    /// not an `Internal` variant.
    AssemblyZoneReady(ZoneId),
}
```

`Operational::AssembliesReady` and `Operational::AssembliesStopped` remain in place for now
(they appear in doc comments and may be used by the ledger/observer layer).  Whether they are
removed is a Phase 6 decision.

### `VehicleContext.pending_assemblies` — `crates/common/src/vehicle_state/mod.rs`

```rust
pub struct VehicleContext {
    // ... existing fields unchanged ...
    /// Assemblies still awaiting `ZoneReady` during `PreparingToStart` or `PreparingToStop`.
    /// Empty in all other states.  Initialised by the `Off → PreparingToStart` transition;
    /// cleared by the final `AssemblyZoneReady` that empties the set.
    pub pending_assemblies: BTreeSet<ZoneId>,
}
```

`Default::default()` gives an empty `BTreeSet`, so all existing tests that create
`VehicleContext::default()` are unaffected without any code change.

---

## Turn ID sequence — before and after Phase 5

| Turn ID | Phase 4 (shim) | Phase 5 (wired) |
|---|---|---|
| 1 | `PowerOn` passthrough (no zone) | `PowerOn` passthrough (no zone) — **unchanged** |
| 2 | `AssembliesReady` passthrough (manually injected) | **Startup barrier** — `AssemblyZoneReady(Headlamp)` event, `pending = {Headlamp}`, tells `BecomeOn` |
| 3+ | First user event | First user event — **unchanged** |

**`FIRST_USER_TURN = 3` is preserved.**  The change is in the *nature* of turn 2: it moves
from an instant-passthrough to a zone-waiting barrier.  The silent-headlamp tests in
`turn_barrier_contract.rs` must inject `ZoneReady(turn_id=2)` to complete the startup barrier.

---

## `MANAGED_ASSEMBLIES` constant

```rust
/// Compile-time list of assemblies the brain actor coordinates.
///
/// Phase 8 replaces this constant with assembly IDs embedded directly in
/// `FsmState::PreparingToStart { assemblies }` and `PreparingToStop { assemblies }`,
/// making the FSM the single source of topology truth.  For Phases 5–7 this
/// constant is the sole place where `ZoneId::Headlamp` (and future `ZoneId::Wiper`)
/// is listed as a managed assembly.
const MANAGED_ASSEMBLIES: &[ZoneId] = &[ZoneId::Headlamp];
```

Location: top of `impl VirtualCarActor` in `virtual_car_actor.rs`.

---

## Code changes

### `crates/common/src/fsm/machineries.rs`

Add `AssemblyZoneReady(ZoneId)` to `FsmEvent` (see above).

### `crates/common/src/vehicle_state/mod.rs`

Add `pending_assemblies: BTreeSet<ZoneId>` to `VehicleContext` (see above).

### `crates/common/src/fsm/transition_map.rs` (or equivalent step function)

Add transition arms for `AssemblyZoneReady`:

```rust
// Startup path: each assembly reply counts down pending_assemblies.
(FsmState::PreparingToStart, FsmEvent::AssemblyZoneReady(zone_id)) => {
    let mut ctx2 = ctx.clone();
    ctx2.pending_assemblies.remove(zone_id);
    let next = if ctx2.pending_assemblies.is_empty() {
        FsmState::Idle
    } else {
        FsmState::PreparingToStart
    };
    StepResult { next_state: next, modified_ctx: ctx2, actions: vec![], ... }
}

// Shutdown path: mirror with PreparingToStop → Off.
(FsmState::PreparingToStop, FsmEvent::AssemblyZoneReady(zone_id)) => {
    let mut ctx2 = ctx.clone();
    ctx2.pending_assemblies.remove(zone_id);
    let next = if ctx2.pending_assemblies.is_empty() {
        FsmState::Off
    } else {
        FsmState::PreparingToStop
    };
    StepResult { next_state: next, modified_ctx: ctx2, actions: vec![], ... }
}
```

Both lifecycle entry transitions initialise `pending_assemblies`:

```rust
// Startup entry
(FsmState::Off, FsmEvent::PowerOn) => {
    let mut ctx2 = ctx.clone();
    ctx2.pending_assemblies = BTreeSet::from([ZoneId::Headlamp]); // topology for Phase 5
    StepResult {
        next_state: FsmState::PreparingToStart,
        modified_ctx: ctx2,
        actions: vec![DomainAction::StartAssemblies],
        ...
    }
}

// Shutdown entry — must also initialise pending_assemblies (mirrors startup).
// Without this, the PreparingToStop + AssemblyZoneReady arm removes from an empty
// set and incorrectly transitions to Off even in Phase 7 with multiple assemblies.
(FsmState::Idle, FsmEvent::PowerOff) => {
    let mut ctx2 = ctx.clone();
    ctx2.pending_assemblies = BTreeSet::from([ZoneId::Headlamp]); // topology for Phase 5
    StepResult {
        next_state: FsmState::PreparingToStop,
        modified_ctx: ctx2,
        actions: vec![DomainAction::StopAssemblies],
        ...
    }
}
```

Note: hardcoding `{Headlamp}` is intentional for Phase 5.  Phase 8 refactors this into
a config-driven or state-embedded topology.

Also **delete** the two arms that become dead code after Phase 5:

```rust
// DELETE — no code path produces Internal(AssembliesReady) after Phase 5.
(FsmState::PreparingToStart, FsmEvent::Internal(Operational::AssembliesReady)) => Idle

// DELETE — no code path produces Internal(AssembliesStopped) after Phase 5.
(FsmState::PreparingToStop, FsmEvent::Internal(Operational::AssembliesStopped)) => Off
```

`Operational::AssembliesReady` and `AssembliesStopped` remain as enum variants (they appear in
doc comments); only the transition arms are removed.

### `crates/common/src/twin_runtime/zone_turn.rs`

Add a case for `AssemblyZoneReady` to apply the zone reply into the context (so the headlamp
state is updated in the ledger even though the transition logic only checks `pending_assemblies`):

```rust
FsmEvent::AssemblyZoneReady(zone_id) => {
    match zone_id {
        ZoneId::Headlamp => {
            if let Some(reply) = zone_replies.headlamp.ingress.as_ref() {
                next.headlamp = reply.ctx.clone();
                headlamp_outcomes.extend(reply.outcomes.clone());
            }
        }
    }
}
```

### `crates/common/src/twin_runtime/controller/virtual_car_actor.rs`

**1. Add `MANAGED_ASSEMBLIES`** (see above).

**2. Wire `StartAssemblies` in `apply_committed_quiescence`**

One barrier per assembly via `TurnBarrier::new_for_assembly_zone` (Q3 — `zone_id` named once):

```rust
DomainAction::StartAssemblies => {
    let now = Instant::now();
    for &zone_id in MANAGED_ASSEMBLIES {
        let turn_id = runtime_state.alloc_turn_id();
        let brain = &runtime_state.self_ref;  // stored in pre_start (see below)
        let msg = HeadlampMessage::BecomeOn;
        let wait = TellBackWait::new(turn_id);
        tell_headlamp_zone(&runtime_state.headlamp_actor, brain, turn_id, 0, msg, now)?;
        let timer = Self::arm_tell_back_timer(brain, turn_id, 0);
        // new_for_assembly_zone: derives event = AssemblyZoneReady(zone_id)
        //                        and registers zone as pending — zone_id named once.
        let barrier = TurnBarrier::new_for_assembly_zone(turn_id, zone_id, msg, wait, timer, now);
        runtime_state.barrier_queue.push_back(barrier);
    }
}
```

**3. Wire `StopAssemblies`** — mirror with `BecomeOff`:

```rust
DomainAction::StopAssemblies => {
    let now = Instant::now();
    for &zone_id in MANAGED_ASSEMBLIES {
        let turn_id = runtime_state.alloc_turn_id();
        let brain = &runtime_state.self_ref;
        let msg = HeadlampMessage::BecomeOff;
        let wait = TellBackWait::new(turn_id);
        tell_headlamp_zone(&runtime_state.headlamp_actor, brain, turn_id, 0, msg, now)?;
        let timer = Self::arm_tell_back_timer(brain, turn_id, 0);
        let barrier = TurnBarrier::new_for_assembly_zone(turn_id, zone_id, msg, wait, timer, now);
        runtime_state.barrier_queue.push_back(barrier);
    }
}
```

**4. Add `self_ref` to `VirtualCarRuntimeState`** (needed by the loops above):

```rust
pub struct VirtualCarRuntimeState {
    twin_car: DigitalTwinCar,
    headlamp_actor: ActorRef<HeadlampActorMsg>,
    self_ref: ActorRef<DigitalTwinCarVocabulary>,  // stored in pre_start; idiomatic actor self-ref
    next_turn_id: u64,
    // ... rest unchanged
}
```

In `pre_start`:
```rust
Ok(VirtualCarRuntimeState {
    self_ref: myself.clone(),
    // ...
})
```

No signature changes to `try_drain_barrier_queue`, `commit_resolved_turn`, or
`apply_committed_quiescence`.

### `crates/common/src/twin_runtime/headlamp_actor.rs`

Verify (and add if missing) that `BecomeOn` is handled when the headlamp is **already in
`Ready`** — it must still reply with `ZoneReady { state: Ready }`.  Similarly verify
`BecomeOff` is handled in any state.

### `crates/common/src/twin_runtime/controller/actuation_manager.rs`

Remove the `StartAssemblies | StopAssemblies => {}` no-op arm — `apply_committed_quiescence`
intercepts these before they reach `actuation_manager.execute()`.

### `crates/common/src/test/mod.rs`

Remove the manual inject from both shims:

```rust
/// Power on and wait for Idle automatically via the StartAssemblies barrier
/// (BecomeOn → ZoneReady → AssemblyZoneReady → Idle).
pub async fn power_on_to_idle(controller: &VehicleController) {
    controller.send_power_on().await.expect("power on");
    wait_fsm_state(controller, FsmState::Idle, Duration::from_millis(500)).await;
}

/// Power off and wait for Off automatically via the StopAssemblies barrier.
pub async fn power_off_to_off(controller: &VehicleController) {
    controller.send_power_off().await.expect("power off");
    wait_fsm_state(controller, FsmState::Off, Duration::from_millis(500)).await;
}
```

### `crates/common/src/test/turn_barrier_contract.rs`

The silent-headlamp tests will hang after Phase 5 because `power_on_to_idle` no longer injects
`AssembliesReady` manually.

**Fix: introduce a `boot_silent` local helper:**

```rust
/// Turn ID for the startup barrier (StartAssemblies → BecomeOn tell).
/// PowerOn occupies turn 1 (passthrough); StartAssemblies handler allocates turn 2.
const STARTUP_BARRIER_TURN: u64 = 2;

/// First turn ID available to user-driven events after boot_silent.
/// PowerOn = 1, startup barrier = 2, first user event = 3.
const FIRST_USER_TURN: u64 = 3;

/// Boot sequence for a silent-headlamp actor: send PowerOn and manually inject
/// the BecomeOn ZoneReady reply (turn 2) that the silent headlamp will not send.
async fn boot_silent(
    controller: &VehicleController,
    rx: &mut mpsc::Receiver<PublishedTransitionRecord>,
) {
    controller.send_power_on().await.expect("power on");
    inject_zone_ready(controller, STARTUP_BARRIER_TURN, HeadlampState::Ready);
    wait_fsm_state(controller, FsmState::Idle, Duration::from_millis(500)).await;
    drain_n(rx, 2, Duration::from_secs(3)).await; // PowerOn row + AssemblyZoneReady(Headlamp) row
}
```

Replace all `power_on_to_idle(&controller).await; drain_n(&mut rx, 2, ...)` pairs with
`boot_silent(&controller, &mut rx).await`.

Also update `spawn_silent` to remove `initial_headlamp_ctx`:

```rust
async fn spawn_silent(identity: &str) -> (...) {
    let opts = VehicleControllerRuntimeOptions {
        transition_tx: Some(tx),
        test_silent_headlamp: true,
        // initial_headlamp_ctx removed: BecomeOn sets headlamp to Ready via startup barrier
        ..Default::default()
    };
    // ...
}
```

---

## RED tests (new file: `test/startup_barrier_contract.rs`)

### Test 1 — `given_power_on_when_headlamp_replies_ready_then_fsm_reaches_idle`

```
Given: Non-silent headlamp (default runtime options). Brain spawned.
When:  controller.send_power_on() sent. No manual AssembliesReady injection.
Then:  Brain sends BecomeOn to headlamp.
       Headlamp replies ZoneReady { state: Ready } automatically.
       Brain receives the Ready reply, startup barrier drains.
       AssemblyZoneReady(Headlamp) is committed; pending_assemblies becomes empty.
       FSM transitions PreparingToStart → Idle.
       wait_fsm_state(Idle, 500ms) succeeds.
       Ledger has exactly 2 rows: PowerOn hop, AssemblyZoneReady(Headlamp) hop.

RED in Phase 4: StartAssemblies is a no-op; FSM stays in PreparingToStart;
wait_fsm_state times out → test fails.
```

### Test 2 — `given_power_on_with_silent_headlamp_then_fsm_stays_in_preparing_to_start`

```
Given: Silent headlamp.
When:  controller.send_power_on() sent. No BecomeOn reply injected.
Then:  FSM stays in PreparingToStart.
       get_status() returns PreparingToStart after 200ms.

RED in Phase 4: power_on_to_idle (with shim) would bypass the barrier entirely.
This test must NOT use power_on_to_idle — only send_power_on.
After Phase 5: brain sends BecomeOn → silent → no reply → stays in PreparingToStart. ✓
```

### Test 3 — `given_power_off_from_idle_when_headlamp_replies_off_then_fsm_reaches_off`

```
Given: Non-silent headlamp. Brain in Idle (reached via power_on_to_idle).
When:  controller.send_power_off() sent. No manual AssembliesStopped injection.
Then:  Brain sends BecomeOff to headlamp.
       Headlamp replies ZoneReady { state: Off }.
       Shutdown barrier drains.
       AssemblyZoneReady(Headlamp) is committed in PreparingToStop; pending_assemblies empty.
       FSM transitions PreparingToStop → Off.
       wait_fsm_state(Off, 500ms) succeeds.

RED in Phase 4: StopAssemblies is a no-op; FSM stays in PreparingToStop → timeout.
```

### Test 4 — `given_power_off_with_silent_headlamp_then_fsm_stays_in_preparing_to_stop`

```
Given: Silent headlamp. Brain in Idle via boot_silent (which injected BecomeOn reply).
When:  controller.send_power_off() sent. No BecomeOff reply injected.
Then:  FSM stays in PreparingToStop after 200ms.

RED in Phase 4: power_off_to_off (with shim) would bypass the barrier.
This test must NOT use power_off_to_off.
```

---

## Existing tests: impact analysis

| Test file | Current use | Phase 5 impact |
|---|---|---|
| `actor_contract.rs` | `initial_headlamp_ctx=Ready` + `power_on_to_idle` | `power_on_to_idle` now sends `PowerOn` only; non-silent headlamp replies to `BecomeOn` automatically. Headlamp starts at `Ready` via `initial_headlamp_ctx` and handles `BecomeOn` as a `Ready→Ready` self-transition with reply. **No test code change needed** — verify headlamp actor handles `BecomeOn` in `Ready`. |
| `quiescence_actor_contract.rs` | `initial_headlamp_ctx=Ready` + `power_on_to_idle` | Same as above. |
| `headlamp_ack_timer_contract.rs` | `initial_headlamp_ctx=Ready` + `power_on_to_idle` | Same as above. |
| `headlamp_reply_contract.rs` | `initial_headlamp_ctx=Ready`, no `power_on_to_idle` | Verify `BecomeOn` at `Ready` works. |
| `controller_api_contract.rs` | `power_on_to_idle` + `power_off_to_off` | Uses default (non-silent) headlamp; both shims simplify; auto-flow handles transitions. **No test code change needed.** |
| `turn_barrier_contract.rs` | `spawn_silent` + `power_on_to_idle` + `drain_n(2)` | Replace with `boot_silent`; remove `initial_headlamp_ctx`; `FIRST_USER_TURN` stays 3. **Test code change required (local only).** |
| `fsm_preparation_contract.rs` | Pure FSM unit tests (no actor) | **Affected.** Tests that feed `Internal(AssembliesReady)` or `Internal(AssembliesStopped)` to reach `Idle`/`Off` must be updated to use `AssemblyZoneReady(ZoneId::Headlamp)` instead. The old arms are deleted; the old events no longer drive those transitions. |
| `headlamp_lifecycle_contract.rs` | Headlamp actor unit tests | Unaffected. |

---

## Discussion checkpoint after Phase 5

1. **Full suite green.** `cargo test -p common` — all tests pass, 0 warnings.
   Specifically: all 4 new `startup_barrier_contract` tests green; all previous tests
   remain green with no manual `AssembliesReady`/`AssembliesStopped` injections in the shims.

2. **`FIRST_USER_TURN = 3` verified.** In `turn_barrier_contract.rs`, trace turn ID
   allocations for `boot_silent`: `PowerOn` → turn 1, `StartAssemblies` loop → turn 2
   (`BecomeOn` barrier for Headlamp), user event → turn 3.

3. **`IgnitionOffReset` is now unreachable.**
   After Phase 5, `PowerOff` transitions to `PreparingToStop` (not directly to `Off`), so
   `fsm_step_lands_off` is never `true` in `on_zone_ready`.  Add `unreachable!()` to the
   `IgnitionOffReset` arm and run the full suite — it must pass cleanly.
   This is the signal that Phase 6 is safe to start.

4. **`initial_headlamp_ctx` is now a cleanup shim.**
   After Phase 5, tests that set `initial_headlamp_ctx = Some(Ready)` are bypassing the
   `BecomeOn` automatic flow. Mark it as a Phase 6 cleanup item.

5. **`Operational::AssembliesReady` / `AssembliesStopped` re-evaluation.**
   With `AssemblyZoneReady(ZoneId)` handling the FSM countdown, the `Internal` variants are
   no longer used in the transition path.  Decide in Phase 6 whether to remove them or keep
   them as observer/ledger annotations.

6. **`MANAGED_ASSEMBLIES` is the single assembly topology source.**
   Phase 8 will embed assembly IDs in `FsmState::PreparingToStart { assemblies }`. Agree on
   whether Phase 7 (Wiper as second assembly) updates this constant first before Phase 8
   refactors the FSM state.

---

## Files changed

| File | Change |
|---|---|
| `fsm/machineries.rs` | Add `FsmEvent::AssemblyZoneReady(ZoneId)` |
| `vehicle_state/mod.rs` | Add `pending_assemblies: BTreeSet<ZoneId>` to `VehicleContext` |
| `fsm/transition_map.rs` | Add `(PreparingToStart, AssemblyZoneReady)` and `(PreparingToStop, AssemblyZoneReady)` arms; initialise `pending_assemblies` in `(Off, PowerOn)` |
| `twin_runtime/zone_turn.rs` | Add `AssemblyZoneReady` arm (apply zone reply into context); rename `fsm_event_headlamp_message` → `user_event_to_headlamp_tell` (Q1 interim) |
| `twin_runtime/turn_barrier.rs` | Add `new_for_assembly_zone` constructor (Q3); delete `new_passthrough`; introduce `PassthroughBarrier` type (Q2) |
| `twin_runtime/controller/virtual_car_actor.rs` | Add `MANAGED_ASSEMBLIES`; add `self_ref` to state; wire `StartAssemblies`/`StopAssemblies` using `new_for_assembly_zone` |
| `twin_runtime/headlamp_actor.rs` | Verify/add `BecomeOn` self-transition when already in `Ready`; verify `BecomeOff` from any state |
| `twin_runtime/controller/actuation_manager.rs` | Remove `StartAssemblies \| StopAssemblies` no-op stub |
| `test/mod.rs` | Simplify `power_on_to_idle` and `power_off_to_off` (remove manual inject) |
| `test/turn_barrier_contract.rs` | Add `boot_silent`; add constants; replace `power_on_to_idle` + `drain_n` with `boot_silent`; remove `initial_headlamp_ctx` from `spawn_silent` |
| `test/startup_barrier_contract.rs` | **NEW** — 4 RED→GREEN tests |
| `test/fsm_preparation_contract.rs` | Replace `Internal(AssembliesReady/Stopped)` with `AssemblyZoneReady(Headlamp)` in all tests that drive `PreparingToStart → Idle` or `PreparingToStop → Off` |

All other files unchanged in Phase 5.
