# SDV Simulation 4 — Architecture and Design

This document is the consolidated design reference for `sdv_simulation_4`.
It captures the final architecture (after Phases 1–9 of the Brain FSM Redesign),
the reasoning behind key decisions, and the open gaps carried forward to
`sdv_simulation_5`.

Source material: `brain_fsm_redesign_plan.md`, per-phase implementation notes
(Phases 2–9), `findings/`, and `analysis_4_response.md`.

---

## 1. Why Actor + FSM?

### Actor model rationale

An actor is single-threaded and event-driven by design.  Each actor has its own
mailbox; the framework (ractor) processes exactly one message at a time, awaiting
the handler future to completion before dispatching the next message.  This gives
a **sequential, deterministic execution order** without explicit locking.

The Virtual Car Brain is a natural actor:

- It collects a large number of messages from multiple assemblies and zones of the
  physical car.
- Each message is an event in the Brain's vocabulary; processing is: one event at a
  time, all externally delivered.
- The FSM it owns must run in the same thread as `handle()` — always synchronously —
  so the FSM step and the actor's state update are atomic from the actor's perspective.

### FSM rationale

The FSM is a **pure function**: `(State, Context, Event, Instant) → (NextState, Actions)`.

- No I/O, no heap discovery, no runtime branching on external sources.
- Testable at the speed of a function call — no mailbox round-trip needed.
- The transition table (`transition_map.rs`) is the **single authoritative source**
  of the Digital Twin's mode story, including startup and shutdown.
- Detectors (e.g., `LightingUnsafe`) are also pure functions that propose additional
  internal events; they do not modify state directly.

### Actor + FSM rule

> `transition()` and `output()` are always called in strict order, exactly once
> per event, from within the actor's `handle()` thread.  They are never interleaved.

---

## 2. Library Pyramid (L0–L6)

The `common` crate follows an acyclic layer pyramid.  The critical invariant:
**L2 (`fsm`) must not import L3 or above** — the FSM has no actor, no I/O, and
no runtime dependency.

| Layer | Module(s) | Role |
|---|---|---|
| **L0** | `vehicle_physics` | Constants and pure kinematics |
| **L1** | `vehicle_state`, `domain_types`, `signals` | Zone contexts, wire vocabulary, VSS signal IDs |
| **L2** | `fsm` | Pure decision core — `step`, `transition_map` |
| **L3** | `digital_twin`, `published` | Twin capsule, serializable mirror types |
| **L4** | `transition_sink`, `diagnostic`, `twin_runtime` | Actor runtime, sinks, detectors |
| **L5** | `facade` | Public surface — the only module gateway binaries may import |
| **L6** | `gateway`, `emulator`, `front_headlamp_actuator` | Application binaries |

The gateway imports **only `common::facade`** — a single doorway enforced by a CI script.

---

## 3. FSM States

```
Off  ──PowerOn──►  PreparingToStart({Headlamp, Wiper})
                          │ each AssemblyZoneReady(id) removes id
                          ▼
                   PreparingToStart({Wiper})
                          │ AssemblyZoneReady(Wiper) → set empty
                          ▼
                        Idle  ──UpdateRpm > threshold──►  Driving
                          │                                    │
                    PowerOff                             UpdateRpm < threshold
                          │                                    │
                          ▼                                    ▼
              PreparingToStop({Headlamp, Wiper})            Idle
                          │ symmetric to start
                          ▼
                         Off

  Driving  ──Internal(LightingUnsafe)──►  DrivingDangerously
  DrivingDangerously  ──headlamp On or lux high or stationary──►  Driving or Idle
```

`ExtremeOperationWarning` and `ActuationIncomplete` are additional transient states
for unsafe operational conditions.

### Key state invariants

- External events arriving in `PreparingToStart` / `PreparingToStop` are **recorded
  in the ledger** with `applied: false` and then discarded.  They are never replayed.
  (Fresh events arrive after `Idle` is reached.)
- The FSM never transitions directly `Off ↔ Idle` in the final design.  Every
  power cycle goes through a preparing state.

---

## 4. Final FSM Type: `FsmState::PreparingToStart(BTreeSet<AssemblyId>)` (Phase 9)

### The problem that was solved

After Phase 8 introduced `PreparingToStart { assemblies: &'static [AssemblyId] }`,
a **temporal mismatch** appeared in `transition_map.rs`.

- `transition()` needed to decide "transition to Idle or self-loop?" based on how
  many assemblies had acknowledged.
- But the countdown lived in `VehicleContext::remaining_assemblies`, which was only
  mutated *later* in `step.rs`.
- `transition()` had to do peek-ahead arithmetic on a future value it did not own.

Additionally, every self-loop reset the `assemblies` field back to `ALL_ASSEMBLIES`,
discarding progress.

### The solution (Phase 9 final design)

The countdown was moved **into** the FSM state variant itself:

```rust
// Phase 9 final — FSM state is the sole countdown authority
FsmState::PreparingToStart(BTreeSet<AssemblyId>)
FsmState::PreparingToStop(BTreeSet<AssemblyId>)
```

State evolution on each acknowledgement:

```
PreparingToStart({Headlamp, Wiper}) + AssemblyZoneReady(Headlamp)
    → PreparingToStart({Wiper})         // BTreeSet filtered; Headlamp removed

PreparingToStart({Wiper}) + AssemblyZoneReady(Wiper)
    → Idle                              // BTreeSet empty
```

`VehicleContext::remaining_assemblies` was deleted entirely.
`transition()` is now a fully self-contained pure function that reads and produces
the `BTreeSet` from the state variant directly.

### Why `BTreeSet` and not a static slice

| Option | Verdict |
|---|---|
| `&'static [AssemblyId]` | Can only hold `ALL_ASSEMBLIES`; cannot represent a shrinking subset |
| Fixed array + length counter | Off-by-one risk, order-dependent `PartialEq`; fine for no-alloc |
| `BTreeSet<AssemblyId>` | Shrinks naturally; `is_empty()` terminates the countdown cleanly |

For a bare-metal ECU (no allocator), replace with `arrayvec::ArrayVec<AssemblyId, MAX_ASSEMBLIES>` — a two-line change in `machineries.rs`; no logic changes elsewhere.

### `output()` intra-mode guard

`PreparingToStart({H,W}) ≠ PreparingToStart({W})` in Rust, so a naive
`(old, new) if old != new => PublishStateSync` would fire on every intermediate
acknowledgement.  Explicit guards suppress this:

```rust
(PreparingToStart(_), PreparingToStart(_)) => vec![],
(PreparingToStop(_), PreparingToStop(_))   => vec![],
```

---

## 5. Assembly Actors and the Zone Coordination Problem

### Why zone consultation is needed

Brain and each Assembly Actor (e.g., HeadlampActor, WiperActor) are separate ractor
actors with independent mailboxes.  For some FSM events, the correct FSM step depends
on what the assembly's internal state has become — but only the assembly actor knows that.

`begin_fsm_turn` answers:
> **"Can I commit the FSM step right now, or must I ask an assembly first?"**

### The four cases

**Case 1 — No zone consultation needed** (`UpdateRpm`):
The event has no zone message.  `run_to_quiescence` runs immediately with a simulated
zone reply.  Completes in a single `handle()` call.

**Case 2 — Zone consultation needed** (`UpdateAmbientLux`):
Brain tells HeadlampActor (fire-and-forget), arms a tell-back timeout timer, stashes a
`TurnBarrier` in the queue, and returns from `handle()`.  FSM commit is deferred until
`HeadlampZoneReady` arrives.

**Case 3 — Concurrent event while zone consultation is in flight**:
The second event is queued in the `VecDeque<TurnBarrier>` as a passthrough barrier.
It commits only after the first turn commits, preserving `old_ctx` accuracy in the ledger.

**Case 4 — `PowerOff` / `PowerOn`** (startup/shutdown coordination):
`PowerOn` → `PreparingToStart(ALL_ASSEMBLIES)`.  The FSM's `DomainAction::StartAssemblies`
triggers `apply_committed_quiescence` to push a `TurnBarrier` with `pending = ALL_ASSEMBLIES`
and tell each assembly `BecomeOn`.  The drain loop waits for all assemblies before
advancing to `Idle`.  Symmetric for `PowerOff`.

The old design (before Phase 4) had an `IgnitionOffReset` special case and
`fsm_step_lands_off()` which ran `zone_turn + step` twice (speculative + real).
Both were deleted.

---

## 6. The `VecDeque<TurnBarrier>` — Reorder Buffer (ROB) Pattern

### Structure

```rust
struct TurnBarrier {
    turn_id:      u64,
    event:        FsmEvent,
    now:          Instant,
    pending:      BTreeSet<ZoneId>,            // zones not yet replied
    zone_waits:   HashMap<ZoneId, TellBackWait>, // per-zone retry counters
    zone_timers:  HashMap<ZoneId, TellBackTimer>,
    replies:      HashMap<ZoneId, ZoneReply>,   // collected so far
}
```

The Brain actor holds:

```rust
barrier_queue: VecDeque<TurnBarrier>,
```

### Drain loop

```
When any ZoneReady(zone_id, turn_id, reply) arrives:
  1. Locate barrier with matching turn_id
  2. Move zone_id from pending → replies
  3. Walk from front:
       while front.pending.is_empty():
           execute completion action (commit_resolved_turn)
           pop_front()
  4. Stop at first barrier that still has pending zones
```

The front of the queue is always the oldest in-flight turn.  This enforces **in-order
commit** (ROB principle) without additional bookkeeping: zone replies are collected as
they arrive, but FSM commits happen strictly in event-arrival order.

### Why ordering matters

Suppose `UpdateAmbientLux(20)` (T1) and `UpdateWindshieldRain` (T2) are both in flight.
If rain (T2) commits before lux (T1), `run_to_quiescence` for rain runs with
`initial_ctx.headlamp = Off` — stale, because lux already moved headlamp to
`OnRequested` in the assembly actor.  The `LightingUnsafe` detector could fire falsely,
producing `DrivingDangerously` when the headlamp was already lit.

With the ROB pattern the same pair of events always produces the same ledger,
regardless of which zone replies arrive first.

### Per-zone retry logic

`TellBackWait` (one per zone, inside `TurnBarrier`) tracks retry count.
`ZONE_TELL_BACK_MAX_RETRIES = 2` means three total attempts.  On exhaustion, a
synthetic reply is committed — the turn proceeds without a real zone acknowledgement.
The zone's internal state becomes `ActuationIncomplete(direction)`.

---

## 7. State-Aware Zone Routing (`zone_message_for_event`)

```rust
fn zone_message_for_event(event: &FsmEvent, state: &FsmState)
    -> Option<(ZoneId, ZoneMessage)>
{
    match state {
        PreparingToStart(_) | PreparingToStop(_) => None,  // all external events discarded
        _ => per_event_type_routing(event),
    }
}
```

During preparing states, every external event returns `None` → no zone tell →
`commit_resolved_turn` runs immediately → FSM transition table stays in
`PreparingToStart`/`Stop` → ledger records `applied: false`.

`handle()` does not consult FSM state.  It dispatches to `begin_fsm_turn`, which
calls `zone_message_for_event`.  State-awareness lives in the routing function,
not the dispatcher.

---

## 8. `handle()` Has Exactly Four Arms

```rust
match message {
    Fsm(event)                                 => begin_fsm_turn(event),
    ZoneReady { zone_id, turn_id, reply }      => on_zone_ready(zone_id, turn_id, reply),
    ZoneTellBackTimeout { zone_id, turn_id }   => on_zone_timeout(zone_id, turn_id),
    GetStatus(reply_port)                      => reply_get_status(reply_port),
}
```

This structure is stable regardless of the number of assemblies.  Adding Wiper or
Window requires:
- `ZoneId::Wiper` added to the enum
- `Wiper(WiperZoneReply)` added to `ZoneReply`
- Wiper registered in `zone_message_for_event`
- Zero new arms in `handle()`

---

## 9. Quiescence and the Detector Catalog

A single external event can trigger multiple FSM hops before the system is stable.

```
external event → hop 1 → hop 2 → ... → stable cut → apply_step → actuation
                  └── one ledger row per hop ──┘
```

`run_to_quiescence` runs the detector catalog after each hop.  If a detector returns
`Some(FsmEvent::Internal(...))`, that event is enqueued as the next hop.  The loop
terminates when no detector fires (stable cut) or `MAX_QUIESCENCE_HOPS` is reached.

Detectors are pure functions in `twin_runtime/detectors/`.  They do not modify state.
They propose; the transition table decides.

### `LightingUnsafe` detector

Fires when: `Driving` state AND `ambient_lux < LUX_ON_THRESHOLD` AND headlamp state
is `Off` or `Ready` (not `OnRequested` or `On`).

Emits: `FsmEvent::Internal(Operational::LightingUnsafe)` → next hop transitions to
`DrivingDangerously` + `StartBuzzer` action.

**Known gap (simulation-5):** After `ActuationIncomplete(Off)` the headlamp state is
neither `Off` nor `Ready`, so the detector does not re-fire even when the lamp is
physically dark.  The system enters a quiescent state until a new lux event arrives.
See `TODO-simulation-5.md` §4c.

---

## 10. Async / Single-Thread Guarantee

`ractor` processes messages one at a time; the `handle` future is awaited to
completion before the next message is dispatched.

Every `.await` inside the Brain's `handle()` chain falls into one of three categories:

| Category | Example | Risk |
|---|---|---|
| Intra-actor structural (safe) | `begin_fsm_turn`, `commit_resolved_turn` chain | None — no external I/O |
| **Actuation channel** `.await` | `actuation_manager.execute(tx.send(...).await)` | Actor-stall if channel is full (backpressure) |
| `send_after` timer | Tell-back timeout delivery | None — ractor timer wheel delivers via mailbox |

The actuation `.await` does not reorder messages (Rust's `&mut` on stack prevents
concurrent access), but it **can stall the actor** if the CAN egress channel is full.
The existing `TODO(actuation-child-actor)` comment acknowledges this; the correct fix
is to offload actuation into `HeadlampActor`'s own thread.  Tracked in `TODO-simulation-5.md` §2.

---

## 11. Key Data Types (final state after Phase 9)

### `FsmState`

```rust
pub enum FsmState {
    Off,
    PreparingToStart(BTreeSet<AssemblyId>),   // shrinking set of pending assemblies
    Idle,
    Driving,
    DrivingDangerously,
    ExtremeOperationWarning,
    PreparingToStop(BTreeSet<AssemblyId>),    // symmetric to PreparingToStart
}
```

### `AssemblyId` and `ALL_ASSEMBLIES`

```rust
pub enum AssemblyId { Headlamp, Wiper }

pub(crate) const ALL_ASSEMBLIES: &[AssemblyId] = &[AssemblyId::Headlamp, AssemblyId::Wiper];
```

`ALL_ASSEMBLIES` is the single declaration of the Digital Twin's assembly topology.
`FsmState::PreparingToStart` seeds its `BTreeSet` from this constant on every
`Off + PowerOn` transition.

### `VehicleContext`

Carries only assembly-domain state (sensors, actuators).  Zero FSM lifecycle
bookkeeping.  The `remaining_assemblies` field (Phase 8 intermediate design) was
deleted in Phase 9.

### `DomainAction`

```rust
DomainAction::StartAssemblies(Vec<AssemblyId>)  // on PreparingToStart entry
DomainAction::StopAssemblies(Vec<AssemblyId>)   // on PreparingToStop entry
DomainAction::RequestFrontHeadlampOn
DomainAction::RequestFrontHeadlampOff
DomainAction::StartBuzzer
DomainAction::PublishStateSync
// ...
```

### `HeadlampState`

```rust
pub enum HeadlampState {
    Off,              // assembly not started
    Ready,            // assembly active; lamp dark; awaiting lux
    OnRequested,      // ON CMD sent; awaiting ACK
    On,               // ACK received; lamp confirmed lit
    OffRequested,     // OFF CMD sent; awaiting ACK
    ActuationIncomplete(FrontHeadlampSwitchDirection),  // max retries exhausted
}
```

`AckOff` lands in `Ready` (not `Off`) because the assembly is still active.
`ActuationIncomplete(On)` recovers to `Ready` (assembly active, lamp dark).

---

## 12. Headlamp Assembly Actor — Known Test Gaps

`headlamp_lifecycle_contract.rs` tests `HeadlampContext::on_receiving_message()` in
isolation (no actor, no Brain).  Coverage gaps:

| Missing test | Scenario |
|---|---|
| `actuation_incomplete_off_*` | What state after NACK/timeout on an OFF command |
| `nack_for_off_while_on_requested` | ON in flight; off-direction NACK arrives |
| `off_cmd_happy_path` | `OffRequested → AckOff → Ready` |

No test spawns `HeadlampActor` in isolation (without `VirtualCarActor`).

The **hang scenario** (ON → NACK-for-OFF → retry → drop → system quiets) is not
covered by any test at any level.  See `TODO-simulation-5.md` §4 for the full
breakdown.

---

## 13. What Was Deliberately Not Done in simulation-4

These items are documented in `TODO-simulation-5.md` and `brain_fsm_redesign_impl_Phase_10.md`
(now subsumed here):

| Item | Why deferred |
|---|---|
| **CAN emulation** for `PowerOn` / `PowerOff` (CAN ID `0x100`) | Architecture exists; wiring not implemented |
| **Non-blocking actuation** (offload `execute()` to child actor) | Correct fix known; risk of regression without actor-level tests |
| **Code commenting pass** over `begin_fsm_turn` call tree | Clean but undocumented; not blocking |
| **Actor-level fuzz/steady-state tests** | FSM-level `proptest` exists; actor-level stress tests missing |
| **HeadlampActor isolation tests** + `ActuationIncomplete(Off)` coverage | Identified from observed hang; not yet written |
| **`ArrayVec` migration** (no-alloc embedded target) | Two-line change; waiting for embedded target requirement |

---

## 14. Design Decisions Resolved During simulation-4

| Decision | Chosen direction |
|---|---|
| `PreparingToStart` payload type | `BTreeSet<AssemblyId>` (shrinking) over `&'static [AssemblyId]` (static) |
| Countdown ownership | Inside `FsmState` (deleted `VehicleContext::remaining_assemblies`) |
| `handle()` growth with assemblies | Generic zone envelope (`ZoneId` as data, not message name) |
| Event ordering across zones | ROB pattern (`VecDeque<TurnBarrier>`) — fire tells immediately, commit in arrival order |
| External events during startup/shutdown | `applied: false` ledger record + discard (never replay stale sensor data) |
| Speculative FSM execution for PowerOff | Deleted (`fsm_step_lands_off`, `IgnitionOffReset`) — explicit `PreparingToStop` state instead |
| Actuation blocking | Known risk; deferred to simulation-5 |
| `DomainAction` as actor intent signal | FSM emits `StartAssemblies`/`StopAssemblies`; actor executes — FSM does not inspect state-transition pairs |
