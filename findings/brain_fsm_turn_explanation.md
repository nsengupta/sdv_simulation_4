# Brain Actor: Turn Lifecycle, Zone Coordination, and Design Constraints

This document explains why `begin_fsm_turn` exists, what guarantees the current design
depends on, where it falls short as assemblies grow, and what the correct model is.
It is the primary input for planning redesign phases and implementation.

---

## 1. The Core Problem

The Brain Actor and each Assembly Actor (e.g. Headlamp) are separate ractor actors.
Each has its own mailbox and its own mutable state.

When an `FsmEvent` arrives at the Brain, the Brain **cannot always run the FSM
immediately**. For some events, the FSM result depends on what the assembly's current
internal state has become after processing that event — and only the assembly actor knows
that. Brain must sometimes ask the assembly first, wait for the answer, and only then run
the FSM commit.

`begin_fsm_turn` is the gatekeeper that answers:
> **"Can I commit the FSM step right now, or must I ask an assembly first?"**

---

## 2. The Four Cases (Current Design)

### Case 1 — `UpdateRpm(3000)`: no zone consultation needed

```
Mailbox: [ Fsm(UpdateRpm(3000)) ]
pending_turn: None
FSM state: Idle
```

```
Fsm(UpdateRpm(3000)) arrives
→ pending_turn.is_none() → OK to start a new turn
→ begin_fsm_turn(UpdateRpm(3000))
    fsm_event_headlamp_message(UpdateRpm) → None   ← RPM has nothing to do with headlamp
    fsm_step_lands_off(...)               → false  ← RPM won't shut the car down
    → commit_resolved_turn immediately
        run_to_quiescence: zone_turn + step + ledger row
        FSM: Idle → Driving (if rpm crossed threshold)
        apply_step: twin_car updated
→ pump_fsm_backlog: nothing to drain
```

Completes synchronously inside one `handle()` call. No waiting, no `pending_turn`.

---

### Case 2 — `UpdateAmbientLux(20)`: must ask Headlamp first

Lux 20 is below threshold — headlamp should switch on. Brain cannot know the headlamp's
current internal state without asking.

```
Mailbox: [ Fsm(UpdateAmbientLux(20)) ]
pending_turn: None,  FSM state: Driving,  HeadlampActor.ctx.state: Off
```

```
Fsm(UpdateAmbientLux(20)) arrives
→ begin_fsm_turn(UpdateAmbientLux(20))
    fsm_event_headlamp_message(UpdateAmbientLux(20)) → Some(AmbientLux(20))
    → begin_headlamp_wait(turn_id=8, message=AmbientLux(20))
        tell HeadlampActor: Apply(AmbientLux(20), turn_id=8, attempt=1)
        arm TellBackTimeout timer
        pending_turn = PrimaryHeadlamp { turn_id=8, event=UpdateAmbientLux(20), ... }
→ handle() returns   ← BRAIN DOES NOT COMMIT YET
```

```
HeadlampActor.handle(Apply(AmbientLux(20)))
  on_receiving_message(AmbientLux(20))
    → state: Off → OnRequested,  outcome: SwitchOnCommand
  tell-back: brain.send(HeadlampZoneReady { turn_id=8, reply={ctx, outcomes} })
```

```
HeadlampZoneReady { turn_id=8, reply } arrives at Brain
→ on_headlamp_zone_ready(turn_id=8)
    matches PrimaryHeadlamp { turn_id=8 } → abort timer
    → commit_resolved_turn(UpdateAmbientLux(20), zone_replies=HeadlampZoneReply)
        run_to_quiescence:
          zone_turn merges REAL reply: headlamp = OnRequested, SwitchOnCommand
          step: Driving + lux 20 + headlamp OnRequested → FSM transition
          ledger row emitted
        apply_step: twin_car.headlamp.state = OnRequested
        actuation: execute SwitchOnCommand
→ pump_fsm_backlog
```

**Why wait?** `run_to_quiescence` needs the headlamp's accurate updated state for the
FSM step and detector. The HeadlampActor is the authoritative zone state — Brain's local
simulation is only a fallback.

---

### Case 3 — Concurrent event arrives while waiting: the staleness problem

```
Mailbox: [ Fsm(UpdateAmbientLux(20)), Fsm(UpdateRpm(4500)) ]
pending_turn: None
```

**Step 1:** `UpdateAmbientLux(20)` processed → `pending_turn = PrimaryHeadlamp { turn_id=8 }`

**Step 2:** `UpdateRpm(4500)` arrives while pending:

```
Fsm(UpdateRpm(4500)) arrives
→ pending_turn.is_some() ← BLOCKED
→ fsm_backlog.push_back(UpdateRpm(4500), now)
→ handle() returns immediately
```

```
State:
  pending_turn: PrimaryHeadlamp { turn_id=8 }
  fsm_backlog:  [ UpdateRpm(4500) ]
  twin_car.context().headlamp.state = Off    ← NOT YET updated
  HeadlampActor.ctx.state = OnRequested      ← already updated (async)
```

**Step 3:** `HeadlampZoneReady` arrives → commit lux → `apply_step`: headlamp = OnRequested
→ `pump_fsm_backlog` → drain UpdateRpm(4500) → commit RPM with headlamp = OnRequested.

#### What the "stale headlamp state" problem means — concretely

Suppose Brain had NO serialization and committed `UpdateRpm(4500)` immediately at Step 2.
The FSM would run with:

```
run_to_quiescence(
    initial_ctx = { headlamp: Off,    ← STALE — HeadlampActor already moved to OnRequested
                    ambient_lux: 20 }
    ingress     = UpdateRpm(4500)
)
```

After the RPM hop, the detector runs:

```
detect_internal_after_hop(exit_state=Driving, ctx={ headlamp: Off, lux: 20 })
  → headlamp is Off AND lux is below threshold AND state is Driving
  → emits Internal(LightingUnsafe)    ← fires because of stale Off
```

Second hop: `step(Driving, ctx, Internal(LightingUnsafe))` → `DrivingDangerously`.
Buzzer starts. Car is in the wrong state. Ledger says:

```
seq=5  UpdateRpm(4500)         old_ctx.headlamp=Off  → DrivingDangerously
seq=6  Internal(LightingUnsafe)                      → DrivingDangerously
```

But headlamp was ALREADY `OnRequested` when the RPM event arrived. These records are false.

**The correct ledger** (with serialization):

```
seq=5  UpdateAmbientLux(20)   headlamp: Off → OnRequested   (committed first)
seq=6  UpdateRpm(4500)        headlamp: OnRequested          (detector does NOT fire)
         → stays in Driving   ← correct
```

**This is the determinism problem.** Without serialization, the same pair of physical events
produces `DrivingDangerously` on one timing and `Driving` on another. The FSM's detector
fires based on whichever context reaches `run_to_quiescence` first. With serialization,
event arrival order defines a total order on FSM commits. Same events → same ledger → same
journey on replay.

---

### Case 4 — `PowerOff`: the two-phase problem (current design flaw)

```
Mailbox: [ Fsm(PowerOff) ]
pending_turn: None,  FSM state: Idle,  HeadlampActor.ctx.state: On
```

```
Fsm(PowerOff) arrives
→ begin_fsm_turn(PowerOff)
    fsm_event_headlamp_message(PowerOff) → None
    fsm_step_lands_off(...):
        speculatively runs zone_turn + step   ← EXECUTION #1 (speculative)
        → next_state = Off  → returns true
    → begin_headlamp_wait(message=ResetForIgnitionOff)
        pending_turn = IgnitionOffReset { turn_id=12, event=PowerOff }
→ handle() returns   ← car is NOT yet Off
```

```
HeadlampZoneReady arrives
→ on_headlamp_zone_ready (IgnitionOffReset path)
    → commit_resolved_turn(PowerOff, headlamp_reset_reply)
        run_to_quiescence: zone_turn + step → Off   ← EXECUTION #2 (real)
        apply_step: FSM = Off
```

**What is wrong:**

1. `fsm_step_lands_off` runs `zone_turn + step` speculatively to peek at the outcome — then
   `commit_resolved_turn` runs them again. Double execution of the FSM per `PowerOff`.
2. `on_headlamp_zone_ready` for `PrimaryHeadlamp` also calls `fsm_step_lands_off` — a third
   run of the same computation.
3. `PendingBrainTurn::IgnitionOffReset` exists only for this one case. Shutdown coordination
   is shoehorned into the zone-observation tell-back mechanism; they are different concerns.
4. `begin_fsm_turn` has three decision branches: (a) zone observation wait, (b) speculative
   Off-landing wait, (c) direct commit. Branch (b) is the problem.

**The redesign** adds explicit `PreparingToStop` / `PreparingToStart` FSM states. `PowerOff`
transitions to `PreparingToStop` directly (no headlamp wait). The headlamp reset is triggered
by a `CoordinationBarrier` entered when the FSM lands on `PreparingToStop`.

---

## 3. What `begin_fsm_turn` Does: Summary of Current Shape

```
Fsm(event) arrives at Brain
        │
        ▼
pending_turn.is_some?
    YES → push to fsm_backlog, return         ← enforce one-at-a-time
    NO  ↓
        ▼
begin_fsm_turn(event)
        │
        ├─ Does event carry a zone message?
        │   (AmbientLux, AckOn, AckOff, ActuationIncomplete)
        │   YES → tell Headlamp, stash pending_turn, return
        │         FSM commit deferred until HeadlampZoneReady
        │
        ├─ Will FSM land on Off?  ← SPECULATIVE: zone_turn + step run here [FLAW]
        │   YES → tell Headlamp to reset, stash pending_turn, return
        │         [this branch is eliminated in the redesign]
        │
        └─ Neither → commit_resolved_turn immediately
                     run_to_quiescence runs zone_turn + step exactly once
```

---

## 4. The Multi-Zone Coupling Problem

Replace `UpdateRpm(4500)` with `UpdateWindshieldRain(heavy)` in Case 3. Now the
second event is also zone-directed — it needs to tell the WiperActor.

**What happens today:**

```
T1  UpdateAmbientLux(20) arrives
    → Brain tells HeadlampActor: AmbientLux(20)
    → pending_turn = PrimaryHeadlamp { AmbientLux }
    → Brain returns

T2  UpdateWindshieldRain(heavy) arrives
    → pending_turn.is_some() ← BLOCKED
    → fsm_backlog.push(UpdateWindshieldRain)
    → WiperActor is NOT told about rain yet
```

**Three problems this creates:**

**A — Unnecessary latency coupling.** WiperActor is idle while HeadlampActor processes.
If HeadlampActor times out (up to `MAX_RETRIES × ZONE_TELL_BACK_WAIT`), the wiper event
is delayed by the full timeout, even though wiper and headlamp are independent.

**B — Wiper's ledger record gets a wrong `old_ctx`.** When `pump_fsm_backlog` drains
`UpdateWindshieldRain` after the lux commit, the context going into `run_to_quiescence` has
`headlamp = OnRequested` (because lux was just committed). But when the rain event arrived
at Brain's mailbox, headlamp was still `Off`. The ledger record for rain will say
`old_ctx.headlamp = OnRequested` — a world state that did not yet exist at the moment the
rain event occurred. For a replay tool this is a false picture.

**C — Scales badly.** With three assemblies (Headlamp, Wiper, Window), any slow assembly
blocks the other two. The single `pending_turn` slot is a bottleneck proportional to the
number of zone-directed events in the stream.

---

## 5. Out-of-Order Zone Replies and the Required Commit Rule

In the parallel zone tells model — where Brain tells each assembly immediately without
waiting for other assemblies — zone replies can arrive in any order.

**Scenario:**

```
T1  UpdateAmbientLux(20)      → tell HeadlampActor  → waiting HeadlampZoneReady(turn=1)
T2  UpdateWindshieldRain      → tell WiperActor      → waiting WiperZoneReady(turn=2)
T3  WiperZoneReady(turn=2)    arrives first           ← Wiper replied faster
T4  HeadlampZoneReady(turn=1) arrives second
```

At T3, WiperZoneReady is in hand but it is NOT correct to commit the rain turn yet.
The rain turn's `run_to_quiescence` takes `initial_ctx` as starting context:

```
run_to_quiescence(
    initial_ctx = ?,              ← must be the context AFTER the lux commit
    ingress     = UpdateWindshieldRain,
    zone_replies = WiperZoneReady reply
)
```

Lux (T1) has not committed yet at T3. If rain committed first, `initial_ctx.headlamp`
would be stale `Off` and the detector could fire `LightingUnsafe` — exactly as in Case 3.

**The correct behaviour at T3:** store the WiperZoneReady reply keyed to turn_id=2, do not
commit. At T4, HeadlampZoneReady arrives → commit T1 (lux) → `apply_step` →
headlamp = OnRequested → immediately use the stored WiperZoneReady to commit T2 (rain)
with `initial_ctx.headlamp = OnRequested`.

**Ledger:**

```
seq=N    UpdateAmbientLux(20)       old_ctx.headlamp=Off         → OnRequested
seq=N+1  UpdateWindshieldRain       old_ctx.headlamp=OnRequested → wiper updated
```

Both records are accurate. Zone replies arrived in reverse order; FSM commits happened
in event arrival order. This is the invariant that must always hold.

---

## 6. The Ordering Guarantees: What Holds and What Does Not

### Within a single assembly — ordering IS guaranteed

Brain sends `[Tell1, Tell2, ...]` to Assembl1 via a single channel. Same sender, same
channel, FIFO — Assembl1 receives them in exactly that send order and processes them in
that order. Its replies are sent in that same order. Brain receives them in that order.

This applies independently to every assembly. Two events can both have their zone tells
to the same assembly in flight simultaneously and it is safe:

```
Brain sends: Apply(AmbientLux(20), turn=1)   → HeadlampActor slot 1
             Apply(AmbientLux(100), turn=2)  → HeadlampActor slot 2

HeadlampActor processes (FIFO):
  slot 1: Off → OnRequested,   reply HeadlampZoneReady(turn=1)
  slot 2: OnRequested → OffRequested (on top of slot 1),  reply HeadlampZoneReady(turn=2)

Brain receives turn=1 reply before turn=2 reply (guaranteed, same channel).
```

Both commits are correct. Same-zone parallel tells are safe.

### Across assemblies — relative timing is NOT guaranteed

Brain sends tells to Assembl1 and Assembl2. These go to independent channels.
Assembl2 can receive and process its entire tell sequence and send back all replies before
Assembl1 has processed even its first tell. With more assemblies this disorder is more
pronounced.

**Critical distinction**: Brain sends tells in event arrival order (sequential actor), but
the actor runtime makes no guarantee that the message *reaches* the target mailbox
before a message sent to a different target. "Sent before" ≠ "received before" across
different actor channels.

### The single hard rule: in-order FSM commit

> **A turn N may commit only after turn N-1 has committed, regardless of when N's zone
> replies arrive.**

Zone tells may be sent immediately. Zone replies are collected as they arrive, stored
keyed by turn_id. A turn commits when all its expected zone replies are in hand AND all
preceding turns have already committed.

This is the **reorder buffer (ROB) pattern**: execute (zone tell) at any time, retire
(FSM commit) strictly in event-arrival order.

---

## 7. The Unified Turn Barrier: One Mechanism for All Assembly Coordination

### The pattern common to all zone coordination

Both startup/shutdown coordination and normal zone-tell-back waits follow the same pattern:

1. Know which assemblies to wait for (set of participants, determined at compile time or
   per event type — never discovered at runtime)
2. Send tells to those assemblies
3. Collect replies as they arrive, remove each participant from the pending set
4. When pending is empty AND all preceding turns have committed → execute completion action

For startup/shutdown the completion action is: inject `Internal(AssembliesReady/Stopped)`
into `begin_fsm_turn`. For normal zone events the completion action is:
`commit_resolved_turn` with the collected replies.

### The `TurnBarrier` abstraction

```rust
struct TurnBarrier {
    turn_id:  u64,
    event:    FsmEvent,
    now:      Instant,
    pending:  BTreeSet<ZoneId>,            // zones not yet replied
    replies:  HashMap<ZoneId, ZoneReply>,  // collected so far
}
```

The Brain actor holds:

```rust
barrier_queue: VecDeque<TurnBarrier>,
```

`VecDeque` is the right structure here. The front is always the oldest in-flight turn.
Drain always starts from the front — this enforces in-order commit without any additional
bookkeeping. Sequential scan to find a barrier by turn_id on reply arrival is O(n) where
n is bounded by (event rate × max assembly latency): in practice 1–3 entries. The scan
cost is negligible for a digital twin processing tens of events per second.

### The drain loop

```
When any ZoneReady(zone_id, turn_id, reply) arrives:
  1. Find barrier in queue with matching turn_id
  2. Move zone_id from pending → replies
  3. Walk from front of barrier_queue:
       while front.pending.is_empty():
           execute completion action for front
           pop front
           if next front exists: check it too
  4. Stop at first barrier that still has pending zones
```

This single loop handles both startup/shutdown coordination and normal zone events.
Adding Wiper to the system requires only adding `ZoneId::Wiper` to `BTreeSet` — no
structural change to the drain loop or `handle()`.

### Startup assembly list is foreknown at init time (intermediate design)

**This is the intermediate design. It will be superseded by Section 10.**

For startup/shutdown, `barrier.pending` is populated with the complete set of managed
assemblies, constructed once during `pre_start` from a hardcoded list in actor
initialisation. The actor state carries this knowledge separately from the FSM state.

For normal events, `barrier.pending` is populated from the zone-routing function based
on event type — compile-time knowledge. `UpdateAmbientLux` always touches `{Headlamp}`.
`UpdateWindshieldRain` always touches `{Wiper}`. The mapping is
`zone_message_for_event(event, state) -> Option<(ZoneId, ZoneMessage)>`.

When Section 10 lands, the startup/shutdown `barrier.pending` set is no longer
constructed from an actor-state constant. It is derived directly from
`FsmState::PreparingToStart { assemblies }` — the FSM state itself declares which
assemblies are being coordinated. The actor becomes a faithful executor of what the
FSM declares; it no longer holds a parallel copy of the coordination topology.

---

## 8. The FSM State Is the Gate — Not the Queue

### The architectural principle

> **The FSM state machine is the source of truth about what the Digital Twin is allowed
> to do in each state. Mechanisms serve it; they do not substitute for it.**

The `VecDeque<TurnBarrier>` is a tool that Brain uses to coordinate assembly tells and
replies. It is not a state machine. It must not be the decision-maker for which events
get processed.

### What this means for `PreparingToStart` and `PreparingToStop`

When the Digital Twin is in `PreparingToStart` or `PreparingToStop`:
- The FSM states define that the system is transitioning — not yet at a steady state
- The Digital Twin gives a guarantee to the Physical world: **"I will not instruct any
  assembly about physical-world observations until I have reached steady state"**
- The actuator (e.g. headlamp switch) learns to switch on only through the Digital Twin.
  If the Digital Twin is still starting up, no actuation commands should be issued.
- Therefore: no zone tells for external events during these states. Period.

**This gating behaviour (Sections 8 and 6) is stable across both the intermediate and
Section 10 designs.** What changes in Section 10 is not the gating policy but the source
of the assembly list used to populate `TurnBarrier.pending` for startup/shutdown
coordination — it moves from actor-state to FSM-state.

All incoming `Fsm(event)` messages during `PreparingToStart` / `PreparingToStop` are:
- **Recorded in the ledger** with `applied: false` (per ADR-6) — they are physical facts
  and must be auditable
- **Discarded** — not queued for later replay

The Physical world sends sensor events continuously. After `Idle` is reached, fresh events
will arrive reflecting the current physical state. Replaying stale buffered events would
give the Digital Twin a false picture of the world that existed moments ago, not now.

### Zone routing must be state-aware — not `handle()`

The current `fsm_event_headlamp_message(event)` function checks only the event type.
It does not consult the current FSM state. This forces an explicit
`if current_state is PreparingToStart/Stop` guard somewhere in the call path.

That guard does not belong in `handle()`. `handle()` should dispatch and nothing else.
The guard belongs in the zone routing function:

```rust
fn zone_message_for_event(event: &FsmEvent, state: &FsmState)
    -> Option<(ZoneId, ZoneMessage)>
{
    match state {
        FsmState::PreparingToStart | FsmState::PreparingToStop => None,
        _ => fsm_event_zone_message(event),  // existing per-event-type logic
    }
}
```

`begin_fsm_turn` calls this state-aware function. In `PreparingToStart`, it returns
`None` for every external event → no zone tell → `commit_resolved_turn` runs directly
→ the FSM transition table returns "stay in `PreparingToStart`" → ledger records the hop
as `applied: false`. `handle()` remains state-blind.

### `handle()` is stable regardless of FSM state

```
Fsm(event) arrives
    → begin_fsm_turn(event)
        zone_message_for_event(event, current_state) decides whether to tell a zone
        FSM transition table decides next state and whether applied: true/false
    → pump barriers

ZoneReady { zone_id, turn_id, reply } arrives
    → find barrier, store reply, drain from front
    → pump barriers

ZoneTellBackTimeout { zone_id, turn_id, attempt } arrives
    → retry or forge synthetic reply, drain from front
    → pump barriers

GetStatus(reply)
    → reply_get_status
```

Four arms. `handle()` never grows.

---

## 9. `handle()` Must Not Grow with Assembly Count

### The current problem

Each assembly currently contributes named message variants to the Brain's vocabulary:

```rust
HeadlampZoneReady { turn_id, reply }   // Headlamp-specific
HeadlampZoneSpontaneous { ... }        // Headlamp-specific
TellBackTimeout { turn_id, attempt }   // implicitly Headlamp-specific
```

With Wiper and Window added this becomes eight or more arms, growing O(3N) with N
assemblies. This is wrong.

### The fix: generic zone envelope

The assembly identity becomes data inside the message, not the message name:

```rust
enum DigitalTwinCarVocabulary {
    Fsm(FsmEvent),
    ZoneReady {
        zone_id:      ZoneId,           // which assembly replied
        turn_id:      u64,
        tell_attempt: u32,
        reply:        ZoneReply,        // enum: Headlamp(HeadlampZoneReply) | Wiper(...) | ...
    },
    ZoneTellBackTimeout {
        zone_id:      ZoneId,
        turn_id:      u64,
        tell_attempt: u32,
    },
    ZoneSpontaneous {
        zone_id:      ZoneId,
        event:        ZoneSpontaneousEvent,
    },
    GetStatus(RpcReplyPort<CarSnapshot>),
}
```

`handle()` has **four arms regardless of how many assemblies exist**.

Adding Wiper or Window requires:
- Adding `ZoneId::Wiper` to the `ZoneId` enum
- Adding `Wiper(WiperZoneReply)` to the `ZoneReply` enum
- Adding Wiper to the zone routing function
- **Zero new arms in `handle()`**

All assembly-specific dispatch happens inside `on_zone_ready`, `on_zone_timeout`, and
`on_zone_spontaneous` — one match per function, which is appropriate.

---

## 10. Target: Semantically Richer `FsmState` (Supersedes Sections 7 and 8 for coordination topology)

### What changes and what stays

Sections 7 and 8 are the **intermediate design** — correct and sufficient for Phases 1–7.
Section 10 is the **target design** for coordination topology ownership. It does not
replace the `VecDeque<TurnBarrier>` drain loop or the `zone_message_for_event` routing.
It changes only **where the assembly list for startup/shutdown coordination is declared**.

What stays unchanged from Sections 7 and 8:
- `VecDeque<TurnBarrier>` as the drain mechanism — unchanged
- `zone_message_for_event(event, state)` for per-event zone routing — unchanged
- FSM state gating external events during `PreparingToStart`/`PreparingToStop` — unchanged
- `handle()` with four arms — unchanged

What is reimplemented in Section 10:
- The source of `TurnBarrier.pending` for startup/shutdown coordination moves from a
  hardcoded actor-state constant to the FSM state variant itself
- `apply_committed_quiescence` no longer has a hardcoded `{ pending: {Headlamp} }` —
  it reads `assemblies` from `FsmState::PreparingToStart { assemblies }` instead
- The actor becomes a faithful executor of what the FSM declares; the coordination
  topology is owned entirely by the FSM

### The richer `FsmState`

The current `FsmState` (in `crates/common/src/fsm/machineries.rs`) carries no information
beyond the state name. `PreparingToStart` and `PreparingToStop` are opaque variants.

The target design embeds the assembly IDs inside the transitioning states at compile time:

```rust
// Illustrative — exact syntax to be decided
FsmState::PreparingToStart { assemblies: &'static [AssemblyId] }
FsmState::PreparingToStop  { assemblies: &'static [AssemblyId] }
```

This makes the FSM the single queryable source of truth about the Digital Twin's
composite state at any point in time:

- "Which assemblies are being coordinated right now?" — answered by reading the FSM state,
  not by inspecting a separate field in actor state
- "What does this Digital Twin manage?" — answered by the FSM transition table, not by
  a list maintained in parallel in the actor
- The coordination topology is pure, compile-time, testable in isolation from the actor

### Constraints

The FSM must remain a pure function. `assemblies` must be a compile-time constant —
a `&'static` slice or a const-generic parameter. No heap allocation, no runtime
discovery. A Digital Twin managing two assemblies is statically distinct from one
managing one assembly. This distinction is enforced by the type system, not by runtime
checks.

### When to implement

After Phase 7 validates that the `VecDeque<TurnBarrier>` coordination pattern works
correctly with two assemblies (Headlamp + Wiper), Section 10 refactors the startup/
shutdown barrier population to derive from FSM state. All tests from Phases 1–7 must
remain green. The refactor is internal to the actor — no change to the FSM's external
contracts (`FsmEvent`, `DomainAction`, `output()`, `transition()` signatures).

---

## 11. Design Summary: Current vs Target

| Concern | Current | Target |
|---|---|---|
| Zone routing | Event-type only (`fsm_event_headlamp_message`) | State-aware (`zone_message_for_event(event, state)`) |
| `begin_fsm_turn` branches | 3: zone wait / speculative Off check / direct commit | 2: zone wait / direct commit |
| Speculative FSM execution | Yes — `fsm_step_lands_off` runs `zone_turn+step` twice | Deleted entirely |
| `PendingBrainTurn` variants | 2 (`PrimaryHeadlamp` + `IgnitionOffReset`) | 1 flat struct → then `TurnBarrier` |
| PowerOff coordination | Shoehorned into zone tell-back as `IgnitionOffReset` | Explicit `CoordinationBarrier` via `PreparingToStop` FSM state |
| Events during startup/shutdown | Buffered in `fsm_backlog`, replayed after | Ledger `applied:false` + discarded |
| `handle()` arm count | O(3N) with N assemblies | 4, fixed regardless of N |
| Cross-assembly parallelism | None — one global `pending_turn` | `VecDeque<TurnBarrier>` + ROB commit |
| FSM state richness | Opaque names | Future: assembly IDs embedded in transitioning states |
| Ledger `old_ctx` accuracy | Wrong for backlogged events | Correct — each event committed with all-predecessor context |

---

## 12. Phased Implementation Order

Each phase is independently compilable and testable. Phases 1–6 target correctness and
simplicity for the single-assembly (Headlamp) case. Phase 7 validates the multi-assembly
architecture. Phase 8 (Section 10) completes the design by moving coordination topology
ownership into the FSM state itself.

| Phase | Scope | Key Invariant Gained |
|---|---|---|
| **1** | FSM: add `PreparingToStart`, `PreparingToStop`; add `Internal(AssembliesReady)`, `Internal(AssembliesStopped)` to vocabulary; update `transition_map` | FSM transition table is the complete mode story including startup/shutdown |
| **2** | `HeadlampMessage`: add `BecomeOn`, `BecomeOff`; `HeadlampActor`: handle and ack both; `ZoneId` enum introduced | Assembly startup/shutdown vocabulary exists |
| **3** | `DigitalTwinCarVocabulary`: replace `HeadlampZoneReady`/`TellBackTimeout`/`HeadlampZoneSpontaneous` with generic `ZoneReady`/`ZoneTellBackTimeout`/`ZoneSpontaneous` carrying `ZoneId` | `handle()` has 4 arms; adding new assemblies requires zero new arms |
| **4** | Brain actor state: add `barrier_queue: VecDeque<TurnBarrier>` replacing `pending_turn: Option<PendingBrainTurn>`; implement unified drain loop | One coordination mechanism for all assembly tells |
| **5** | `apply_committed_quiescence`: on entering `PreparingToStart`/`PreparingToStop`, push `TurnBarrier` with `BecomeOn`/`BecomeOff` and fire tells; assembly list is a hardcoded actor-state constant (intermediate) | Startup/shutdown coordination flows through barrier queue |
| **6** | Zone routing: replace `fsm_event_headlamp_message(event)` with `zone_message_for_event(event, state)`; normal events in `PreparingToStart`/`Stop` get `applied:false` + discarded; `fsm_step_lands_off` deleted; `IgnitionOffReset` variant deleted | FSM state gates zone routing; no speculative execution; `handle()` is state-blind |
| **7** | Add Wiper as second assembly: `ZoneId::Wiper`, `WiperZoneReply`, wiper zone routing, wiper in startup/shutdown barrier | Multi-assembly architecture validated end-to-end; intermediate coordination topology confirmed working |
| **8** *(Section 10)* | Embed `&'static [AssemblyId]` in `FsmState::PreparingToStart` and `FsmState::PreparingToStop`; `apply_committed_quiescence` derives `TurnBarrier.pending` from FSM state instead of actor-state constant; all Phase 1–7 tests stay green | FSM is the single queryable source of Digital Twin composite state; actor holds no parallel copy of coordination topology; Sections 7 and 8 intermediate design superseded for startup/shutdown |

Phases 1–3: structural and vocabulary changes only — no behaviour change.
Phase 4: replaces the core coordination mechanism (`pending_turn` → `VecDeque<TurnBarrier>`).
Phases 5–6: wire the new mechanism to the FSM and actuation paths; eliminate speculative execution.
Phase 7: validates multi-assembly architecture end-to-end with Headlamp + Wiper.
Phase 8 (Section 10): moves coordination topology ownership from actor state into FSM state.
  The `VecDeque<TurnBarrier>` drain loop and `zone_message_for_event` routing are unchanged.
  Only the source of `TurnBarrier.pending` for startup/shutdown changes: actor constant → FSM state.

---

## 13. Open Questions Resolved — Phases 4–6 Implementation Inputs

Four design gaps must be closed before coding Phases 4–6. Each is resolved below.

---

### Gap 1 — How does `TurnBarrier` relate to the existing `TellBackWait` retry logic?

**Current code:** `TellBackWait` (`zone_tell_back.rs`) carries `turn_id`, `tell_attempt`,
`retries_remaining` for one in-flight zone wait. `TellBackTimeoutOutcome` decides retry vs
synthetic reply on exhaustion. This works for a single zone per turn.

**Resolution:** `TellBackWait` is promoted to per-zone state inside `TurnBarrier`.

```rust
struct TurnBarrier {
    turn_id:      u64,
    event:        FsmEvent,
    now:          Instant,
    pending:      BTreeSet<ZoneId>,
    zone_waits:   HashMap<ZoneId, TellBackWait>,   // one retry counter per zone
    zone_timers:  HashMap<ZoneId, TellBackTimer>,  // one deadline handle per zone
    replies:      HashMap<ZoneId, ZoneReply>,
}
```

`TellBackWait` itself is **unchanged** — it just becomes one value per zone in the map.

When `ZoneTellBackTimeout { zone_id, turn_id, tell_attempt }` arrives:

1. Locate the barrier with matching `turn_id`
2. Call `on_tell_back_timeout(zone_ctx, zone_waits[zone_id])` as before
3. On `Retry(next)`: re-tell that zone, re-arm its timer; other zones unaffected
4. On `Exhausted(synthetic)`: move `zone_id` from `pending` into `replies` with the
   synthetic reply; run the drain loop

`on_tell_back_timeout` and its synthetic reply helper are reused without modification.
The only change: they are called per-zone, not per-turn.

---

### Gap 2 — What triggers the `BecomeOn`/`BecomeOff` tells from `apply_committed_quiescence`?

**Current code:** `apply_committed_quiescence` reacts to `DomainAction` variants emitted by
the FSM. `DomainAction::EnterMode` already exists but its value (`mode`) is computed and
then dropped (`let _ = mode;`) — it was never wired to assemble startup/shutdown tells.

**Resolution:** Add two `DomainAction` variants to `machineries.rs`:

```rust
DomainAction::StartAssemblies,  // emitted by FSM on → PreparingToStart
DomainAction::StopAssemblies,   // emitted by FSM on → PreparingToStop
```

The FSM transition table emits these when the relevant transitions fire.
`apply_committed_quiescence` matches them:

```rust
DomainAction::StartAssemblies => {
    // Phase 5 intermediate: read managed assembly list from actor-state const
    // push TurnBarrier, tell each assembly BecomeOn, arm per-zone timers
}
DomainAction::StopAssemblies => {
    // push TurnBarrier, tell each assembly BecomeOff, arm per-zone timers
}
```

Why not detect the transition by comparing old vs new FSM state? Because
`apply_committed_quiescence` is an executor of FSM intent — it must not inspect
state-transition pairs. The FSM declares intent via actions; the actor executes them.
This is consistent with how `EnterMode` already works and upholds the FSM-as-gate principle.

`DomainAction::EnterMode` is **deleted** in Phase 5. `ActorModeHintFromDomain` and
`ActorMode` in `virtual_car_actor.rs` are also deleted. `StartAssemblies`/`StopAssemblies`
fully subsume the transitioning-state signalling with explicit semantics.

---

### Gap 3 — What happens to the `IgnitionOffReset` block inside `apply_external_hop`?

**Current code** (`twin_turn.rs` lines 184–193):

```rust
if matches!(result.next_state, FsmState::Off) {
    let zone_reply = zone_replies.headlamp.ignition_off_reset.clone()
        .unwrap_or_else(|| {
            result.modified_ctx.headlamp.on_receiving_message(
                HeadlampMessage::ResetForIgnitionOff, now)
        });
    result.modified_ctx.headlamp = zone_reply.ctx;
    headlamp_outcomes.extend(zone_reply.outcomes);
}
```

This fires when any external hop lands on `FsmState::Off`. It was needed because the old
design allowed a direct `PowerOff → Off` transition with an in-flight `IgnitionOffReset` tell.

**Resolution:** The block is **deleted entirely** in Phase 6. Here is why it is safe:

- In the new design, `PowerOff` transitions to `PreparingToStop`, never directly to `Off`
- `Off` is entered only via `Internal(AssembliesStopped)`
- `Internal` events use `apply_internal_hop`, which calls `step` only — it never reaches
  `apply_external_hop`
- Therefore the `matches!(result.next_state, FsmState::Off)` branch inside
  `apply_external_hop` is dead code once Phases 1 and 5 land

Cascading deletions in Phase 6:

| Item deleted | File |
|---|---|
| `apply_external_hop` ignition-off block | `twin_turn.rs` |
| `HeadlampReplies.ignition_off_reset` field | `zone_replies.rs` |
| `ZoneReplies::with_headlamp(ingress, ignition_off_reset)` second argument | `zone_replies.rs`, call sites |
| `PendingBrainTurn::IgnitionOffReset` variant | `virtual_car_actor.rs` |
| `fsm_step_lands_off` function | `twin_turn.rs` |
| `DomainAction::EnterMode` + `ActorMode` + `ActorModeHintFromDomain` | `machineries.rs`, `virtual_car_actor.rs` |

All of these are mechanically removed together in Phase 6 after Phase 5 has established the
`PreparingToStop → BecomeOff tells → Internal(AssembliesStopped) → Off` path.

---

### Gap 4 — How does `ZoneReplies` evolve across phases?

**Current shape** (`zone_replies.rs`):

```rust
pub struct HeadlampReplies {
    pub ingress:            Option<HeadlampZoneReply>,
    pub ignition_off_reset: Option<HeadlampZoneReply>,  // ← deleted in Phase 6
}
pub struct ZoneReplies {
    pub headlamp: HeadlampReplies,
}
```

**Phase 4–6 shape** (after deleting `ignition_off_reset`; headlamp-specific struct retained):

```rust
pub struct ZoneReplies {
    pub headlamp: Option<HeadlampZoneReply>,  // ingress tell-back; None = local fallback
}
impl ZoneReplies {
    pub fn simulate_locally() -> Self { Self { headlamp: None } }
}
```

`zone_turn` reads `zone_replies.headlamp` directly. No other change to `zone_turn` logic.
Pure-test path is unchanged: `simulate_locally()` returns `headlamp: None`.

**Phase 7 shape** (multi-assembly):

```rust
pub struct ZoneReplies {
    pub replies: HashMap<ZoneId, ZoneReply>,
}
impl ZoneReplies {
    pub fn simulate_locally() -> Self { Self { replies: HashMap::new() } }
    pub fn get(&self, id: ZoneId) -> Option<&ZoneReply> { self.replies.get(&id) }
}
```

`zone_turn` accesses `zone_replies.get(ZoneId::Headlamp)` and downcasts
`ZoneReply::Headlamp`. `ZoneId` is introduced in Phase 2 (in `vehicle_state` module or a
new `twin_runtime/zone.rs`).

The transition is two discrete steps: Phase 6 simplifies (removes `ignition_off_reset`),
Phase 7 generalises (map-based). No single big-bang refactor.
