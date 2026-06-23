+++
title = "Prototyping a Software Defined Vehicle - Stage IV"
date = 2026-06-23
draft = true

[taxonomies]
tags = ["sdv-prototype"]
+++

### Preface

[Stage III](@/blog/Prototype-Software-Defined-Vehicle-3/index.md) made the Digital Twin
actor-based: the Headlamp zone moved into its own child actor (`HeadlampActor`), the Brain
became a quiescence engine running multiple internal FSM hops per external event, and the
library pyramid reached an acyclic L0–L6 layering.

**Stage IV is about coordination correctness at scale.**

The single-assembly demo worked.  But adding a second assembly (Wiper) exposed fundamental
flaws in how the Brain coordinated zone consultations: events were serialised through a
single `pending_turn` slot, out-of-order zone replies could produce false `LightingUnsafe`
alerts, and startup/shutdown coordination was shoehorned into the zone tell-back mechanism
as a special case.

Stage IV replaces all of that with three coherent changes:

1. **A `VecDeque<TurnBarrier>` reorder buffer** — zone tells fire immediately; FSM commits
   happen in event-arrival order regardless of when zone replies arrive.
2. **Explicit `PreparingToStart` / `PreparingToStop` FSM states** — the FSM owns the
   startup/shutdown lifecycle; the actor is a faithful executor of what the FSM declares.
3. **The FSM state is the countdown** — `PreparingToStart(BTreeSet<AssemblyId>)` carries
   the shrinking set of pending assemblies inside the state variant itself, eliminating a
   parallel bookkeeping field in `VehicleContext`.

Same three processes, same CAN traffic.  Everything that changed is inside the `common`
crate — the coordination fabric.

---

### What was wrong with the Stage III design

#### Problem 1 — A single `pending_turn` slot blocks all other events

```
T1  UpdateAmbientLux(20)   → tell HeadlampActor  → pending_turn = Some(HeadlampTurn)
T2  UpdateWindshieldRain   → pending_turn is Some → pushed to fsm_backlog; WiperActor not told
T3  HeadlampZoneReady      → commit lux turn; drain backlog
T4  UpdateWindshieldRain   drains from backlog → tell WiperActor (delayed by T1 latency)
```

Three problems:

- **Latency coupling.** WiperActor is idle while HeadlampActor processes.  If Headlamp
  times out (`MAX_RETRIES × ZONE_TELL_BACK_WAIT`), Wiper's event is delayed by the full
  timeout.
- **Wrong `old_ctx` in the ledger.**  When rain drains at T4, `initial_ctx.headlamp`
  is `OnRequested` — reflecting the world *after* lux committed, not at the moment
  the rain event arrived.  The ledger record is false for a replay tool.
- **O(N) growth.**  With three assemblies, any slow one blocks the other two.

#### Problem 2 — `PowerOff` ran the FSM twice

The old `begin_fsm_turn` had three branches: zone consultation wait, speculative
Off-landing check (`fsm_step_lands_off`), and direct commit.  Branch two ran
`zone_turn + step` speculatively to peek at whether PowerOff would land on `Off` — then
`commit_resolved_turn` ran them again.  The FSM executed twice per `PowerOff` event.
A special `PendingBrainTurn::IgnitionOffReset` variant existed solely for this case.

#### Problem 3 — Startup/shutdown had no FSM representation

The car's boot sequence (`Off → Idle`) and shutdown (`Idle → Off`) required both
`HeadlampActor` and `WiperActor` to confirm readiness or cessation before the FSM
could advance.  But the FSM had no states for this — `Off + PowerOn → Idle` was a
single step.  The coordination was actor-side, ad hoc, and not captured in the FSM
transition table.

---

### Design decision 1 — Explicit startup/shutdown states in the FSM

Two new FSM states were added:

```
Off  ──PowerOn──►  PreparingToStart
PreparingToStart  ──(all assemblies ready)──►  Idle
Idle  ──PowerOff──►  PreparingToStop
PreparingToStop  ──(all assemblies stopped)──►  Off
```

During `PreparingToStart` and `PreparingToStop` the FSM gating rule is:

> All incoming external events are **recorded in the ledger** with `applied: false`
> and then **discarded**.  They are not buffered for replay.

Why discard rather than buffer?  The physical world sends sensor events continuously.
After `Idle` is reached, fresh events arrive reflecting the *current* physical state.
Replaying stale buffered events would give the Digital Twin a false picture of a
world that existed moments ago.  `applied: false` in the ledger makes the suppression
auditable without introducing reordering risk.

The FSM transition table is now the **complete mode story** — including the car's
lifecycle — not just its operational states.

---

### Design decision 2 — `VecDeque<TurnBarrier>`: the reorder buffer

The `VecDeque<TurnBarrier>` replaces the single `pending_turn: Option<PendingBrainTurn>`.

```rust
struct TurnBarrier {
    turn_id:      u64,
    event:        FsmEvent,
    now:          Instant,
    pending:      BTreeSet<ZoneId>,             // zones not yet replied
    zone_waits:   HashMap<ZoneId, TellBackWait>, // per-zone retry counters
    zone_timers:  HashMap<ZoneId, TellBackTimer>,
    replies:      HashMap<ZoneId, ZoneReply>,    // collected so far
}
```

The drain loop:

```
When ZoneReady(zone_id, turn_id, reply) arrives:
  1. Locate barrier with matching turn_id
  2. Remove zone_id from pending; store reply
  3. From the front: while front.pending.is_empty() → commit and pop
  4. Stop at first barrier that still has pending zones
```

This is the **reorder buffer (ROB) pattern**: zone tells fire immediately (issue-in-order),
zone replies are collected as they arrive, FSM commits happen strictly in event-arrival
order (retire-in-order).

Under this model the earlier scenario becomes:

```
T1  UpdateAmbientLux(20)   → push barrier(turn=1, pending={Headlamp}); tell HeadlampActor
T2  UpdateWindshieldRain   → push barrier(turn=2, pending={Wiper}); tell WiperActor
T3  WiperZoneReady(turn=2) → store reply for turn=2; front (turn=1) still pending; no commit
T4  HeadlampZoneReady(turn=1) → front ready; commit turn=1 (lux); then commit turn=2 (rain)
    Ledger turn=2: initial_ctx.headlamp = OnRequested  ← accurate
```

Both commits are accurate; neither is delayed by the other assembly's latency.

#### Per-zone retry and synthetic replies

`TellBackWait` (unchanged from Stage III) is promoted to per-zone state inside
`TurnBarrier`.  `ZONE_TELL_BACK_MAX_RETRIES = 2` means three total attempts per zone.
On exhaustion, a synthetic reply commits the turn — the zone enters
`ActuationIncomplete(direction)` and the system moves on.

---

### Design decision 3 — `handle()` has exactly four arms, forever

Stage III had named message variants per assembly (`HeadlampZoneReady`,
`TellBackTimeout`, `HeadlampZoneSpontaneous`).  With two assemblies that would grow to
six or more arms, O(3N) with N assemblies.

Stage IV collapses this to a generic zone envelope:

```rust
enum DigitalTwinCarVocabulary {
    Fsm(FsmEvent),
    ZoneReady       { zone_id: ZoneId, turn_id: u64, reply: ZoneReply },
    ZoneTellBackTimeout { zone_id: ZoneId, turn_id: u64, tell_attempt: u32 },
    GetStatus(RpcReplyPort<CarSnapshot>),
}
```

`handle()` has four arms.  Adding a third assembly (Window) requires adding
`ZoneId::Window`, `Window(WindowZoneReply)` to `ZoneReply`, and updating
`zone_message_for_event` — **zero new arms in `handle()`**.

---

### Design decision 4 — State-aware zone routing

Stage III's `fsm_event_headlamp_message(event)` checked only the event type, not the
FSM state.  An explicit guard inside `begin_fsm_turn` prevented zone tells during
startup/shutdown.

Stage IV moves the guard into the routing function itself:

```rust
fn zone_message_for_event(event: &FsmEvent, state: &FsmState)
    -> Option<(ZoneId, ZoneMessage)>
{
    match state {
        PreparingToStart(_) | PreparingToStop(_) => None,  // gated
        _ => per_event_type_routing(event),
    }
}
```

`handle()` remains state-blind.  The FSM state gates zone routing; the dispatcher
does not need to know.

---

### Design decision 5 — FSM state is the countdown (Phase 9 refinement)

An intermediate design (Phase 8) embedded `&'static [AssemblyId]` in the
`PreparingToStart` struct variant and kept a parallel `VehicleContext::remaining_assemblies`
field for the live countdown.  A code review during Phase 8 revealed a **temporal mismatch**:
`transition()` had to peek ahead into a future value of `remaining_assemblies` that was only
mutated *later* in `step.rs`.

The fix collapsed the two representations into one.  The countdown moved into the FSM state:

```rust
// Before (Phase 8 intermediate)
PreparingToStart { assemblies: &'static [AssemblyId] }  // always ALL_ASSEMBLIES, never shrinks
VehicleContext::remaining_assemblies: BTreeSet<AssemblyId>  // the live countdown

// After (Phase 9 final)
PreparingToStart(BTreeSet<AssemblyId>)  // shrinks on every AssemblyZoneReady
// VehicleContext::remaining_assemblies — DELETED
```

State evolution:

```
PreparingToStart({Headlamp, Wiper}) + AssemblyZoneReady(Headlamp) → PreparingToStart({Wiper})
PreparingToStart({Wiper}) + AssemblyZoneReady(Wiper) → Idle
```

`transition()` reads the `BTreeSet` directly from the state, produces a filtered copy,
and calls `is_empty()` to decide between self-loop and `Idle`.  It makes zero reads
from `VehicleContext` for countdown purposes — a fully self-contained pure function.

`VehicleContext` now carries only assembly-domain state (sensors, actuators): no FSM
lifecycle bookkeeping.

---

### The "hang" scenario — a design gap, not a deadlock

During integration testing the following was observed:

```
1. HeadlampActor is On (ACK confirmed).
2. FSM transitions to Driving, then powertrain slows → Idle.
3. Brain emits RequestFrontHeadlampOff.
4. HeadlampActor sends OFF CMD to actuator.
5. Actuator responds NACK.  HeadlampActor retries.  Actuator drops response.  Max retries exhausted.
6. HeadlampActor enters ActuationIncomplete(Off).  Brain commits synthetic reply.
7. No more RequestFrontHeadlampOff is ever emitted.  System is quiet.
```

This is not a deadlock.  It is a **quiescent but inconsistent state**.

After step 6, `HeadlampState::ActuationIncomplete(Off)` is neither `Off` nor `Ready`,
so the `LightingUnsafe` detector does not re-fire (it only fires for `Off` or `Ready`
when driving in darkness).  No new lux event arrives.  No new command is issued.

The physical lamp may still be on.  The Digital Twin has lost track of it.

**The correct fix** is a periodic reconciliation loop: if `ActuationIncomplete(direction)`
persists for longer than N seconds, re-issue the command.  This is a simulation-5 item —
see `TODO-simulation-5.md` §4c.  The scenario is also unverified by any test — §4a and
§4b cover the missing coverage.

---

### The assembly actor test gap

`headlamp_lifecycle_contract.rs` tests `HeadlampContext::on_receiving_message()` — the
pure state machine function — in isolation.  It does not test `HeadlampActor` as a ractor
actor.

`headlamp_ack_timer_contract.rs` tests the ACK timer behaviour but via the full
`VehicleController` stack — the Brain is always involved.

There are no tests that:
- Spawn `HeadlampActor` in isolation and send messages to it directly.
- Cover the `ActuationIncomplete(Off)` recovery path at the zone level.
- Assert what happens after max retries are exhausted for an OFF command.

These are simulation-5 items.

---

### What comes next (simulation-5)

| Item | Description |
|---|---|
| **CAN emulation** | CAN ID `0x100` → `FsmEvent::PowerOn` / `PowerOff`; a `can_emulator` module or actor |
| **Non-blocking actuation** | Move `actuation_manager.execute()` into `HeadlampActor`'s own thread so `VirtualCarActor` never blocks on CAN egress backpressure |
| **HeadlampActor isolation tests** | Spawn actor in isolation; cover `ActuationIncomplete(Off)`, OFF NACK, and the hang scenario |
| **Actor-level fuzz tests** | Spawn `VirtualCarActor`, fire random ractor messages, assert `barrier_queue` drains and final state is valid |
| **Code commenting pass** | Doc-comments on `begin_fsm_turn`, `zone_message_for_event`, drain loop, `apply_committed_quiescence` |
| **ArrayVec migration (optional)** | Replace `BTreeSet<AssemblyId>` with `arrayvec::ArrayVec` for no-alloc embedded targets — two-line change |

---

### Summary

Stage IV makes the Digital Twin **coordination-correct at multi-assembly scale**.

The `VecDeque<TurnBarrier>` reorder buffer eliminates the single-slot serialisation
bottleneck: zone tells fire immediately; FSM commits happen in event-arrival order.
`PreparingToStart(BTreeSet<AssemblyId>)` makes the FSM the single authority on both
the assembly topology and the live countdown — `VehicleContext` carries no lifecycle
bookkeeping.  `handle()` has four arms and will never grow regardless of how many
assemblies join.

From the user's perspective: same demo, same three processes, now with a Wiper as
second assembly.  From the developer's perspective: the coordination architecture is
correct, the FSM is the complete mode story, and the groundwork for CAN-emulated
ignition and non-blocking actuation is laid.

---

**Where the code is**

- Repository: [**`sdv_simulation_4`**](https://github.com/nsengupta/sdv_simulation_4) — builds on [**`sdv_simulation_3`**](https://github.com/nsengupta/sdv_simulation_3).
- Design reference: `DESIGN.md` in the project root — architecture, type definitions, key decisions.
- Open work: `TODO-simulation-5.md` in the project root.

---

**Series:** Prototyping a Software Defined Vehicle

**← Previous** [Stage III](@/blog/Prototype-Software-Defined-Vehicle-3/index.md)

All posts in this series: [sdv-prototype](/tags/sdv-prototype/)
