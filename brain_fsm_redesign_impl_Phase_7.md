# Brain FSM Redesign — Phase 7 Implementation Plan
## Wiper as Second Assembly

**Status:** Design locked — discussion resolved (2026-06-22).  Ready for implementation.  
**Depends on:** Phase 6 complete (`zone_message_for_event` for headlamp only; `IgnitionOffReset` / `EnterMode` deleted; all 139 tests green).  
**Next phase:** Phase 8 — FSM state embeds assembly IDs; `MANAGED_ASSEMBLIES` deleted.

---

## Phases 1–6 achieved so far

| Phase | Tag | Core deliverable |
|---|---|---|
| 1 | `phase-1-fsm-vocabulary` | `PreparingToStart`/`PreparingToStop`; `StartAssemblies`/`StopAssemblies`; 8 RED→GREEN tests |
| 2 | `phase-2-headlamp-zone-alphabet` | `HeadlampMessage::BecomeOn/BecomeOff`; `ZoneId::Headlamp`; 9 RED→GREEN tests |
| 3 | `phase-3-generic-zone-envelope` | `ZoneReply`, `ZoneSpontaneousEvent`, generic `ZoneReady`/`ZoneSpontaneous`/`ZoneTellBackTimeout` |
| 4 | `phase-4-reorder-buffer-barrier-queue` | `TurnBarrier`; `VecDeque<BarrierEntry>` ROB; HOB drain loop; `alloc_turn_id` |
| 5 | `phase-5-assembly-barriers` | `StartAssemblies`→`BecomeOn` barrier; `AssemblyZoneReady` FSM event; `pending_assemblies` countdown |
| 6 | `phase-6-state-aware-routing` | `zone_message_for_event`; `IgnitionOffReset` deleted; speculative execution gone; 6 RED→GREEN tests |

---

## What Phase 7 delivers

Phase 7 validates that the intermediate single-assembly architecture generalises cleanly to
**N assemblies** by adding the Wiper as the second managed zone.  It requires no new
coordination protocol — the same `TurnBarrier` / drain-loop / `MANAGED_ASSEMBLIES` machinery
used for the headlamp handles the wiper unchanged, once the type-level barriers are lifted.

Concretely, Phase 7 does four things:

1. **New Wiper assembly** — `WiperMessage`, `WiperContext`, `WiperState`, `WiperZoneReply`,
   `WiperActor`, `WiperActorMsg` parallel the headlamp structure; wired into
   `VirtualCarRuntimeState`.
2. **`ZoneMessage` routing envelope** — a new `ZoneMessage` enum (living in
   `digital_twin/mod.rs`, the symmetric counterpart of `ZoneReply`) lifts the return type of
   `zone_message_for_event` from `Option<HeadlampMessage>` to `Option<(ZoneId, ZoneMessage)>`.
   `TurnBarrier::zone_messages` changes from `HashMap<ZoneId, HeadlampMessage>` to
   `HashMap<ZoneId, ZoneMessage>`.
3. **`ZoneReplies` map migration** — the struct migrates from a field-per-zone layout
   (`headlamp: HeadlampReplies`) to a homogeneous map (`replies: HashMap<ZoneId, ZoneReply>`),
   eliminating the `HeadlampReplies` wrapper.
4. **`MANAGED_ASSEMBLIES` extended** — `&[ZoneId::Headlamp, ZoneId::Wiper]`; the
   startup/shutdown loop already iterates over it; no structural change to the loop is needed.

Gateway events: `RainsStarted` and `RainsStopped` are the binary triggers.  They are binary
facts — no intensity — which keeps the wiper model simple in this phase.

---

## Resolved design decisions

### D1 — `ZoneMessage` lives in `digital_twin/mod.rs`

**Decision:** `ZoneMessage` is placed in `crates/common/src/digital_twin/mod.rs`.

**Rationale:** `digital_twin/mod.rs` is already the zone-communication vocabulary layer.
It defines what the brain *receives* from zones (`ZoneReply`, `ZoneSpontaneousEvent`,
`DigitalTwinCarVocabulary`).  `ZoneMessage` is the symmetric send-side counterpart — what
the brain *tells* zones.  Placing both in the same module makes the communication contract
self-contained and keeps `digital_twin` at a lower level than `twin_runtime` (which depends
on `digital_twin` for `ZoneReply` already).

Dependency graph after placement:
```
vehicle_state   ──────────────────────────────► L1 zone alphabets (HeadlampMessage, WiperMessage, …)
      │                                              ▲
      └──────────────────► digital_twin  ────────────┘
                           (ZoneMessage, ZoneReply, ZoneSpontaneous…)
                                 ▲
                           twin_runtime
                           (zone_turn, turn_barrier, virtual_car_actor, …)
```

`ZoneMessage` is `pub(crate)` — it is not part of the external crate API.

```rust
// crates/common/src/digital_twin/mod.rs  (addition)

/// Brain-to-zone routing envelope — symmetric counterpart of [`ZoneReply`].
///
/// `zone_message_for_event` produces this; `TurnBarrier` stores it for retry;
/// `tell_zone` dispatches it to the correct actor.  `pub(crate)` only.
#[derive(Debug, Clone)]
pub(crate) enum ZoneMessage {
    Headlamp(HeadlampMessage),
    Wiper(WiperMessage),
}
```

`ZoneMessage` derives `Clone` (not `Copy`); the `zone_message` getter on `TurnBarrier`
returns `Option<ZoneMessage>` (cloned value).  See D6.

---

### D2 — Binary wiper events: `RainsStarted` / `RainsStopped`

**Decision:** There is no `RainIntensity` in Phase 7.  The gateway-to-controller interface
exposes two binary facts:

| Gateway event | FsmEvent | ZoneMessage produced |
|---|---|---|
| Rain started | `FsmEvent::RainsStarted` | `ZoneMessage::Wiper(WiperMessage::Start)` |
| Rain stopped | `FsmEvent::RainsStopped` | `ZoneMessage::Wiper(WiperMessage::Stop)` |

**`WiperMessage`:**
```rust
// crates/common/src/vehicle_state/wiper.rs  (new file)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WiperMessage {
    BecomeOn,
    BecomeOff,
    Start,   // from RainsStarted gateway event
    Stop,    // from RainsStopped gateway event
}
```

**`WiperState`:**
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WiperState {
    Off,      // assembly not started
    Ready,    // assembly active; no rain
    Running,  // wiping; rain is active
}
```

`Running` carries no embedded intensity payload.  If a future phase needs intensity, it
extends `Running(RainIntensity)` — but that is deferred.

**`WiperOutcome`:**
```rust
#[derive(Debug, Clone, PartialEq)]
pub enum WiperOutcome {
    StartWiping,
    StopWiping,
}
```

---

### D3 — Wiper state machine: all transitions direct, no intermediate states

| Message | From state | → To state | Outcomes |
|---|---|---|---|
| `BecomeOn` | `Off` | `Ready` | — |
| `BecomeOn` | `Ready`/`Running` | unchanged (no-op) | — |
| `BecomeOff` | any | `Off` | — |
| `Start` | `Ready` | `Running` | `[StartWiping]` |
| `Start` | `Running` | `Running` (no-op) | — |
| `Start` | `Off` | `Off` (no-op) | — |
| `Stop` | `Running` | `Ready` | `[StopWiping]` |
| `Stop` | `Ready`/`Off` | unchanged (no-op) | — |

`BecomeOff` transitions directly to `Off` regardless of the current operational state.
No `OffRequested`/`OnRequested` intermediate states.  The wiper has no actuation-ack
protocol in Phase 7.

`WiperContext::on_receiving_message(msg: WiperMessage) -> WiperZoneReply` encodes the
table above (same L1 pattern as `HeadlampContext::on_receiving_message`).

---

### D4 — `ZoneTurnResult` pre-emptively generic

**Decision:** Remove per-zone fields from `ZoneTurnResult`; replace with a homogeneous
`Vec<ZoneOutcome>`.  Remove the `headlamp_before` field — callers capture the before state
from the input `ctx` directly.

**New `ZoneOutcome` enum** (lives in `zone_turn.rs`, the module that produces it):
```rust
// crates/common/src/twin_runtime/zone_turn.rs
#[derive(Debug, Clone, PartialEq)]
pub enum ZoneOutcome {
    Headlamp(HeadlampOutcome),
    Wiper(WiperOutcome),
}
```

**New `ZoneTurnResult`:**
```rust
#[derive(Debug)]
pub struct ZoneTurnResult {
    pub ctx:      VehicleContext,
    pub outcomes: Vec<ZoneOutcome>,
    // headlamp_before REMOVED — caller snapshots ctx.headlamp.state before calling zone_turn
}
```

**Impact on `apply_external_hop` (twin_turn.rs):**
```rust
// Before:
let zone_result = zone_turn(ctx, event, state, now, zone_replies);
let headlamp_before = zone_result.headlamp_before;
// ...
front_headlamp_confirmed_direction(headlamp_before, headlamp_after)

// After:
let headlamp_before = ctx.headlamp.state;     // snapshot BEFORE zone_turn
let zone_result = zone_turn(ctx, event, state, now, zone_replies);
// headlamp_before is already captured; headlamp_after = zone_result.ctx.headlamp.state
front_headlamp_confirmed_direction(headlamp_before, headlamp_after)
```

`ZoneOutcome` filtering (for actuation): callers iterate `zone_result.outcomes` and
match on each variant.

---

### D5 — `ZoneReplies` map migration

**Before (Phase 6):**
```rust
pub struct HeadlampReplies { pub ingress: Option<HeadlampZoneReply> }
pub struct ZoneReplies { pub headlamp: HeadlampReplies }
```

**After (Phase 7):**
```rust
pub struct ZoneReplies { pub replies: HashMap<ZoneId, ZoneReply> }
impl ZoneReplies {
    pub fn simulate_locally() -> Self { Self { replies: HashMap::new() } }
    pub fn with_reply(zone_id: ZoneId, reply: ZoneReply) -> Self {
        let mut r = HashMap::new();
        r.insert(zone_id, reply);
        Self { replies: r }
    }
    pub fn get(&self, id: &ZoneId) -> Option<&ZoneReply> { self.replies.get(id) }
}
```

`HeadlampReplies` struct is deleted.  `with_headlamp_ingress` is deleted (replaced by
`with_reply`).

**Call-site translation table:**

| Old | New |
|---|---|
| `zone_replies.headlamp.ingress.as_ref()` | `zone_replies.get(&ZoneId::Headlamp).and_then(ZoneReply::as_headlamp)` |
| `ZoneReplies::with_headlamp_ingress(r)` | `ZoneReplies::with_reply(ZoneId::Headlamp, ZoneReply::Headlamp(r))` |
| `zone_replies.headlamp.ingress` in `AssemblyZoneReady` arm | `zone_replies.get(&zone_id).and_then(ZoneReply::as_headlamp_or_wiper)` |

`ZoneReply` gains two accessor helpers (added to `digital_twin/mod.rs`):
```rust
impl ZoneReply {
    pub fn as_headlamp(&self) -> Option<&HeadlampZoneReply> {
        if let ZoneReply::Headlamp(r) = self { Some(r) } else { None }
    }
    pub fn as_wiper(&self) -> Option<&WiperZoneReply> {
        if let ZoneReply::Wiper(r) = self { Some(r) } else { None }
    }
}
```

---

### D6 — `TurnBarrier::zone_messages` type; `Clone` return

`zone_messages: HashMap<ZoneId, HeadlampMessage>` → `HashMap<ZoneId, ZoneMessage>`.

Methods that change:

| Method | Before | After |
|---|---|---|
| `add_pending_zone` | `message: HeadlampMessage` | `message: ZoneMessage` |
| `zone_message` | `→ Option<HeadlampMessage>` (Copy) | `→ Option<ZoneMessage>` (Clone) |
| `new_for_assembly_zone` | `message: HeadlampMessage` | `message: ZoneMessage` |

`ZoneMessage: Clone` (not `Copy`).  `zone_message` returns a cloned value so callers
(retry path in `on_zone_timeout`) do not need lifetime annotations:
```rust
pub fn zone_message(&self, zone_id: ZoneId) -> Option<ZoneMessage> {
    self.zone_messages.get(&zone_id).cloned()
}
```

**`into_resolved_turn` becomes a trivial map move** (D6 + D5 combined):
```rust
pub fn into_resolved_turn(self) -> ResolvedTurn {
    ResolvedTurn {
        ingress: self.event,
        now:     self.now,
        zone_replies: ZoneReplies { replies: self.zone_replies },
    }
}
```
The `zone_replies: HashMap<ZoneId, ZoneReply>` field in `TurnBarrier` and
`ZoneReplies::replies: HashMap<ZoneId, ZoneReply>` share the same type — the map is moved
directly, zero re-allocation.

---

### D7 — `ZoneId::Wiper`; `ZoneReply::Wiper`

```rust
// crates/common/src/fsm/machineries.rs
pub enum ZoneId { Headlamp, Wiper }

// crates/common/src/digital_twin/mod.rs
pub enum ZoneReply {
    Headlamp(crate::vehicle_state::HeadlampZoneReply),
    Wiper(crate::vehicle_state::WiperZoneReply),
}
```

`ZoneSpontaneousEvent` remains headlamp-only (Wiper has no spontaneous events in Phase 7).

---

### D8 — `VehicleContext` addition

```rust
pub struct VehicleContext {
    pub headlamp:   HeadlampContext,
    pub wiper:      WiperContext,      // NEW
    pub visibility: VisibilityContext,
    pub powertrain: PowertrainContext,
    pub gps:        GpsContext,
}
```

`WiperContext` defaults to `WiperState::Off`.  All existing struct-literal fixtures that
spread with `..Default::default()` continue to compile without change; any that use full
positional struct syntax will need a `wiper: Default::default()` field added.

---

### D9 — `WiperActor` structure (no ACK timer)

Mirrors `HeadlampActor` but without the ACK timer — all wiper transitions are immediate.

```
WiperActorVocabulary { message: WiperMessage, now: Instant, turn_id: u64, tell_attempt: u32, brain }
WiperActorMsg::Apply(WiperActorVocabulary)     ← no AckWaitElapsed variant
WiperActorState { ctx: WiperContext, silent: bool, brain: Option<ActorRef<...>> }
```

On every `Apply`:
```
let zone_reply = state.ctx.on_receiving_message(message);
state.ctx = zone_reply.ctx.clone();
brain.send_message(ZoneReady { zone_id: ZoneId::Wiper, turn_id, tell_attempt,
                               reply: ZoneReply::Wiper(zone_reply) });
```

`tell_wiper_zone` is added with the same fire-and-forget signature as `tell_headlamp_zone`.
`WiperActorState::new(ctx, silent)` follows the same `silent` pattern used by headlamp
contract tests.  `post_stop` is a no-op (no timers to abort).

No `ZoneSpontaneous` path in Phase 7.

---

### D10 — `zone_turn` wiper extension

`user_event_to_zone_tell` (renamed from `user_event_to_headlamp_tell`) returns
`Option<(ZoneId, ZoneMessage)>`:

```rust
fn user_event_to_zone_tell(event: &FsmEvent) -> Option<(ZoneId, ZoneMessage)> {
    match event {
        FsmEvent::UpdateAmbientLux(lux) =>
            Some((ZoneId::Headlamp, ZoneMessage::Headlamp(HeadlampMessage::AmbientLux(*lux)))),
        FsmEvent::FrontHeadlampOnAck =>
            Some((ZoneId::Headlamp, ZoneMessage::Headlamp(HeadlampMessage::AckOn))),
        FsmEvent::FrontHeadlampOffAck =>
            Some((ZoneId::Headlamp, ZoneMessage::Headlamp(HeadlampMessage::AckOff))),
        FsmEvent::FrontHeadlampActuationIncomplete { direction, cause } =>
            Some((ZoneId::Headlamp, ZoneMessage::Headlamp(HeadlampMessage::ActuationIncomplete {
                direction: *direction, cause: *cause,
            }))),
        FsmEvent::RainsStarted =>
            Some((ZoneId::Wiper, ZoneMessage::Wiper(WiperMessage::Start))),
        FsmEvent::RainsStopped =>
            Some((ZoneId::Wiper, ZoneMessage::Wiper(WiperMessage::Stop))),
        FsmEvent::UpdateRpm(_)
        | FsmEvent::PowerOn
        | FsmEvent::PowerOff
        | FsmEvent::TimerTick
        | FsmEvent::Internal(_)
        | FsmEvent::AssemblyZoneReady(_) => None,
    }
}
```

`zone_turn` function body extension (wiper arms):

```rust
FsmEvent::RainsStarted | FsmEvent::RainsStopped => {
    let ingress = zone_replies.get(&ZoneId::Wiper).and_then(ZoneReply::as_wiper);
    let msg = if matches!(event, FsmEvent::RainsStarted) {
        WiperMessage::Start
    } else {
        WiperMessage::Stop
    };
    let zone_reply = merge_wiper_for_message(ctx, msg, now, ingress);
    next.wiper = zone_reply.ctx;
    outcomes.extend(zone_reply.outcomes.into_iter().map(ZoneOutcome::Wiper));
}
FsmEvent::AssemblyZoneReady(zone_id) => {
    match zone_id {
        ZoneId::Headlamp => { /* existing headlamp arm */ }
        ZoneId::Wiper => {
            if let Some(reply) = zone_replies.get(&ZoneId::Wiper).and_then(ZoneReply::as_wiper) {
                next.wiper = reply.ctx.clone();
                outcomes.extend(reply.outcomes.iter().cloned().map(ZoneOutcome::Wiper));
            }
        }
    }
}
```

---

### D11 — `apply_committed_quiescence` zone-dispatch helpers

```rust
fn become_on_message_for(zone_id: ZoneId) -> ZoneMessage {
    match zone_id {
        ZoneId::Headlamp => ZoneMessage::Headlamp(HeadlampMessage::BecomeOn),
        ZoneId::Wiper    => ZoneMessage::Wiper(WiperMessage::BecomeOn),
    }
}

fn become_off_message_for(zone_id: ZoneId) -> ZoneMessage {
    match zone_id {
        ZoneId::Headlamp => ZoneMessage::Headlamp(HeadlampMessage::BecomeOff),
        ZoneId::Wiper    => ZoneMessage::Wiper(WiperMessage::BecomeOff),
    }
}

fn tell_zone(
    runtime_state: &VirtualCarRuntimeState,
    brain: &ActorRef<DigitalTwinCarVocabulary>,
    zone_id: ZoneId,
    message: &ZoneMessage,
    turn_id: u64,
    tell_attempt: u32,
    now: Instant,
) -> Result<(), ActorProcessingErr> {
    match message {
        ZoneMessage::Headlamp(m) =>
            tell_headlamp_zone(&runtime_state.headlamp_actor, brain, turn_id, tell_attempt, *m, now),
        ZoneMessage::Wiper(m) =>
            tell_wiper_zone(&runtime_state.wiper_actor, brain, turn_id, tell_attempt, *m, now),
    }
}

fn synthetic_reply_for(ctx: &VehicleContext, zone_id: ZoneId) -> ZoneReply {
    match zone_id {
        ZoneId::Headlamp => ZoneReply::Headlamp(HeadlampZoneReply {
            ctx: ctx.headlamp.clone(), outcomes: vec![]
        }),
        ZoneId::Wiper => ZoneReply::Wiper(WiperZoneReply {
            ctx: ctx.wiper.clone(), outcomes: vec![]
        }),
    }
}
```

`begin_fsm_turn` after D1 and the above helpers:
```rust
if let Some((zone_id, message)) =
    zone_message_for_event(&event, runtime_state.twin_car.current_state())
{
    let wait = TellBackWait::new(turn_id);
    tell_zone(runtime_state, brain, zone_id, &message, turn_id, 0, now)?;
    let timer = Self::arm_tell_back_timer(brain, turn_id, 0);
    let mut barrier = TurnBarrier::new(turn_id, event, now);
    barrier.add_pending_zone(zone_id, message, wait, timer);
    runtime_state.barrier_queue.push_back(BarrierEntry::Waiting(barrier));
    return Ok(());
}
```
The explicit `crate::fsm::ZoneId::Headlamp` hardcode from Phase 6 is gone.

`on_zone_ready` loses the headlamp-specific unpack:
```rust
async fn on_zone_ready(
    runtime_state: &mut VirtualCarRuntimeState,
    zone_id: ZoneId,
    turn_id: u64,
    tell_attempt: u32,
    reply: ZoneReply,           // stored as-is; no zone-specific unwrap
) -> Result<(), ActorProcessingErr> {
    let Some(entry) = runtime_state.barrier_queue.iter_mut()
        .find(|e| e.turn_id() == turn_id) else { return Ok(()); };
    let Some(barrier) = entry.as_waiting_mut() else { return Ok(()); };
    if !barrier.tell_attempt_matches(zone_id, tell_attempt) { return Ok(()); }
    barrier.act_on_zone_reply(zone_id, reply);
    Ok(())
}
```

`on_zone_timeout` synthetic reply uses `synthetic_reply_for` instead of
headlamp-specific construction.

---

### D12 — Gateway tests: both twinlets complete the lifecycle dance

**Decision:** Both twinlets (headlamp and wiper) must be observed in the expected state
before a gateway test submits user events.  Tests that currently call
`wait_headlamp_state(&controller, HeadlampState::Ready, ...)` after `send_power_on()` must
also call `wait_wiper_state(&controller, WiperState::Ready, ...)`.  The order is:

```rust
controller.send_power_on().await;
// FSM enters PreparingToStart; two assembly barriers pushed: [Headlamp, Wiper]
wait_headlamp_state(&controller, HeadlampState::Ready, Duration::from_millis(500)).await;
wait_wiper_state(&controller, WiperState::Ready, Duration::from_millis(500)).await;
// FSM is now in Idle; both twinlets are Ready; safe to send user events
```

Analogously for shutdown:
```rust
controller.send_power_off().await;
wait_headlamp_state(&controller, HeadlampState::Off, Duration::from_millis(500)).await;
wait_wiper_state(&controller, WiperState::Off, Duration::from_millis(500)).await;
```

A `wait_wiper_state` helper must be added to the gateway test utilities, mirroring
`wait_headlamp_state`.

---

### D13 — `FsmEvent` additions and transition-table self-loops

```rust
// crates/common/src/fsm/machineries.rs
pub enum FsmEvent {
    // … existing variants …
    RainsStarted,    // NEW — binary; no intensity payload
    RainsStopped,    // NEW
}
```

`transition_map.rs` self-loops for `RainsStarted`/`RainsStopped` in every state
(FSM does not change state on rain events; zone handles it):

```
Off               + RainsStarted/RainsStopped → Off,               no actions
PreparingToStart  + RainsStarted/RainsStopped → PreparingToStart,  no actions (filtered by zone_message_for_event anyway)
Idle              + RainsStarted/RainsStopped → Idle,              no actions
Driving / …       + RainsStarted/RainsStopped → self, no actions
PreparingToStop   + RainsStarted/RainsStopped → PreparingToStop,   no actions
```

---

## RED tests (new file: `test/wiper_zone_contract.rs`)

| Test | Asserts |
|---|---|
| `test_wiper_zone_id_exists_and_is_distinct_from_headlamp` | `ZoneId::Wiper != ZoneId::Headlamp`; both `Debug`-print |
| `test_rains_started_routes_to_wiper_zone` | `zone_message_for_event(&RainsStarted, Driving)` → `Some((ZoneId::Wiper, ZoneMessage::Wiper(WiperMessage::Start)))` |
| `test_rains_started_suppressed_during_preparing_to_start` | `zone_message_for_event(&RainsStarted, PreparingToStart)` → `None` |
| `test_wiper_become_on_transitions_to_ready` | `WiperContext::default().on_receiving_message(BecomeOn)` → `WiperState::Ready` |
| `test_wiper_start_while_ready_transitions_to_running` | `on_receiving_message(Start)` from `Ready` → `WiperState::Running` |
| `test_wiper_stop_while_running_transitions_to_ready` | `on_receiving_message(Stop)` from `Running` → `WiperState::Ready` |
| `test_wiper_become_off_from_running_transitions_to_off_directly` | `on_receiving_message(BecomeOff)` from `Running` → `WiperState::Off` (no intermediate) |
| `test_concurrent_headlamp_and_wiper_events_commit_in_arrival_order` | Two zone-directed events to different assemblies; replies arrive in reverse order; ledger sequence = arrival order; `old_ctx` values are accurate |
| `test_wiper_included_in_startup_barrier` | After `PowerOn`, both `BecomeOn` tells are sent (headlamp and wiper) before any `AssemblyZoneReady` is committed |
| `test_slow_wiper_does_not_delay_headlamp_event_commit` | Headlamp barrier completes; headlamp event commits even while a wiper barrier is still pending (different turns) |

**Additional RED tests in existing files:**

| File | Test | Asserts |
|---|---|---|
| `test/zone_replies_contract.rs` | `test_zone_replies_map_get_returns_none_for_absent_zone` | `ZoneReplies::simulate_locally().get(&ZoneId::Headlamp)` → `None` |
| `test/zone_replies_contract.rs` | `test_zone_replies_with_reply_stores_and_retrieves` | `ZoneReplies::with_reply(Headlamp, r).get(&ZoneId::Headlamp)` → `Some(&r)` |
| `test/fsm_preparation_contract.rs` | `test_rains_started_is_self_loop_in_idle` | `step(Idle, ctx, RainsStarted)` → `next_state == Idle`, no actions |

---

## Implementation steps (sequenced)

### Step 1 — Write RED tests (all failing for the right reasons)

Create `crates/common/src/test/wiper_zone_contract.rs`.  Add module entry to `lib.rs`.
Add two tests to `zone_replies_contract.rs`; one test to `fsm_preparation_contract.rs`.

Confirm: `cargo test -p common 2>&1 | grep FAILED` shows only the new test names.

---

### Step 2 — Wiper L1 vocabulary (new file: `crates/common/src/vehicle_state/wiper.rs`)

Implement `WiperMessage`, `WiperState`, `WiperContext`, `WiperZoneReply`, `WiperOutcome`.
Implement `WiperContext::on_receiving_message(WiperMessage) -> WiperZoneReply` per the D3
state-machine table.

Re-export from `crates/common/src/vehicle_state/mod.rs`.

Extend `VehicleContext` with `pub wiper: WiperContext`.

**Checkpoint:** `cargo build -p common` compiles.
Tests `test_wiper_become_on_transitions_to_ready`, `test_wiper_start_while_ready_transitions_to_running`,
`test_wiper_stop_while_running_transitions_to_ready`, `test_wiper_become_off_from_running_transitions_to_off_directly`
are all GREEN.

---

### Step 3 — FSM vocabulary additions

- `crates/common/src/fsm/machineries.rs`:
  - Add `FsmEvent::RainsStarted` and `FsmEvent::RainsStopped`.
  - Add `ZoneId::Wiper`.
- `crates/common/src/fsm/transition_map.rs`:
  - Add `RainsStarted` and `RainsStopped` self-loop arms for every state.
- `crates/common/src/digital_twin/mod.rs`:
  - Add `ZoneReply::Wiper(crate::vehicle_state::WiperZoneReply)`.
  - Add `ZoneReply::as_headlamp()` and `ZoneReply::as_wiper()`.
  - Add `pub(crate) enum ZoneMessage { Headlamp(HeadlampMessage), Wiper(WiperMessage) }`.

**Checkpoint:** `cargo build -p common` compiles.
`test_wiper_zone_id_exists_and_is_distinct_from_headlamp` and
`test_rains_started_is_self_loop_in_idle` are GREEN.

---

### Step 4 — `zone_turn.rs` full update

- Rename `user_event_to_headlamp_tell` → `user_event_to_zone_tell`; new return type
  `Option<(ZoneId, ZoneMessage)>`; add `RainsStarted`/`RainsStopped` arms.
- Change `zone_message_for_event` return type to `Option<(ZoneId, ZoneMessage)>`.
- Add `pub enum ZoneOutcome { Headlamp(HeadlampOutcome), Wiper(WiperOutcome) }`.
- Rewrite `ZoneTurnResult`: replace `headlamp_outcomes`/`headlamp_before` with
  `outcomes: Vec<ZoneOutcome>` (no `before` fields — D4).
- Update `zone_turn` function body:
  - Replace `headlamp_outcomes` with generic `outcomes`.
  - Add wiper arms for `RainsStarted`/`RainsStopped` and `AssemblyZoneReady(Wiper)`.
  - Replace `zone_replies.headlamp.ingress.as_ref()` with
    `zone_replies.get(&ZoneId::Headlamp).and_then(ZoneReply::as_headlamp)`.
- Delete `merge_headlamp_for_message`; replace with generic `merge_zone_reply` or keep
  zone-specific private helpers.

**Checkpoint:** `cargo build -p common` fails at `twin_turn.rs` (old `ZoneTurnResult` fields)
and `virtual_car_actor.rs` — expected.
`test_rains_started_routes_to_wiper_zone` and
`test_rains_started_suppressed_during_preparing_to_start` are GREEN.

---

### Step 5 — `ZoneReplies` map migration

- `crates/common/src/twin_runtime/zone_replies.rs`:
  - Delete `HeadlampReplies` struct.
  - Change `ZoneReplies` to `{ pub replies: HashMap<ZoneId, ZoneReply> }`.
  - Rewrite: `simulate_locally`, `with_reply`, `get`.
  - Delete `with_headlamp_ingress`.

**Checkpoint:** `cargo build -p common` fails at call sites — expected.
`test_zone_replies_map_get_returns_none_for_absent_zone` and
`test_zone_replies_with_reply_stores_and_retrieves` are GREEN.

---

### Step 6 — `TurnBarrier` type lifts

- `crates/common/src/twin_runtime/turn_barrier.rs`:
  - `zone_messages: HashMap<ZoneId, ZoneMessage>`.
  - Update `add_pending_zone`, `zone_message` (returns `Option<ZoneMessage>` cloned), `new_for_assembly_zone`.
  - `into_resolved_turn`: trivial map move (D6).

---

### Step 7 — `WiperActor` (new file: `crates/common/src/twin_runtime/wiper_actor.rs`)

Implement `WiperActor`, `WiperActorMsg::Apply`, `WiperActorVocabulary`, `WiperActorState`.
Implement `tell_wiper_zone`.

Add `pub(crate) mod wiper_actor;` to `crates/common/src/twin_runtime/mod.rs`.

---

### Step 8 — `virtual_car_actor.rs` full update

- Add `wiper_actor: ActorRef<WiperActorMsg>` to `VirtualCarRuntimeState`.
- Add helpers: `become_on_message_for`, `become_off_message_for`, `tell_zone`,
  `synthetic_reply_for` (D11).
- Update `MANAGED_ASSEMBLIES` to `&[ZoneId::Headlamp, ZoneId::Wiper]`.
- Update `StartAssemblies`/`StopAssemblies` branches to use `tell_zone` and
  `become_on/off_message_for`.
- Update `begin_fsm_turn` to destructure `(zone_id, message)` from `zone_message_for_event`
  and use `tell_zone`.
- Remove headlamp-specific unpack from `on_zone_ready` (D11 / D9).
- Update `on_zone_timeout` to use `synthetic_reply_for`.
- Spawn `WiperActor` in `pre_start` alongside `HeadlampActor`.
- Update all import paths; remove `user_event_to_headlamp_tell` (renamed); remove
  explicit `ZoneId::Headlamp` hardcodes.

---

### Step 9 — `twin_turn.rs` and other call-site fixes

- `crates/common/src/twin_runtime/twin_turn.rs`:
  - Capture `headlamp_before` (and optionally `wiper_before`) at call site before `zone_turn`.
  - Update `ZoneTurnResult` accesses to the new field layout.
  - Update outcome filtering for actuation (now iterates `Vec<ZoneOutcome>`).

- All test fixtures: verify `VehicleContext` construction compiles with the new `wiper` field.

---

### Step 10 — Gateway tests

- `crates/gateway/tests/front_headlamp_e2e.rs`:
  - Add `wait_wiper_state` helper (mirror of `wait_headlamp_state`).
  - After `send_power_on()`: call both `wait_headlamp_state(Ready)` and
    `wait_wiper_state(Ready)`.
  - After `send_power_off()` (if tested): call both `wait_headlamp_state(Off)` and
    `wait_wiper_state(Off)`.

---

### Step 11 — Full suite green

```bash
cargo test -p common
cargo test -p gateway
```

All 13 new RED tests must be GREEN.  All prior 139 tests must remain GREEN.
`cargo build -p common` emits zero dead-code warnings.

---

## File change summary

| File | Action | Reason |
|---|---|---|
| `crates/common/src/vehicle_state/wiper.rs` | **New** | Wiper L1 alphabet + context + behavior |
| `crates/common/src/twin_runtime/wiper_actor.rs` | **New** | Wiper zone twinlet + `tell_wiper_zone` |
| `crates/common/src/test/wiper_zone_contract.rs` | **New** | 10 RED→GREEN tests |
| `crates/common/src/fsm/machineries.rs` | Modify | `ZoneId::Wiper`; `FsmEvent::RainsStarted/RainsStopped` |
| `crates/common/src/fsm/transition_map.rs` | Modify | Self-loop arms for `RainsStarted`/`RainsStopped` |
| `crates/common/src/digital_twin/mod.rs` | Modify | `ZoneMessage` enum; `ZoneReply::Wiper`; `as_headlamp`/`as_wiper` |
| `crates/common/src/twin_runtime/zone_turn.rs` | Modify | `ZoneOutcome`; generic `ZoneTurnResult`; `user_event_to_zone_tell`; wiper arms |
| `crates/common/src/twin_runtime/zone_replies.rs` | Modify | Map migration; delete `HeadlampReplies`; `with_reply`/`get` |
| `crates/common/src/twin_runtime/turn_barrier.rs` | Modify | `zone_messages` type lift; `zone_message` Clone return; `into_resolved_turn` simplification |
| `crates/common/src/twin_runtime/controller/virtual_car_actor.rs` | Modify | `MANAGED_ASSEMBLIES`; `wiper_actor`; dispatch helpers; `begin_fsm_turn`; `on_zone_ready`; `on_zone_timeout` |
| `crates/common/src/twin_runtime/twin_turn.rs` | Modify | `ZoneTurnResult` field updates; `headlamp_before` capture at call site |
| `crates/common/src/twin_runtime/mod.rs` | Modify | Expose `wiper_actor` module |
| `crates/common/src/vehicle_state/mod.rs` | Modify | Re-export wiper types; `VehicleContext.wiper` |
| `crates/common/src/test/zone_replies_contract.rs` | Modify | 2 new map-shape tests |
| `crates/common/src/test/fsm_preparation_contract.rs` | Modify | 1 new `RainsStarted` self-loop test |
| `crates/gateway/tests/front_headlamp_e2e.rs` | Modify | `wait_wiper_state` helper; both twinlets waited during startup/shutdown |

---

## Discussion checkpoint after Phase 7

1. `cargo test -p common && cargo test -p gateway` — full suite green.
2. All 13 new tests GREEN; all prior 139 tests GREEN.
3. `cargo build -p common` emits zero dead-code warnings.  Specifically:
   - `user_event_to_headlamp_tell` is gone (renamed `user_event_to_zone_tell`).
   - `with_headlamp_ingress` is gone (replaced by `with_reply`).
   - `HeadlampReplies` struct is gone.
   - `ZoneTurnResult::headlamp_before` is gone (caller captures from input `ctx`).
4. Confirm `handle()` still has exactly **four arms**: `Fsm`, `ZoneReady`,
   `ZoneTellBackTimeout`, `GetStatus`.  Adding Wiper required zero new arms.
5. Inspect `test_concurrent_headlamp_and_wiper_events_commit_in_arrival_order` output:
   the two `old_ctx` entries must differ in exactly the expected way — first has neither
   wiper nor headlamp update applied; second has exactly one applied.
6. Walk the startup sequence: `PowerOn → PreparingToStart → BecomeOn×2 → AssemblyZoneReady×2 → Idle`.
   Barrier queue drain order: `[Headlamp-startup, Wiper-startup]` (insertion order of
   `MANAGED_ASSEMBLIES`).  FSM stays in `PreparingToStart` until both drain.
7. **Pre-Phase 8 decision:** the exact Rust syntax for embedding `&'static [AssemblyId]`
   in `FsmState::PreparingToStart { assemblies: ... }` — `const` generic vs. `&'static` slice
   vs. a `AssemblySet` newtype.  Agree before Phase 8 starts.
