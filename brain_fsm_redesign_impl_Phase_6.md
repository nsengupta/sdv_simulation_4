# Brain FSM Redesign — Phase 6 Implementation Plan
## State-Aware Zone Routing; Delete Speculative Execution

**Status:** Design reviewed and ready for implementation — gate conditions from Phase 5 checkpoint verified.  
**Depends on:** Phase 5 complete (`StartAssemblies`/`StopAssemblies` wired to `BecomeOn`/`BecomeOff` barriers).  
**Next phase:** Phase 7 — Wiper as Second Assembly.

---

## Phases 1–5 achieved so far

| Phase | Tag | Core deliverable |
|---|---|---|
| 1 | `phase-1-fsm-vocabulary` | `PreparingToStart`/`PreparingToStop` states; `StartAssemblies`/`StopAssemblies` actions; `AssembliesReady`/`AssembliesStopped` internal events; 8 RED→GREEN tests |
| 2 | `phase-2-headlamp-zone-alphabet` | `HeadlampState::Ready`; `HeadlampMessage::BecomeOn`/`BecomeOff`; `ZoneId::Headlamp`; 9 RED→GREEN tests |
| 3 | `phase-3-generic-zone-envelope` | `ZoneReply`, `ZoneSpontaneousEvent`, `ZoneReady`/`ZoneSpontaneous`/`ZoneTellBackTimeout` in `DigitalTwinCarVocabulary`; handler rename; 3 RED→GREEN tests |
| 4 | `phase-4-reorder-buffer-barrier-queue` | `TurnBarrier`; `barrier_queue: VecDeque<TurnBarrier>`; HOB drain loop; `alloc_turn_id`; 4 RED→GREEN tests |
| 5 | `phase-5-startup-shutdown-barriers` | `FsmEvent::AssemblyZoneReady(ZoneId)`; `VehicleContext.pending_assemblies`; `MANAGED_ASSEMBLIES`; `TurnBarrier::new_for_assembly_zone`; `PassthroughBarrier`; 4 RED→GREEN tests |

---

## Phase 5 gate check — required before Phase 6 begins

Phase 5's discussion checkpoint 3 mandates: add `unreachable!()` to the `IgnitionOffReset` arm in
`on_zone_ready` (and the parallel block in `begin_fsm_turn`) and confirm `cargo test -p common`
passes cleanly.  This is not a code change to ship — it is a verification step only.  After the
full suite passes with those guards in place, Phase 6 may proceed to delete the dead code.

The path that becomes unreachable is:

```rust
// begin_fsm_turn (virtual_car_actor.rs ~line 342):
if fsm_step_lands_off(...) {
    unreachable!("IgnitionOffReset path is dead after Phase 5 — Phase 6 deletes this block");
}

// on_zone_ready (~line 433):
if let Some(barrier_now) = needs_ignition_off_reset {
    unreachable!("IgnitionOffReset path is dead after Phase 5 — Phase 6 deletes this block");
}
```

Run `cargo test -p common` with these guards — the suite must pass with zero failures.
**Only then proceed to the deletion steps below.**

---

## What Phase 6 delivers

Phase 6 is the **cleanup phase** for two classes of dead code that became unreachable after
Phase 5, plus one architectural improvement and two deferred shim deletions from Phase 5.

1. **State-aware zone routing** — introduce `zone_message_for_event(event, state)` in
   `zone_turn.rs`.  In `PreparingToStart`/`PreparingToStop`, this function returns `None`,
   causing `begin_fsm_turn` to push a `PassthroughBarrier` instead of a zone-directed
   `TurnBarrier`.  No twinlet tell is emitted for user events arriving during assembly
   lifecycle phases.

2. **Delete `IgnitionOffReset` machinery** — `BarrierPhase::IgnitionOffReset`, `fsm_step_lands_off`,
   `start_ignition_off_reset`, the `IgnitionOffReset` block in `apply_external_hop`, and the
   `ignition_off_reset` field in `HeadlampReplies`.  Every one of these was kept as scaffolding
   pending this gate check.

3. **Delete `DomainAction::EnterMode` + `ActorModeHintFromDomain`** — `EnterMode` was
   stubbed with `let _ =` in Phase 1 and retained as a no-op through every phase since.
   After Phase 6, the concept does not exist in the codebase.

4. **Delete `initial_headlamp_ctx`** from `VehicleControllerRuntimeOptions` — the shim was
   marked "Phase 6 cleanup item" in Phase 5's discussion checkpoint 4.  All tests now boot
   through the `BecomeOn` automatic flow.

5. **Delete `Operational::AssembliesReady` and `Operational::AssembliesStopped`** — their
   transition arms were removed in Phase 5; no code path produces these events anymore.
   `Operational::LightingUnsafe` is retained; the `Operational` enum itself survives.

After Phase 6, `handle()` retains exactly **four arms**, `begin_fsm_turn` retains exactly
**two decision branches** (zone-directed vs. passthrough), and the deletion checklist from
the plan document is fully satisfied.

---

## Design decisions

### D1 — `zone_message_for_event` return type: `Option<HeadlampMessage>` for Phase 6

The plan document specifies `Option<(ZoneId, ZoneMessage)>`.  For Phase 6 only Headlamp
exists, so the `ZoneId` is always implicit and `ZoneMessage` would be a wrapper around
`HeadlampMessage`.  Introducing a new `ZoneMessage` enum now would create another symbol for
Phase 7 to immediately generalise.

**Decision:** Phase 6 signature is `pub(crate) fn zone_message_for_event(event: &FsmEvent, state: &FsmState) -> Option<HeadlampMessage>`.  The inner `user_event_to_headlamp_tell` continues to exist as the state-unaware delegate.  Phase 7 lifts the return type to `Option<(ZoneId, ZoneLifecycleMessage)>` when Wiper is added.

### D2 — `zone_turn` remains state-unaware in Phase 6

`zone_turn` is called by `apply_external_hop` in `twin_turn.rs` during committed-turn
processing.  For a `PassthroughBarrier` committed during `PreparingToStart`, `zone_replies.headlamp.ingress` is `None`; `merge_headlamp_for_message` falls back to the local in-process
simulation.  The headlamp context IS updated locally (e.g. `AmbientLux` applied to the
headlamp model).

This is intentionally left as-is for Phase 6.  The headlamp actor is the authoritative source;
local simulation is an approximation that is overwritten when the actual `BecomeOn` barrier
commits.  Making `zone_turn` fully state-aware requires the Phase 7 map-based `ZoneReplies`
redesign.

### D3 — `apply_external_hop` loses its `DomainAction::EnterMode` filter

After deleting `DomainAction::EnterMode`, the filter `!matches!(action, DomainAction::EnterMode(_))` in `apply_external_hop` has no match targets.  The entire filter is removed; the two-line
"recorded\_actions" closure in `step.rs` loses only the `EnterMode` branch (the `StartAssemblies`
/ `StopAssemblies` exclusion is retained for ledger correctness).

### D4 — `ZoneReplies::with_headlamp` is deleted; `with_headlamp_ingress` is kept

`with_headlamp(ingress, ignition_off_reset)` is the two-argument constructor that would need
updating at every call site after deleting `ignition_off_reset`.  Since the `IgnitionOffReset`
deletion removes all call sites of the two-argument form, the constructor is deleted outright.
`with_headlamp_ingress(ingress)` (one argument) is the only non-default constructor kept.

---

## Code changes

### `crates/common/src/twin_runtime/zone_turn.rs`

Add `zone_message_for_event` as a state-aware wrapper around `user_event_to_headlamp_tell`:

```rust
/// State-aware zone routing for a *user-originated* [`FsmEvent`].
///
/// Returns `None` when the FSM is in a lifecycle transition state (`PreparingToStart` or
/// `PreparingToStop`): no zone tell is emitted for user events during assembly startup or
/// shutdown.  For all other states, delegates to [`user_event_to_headlamp_tell`].
///
/// Used by `begin_fsm_turn` to decide between a zone-directed [`TurnBarrier`] and a
/// [`PassthroughBarrier`].  Phase 7 generalises the return type to `Option<(ZoneId, ZoneMessage)>`
/// when a second assembly (Wiper) is introduced.
pub(crate) fn zone_message_for_event(
    event: &FsmEvent,
    state: &FsmState,
) -> Option<HeadlampMessage> {
    match state {
        FsmState::PreparingToStart | FsmState::PreparingToStop => None,
        _ => user_event_to_headlamp_tell(event),
    }
}
```

`user_event_to_headlamp_tell` is unchanged.

### `crates/common/src/twin_runtime/controller/virtual_car_actor.rs`

**1. Replace `user_event_to_headlamp_tell` with `zone_message_for_event` in `begin_fsm_turn`**

```rust
// Replace:
if let Some(message) = user_event_to_headlamp_tell(&event) {

// With:
if let Some(message) = zone_message_for_event(&event, runtime_state.twin_car.current_state()) {
```

**2. Delete the entire `IgnitionOffReset` block in `begin_fsm_turn` (lines ~339–357)**

Delete:
```rust
// Pure ignition-off reset: event has no headlamp message, but FSM will land on Off.
// After Phase 5 this path is unreachable ...  Phase 6 removes it.
if fsm_step_lands_off( ... ) {
    let msg = HeadlampMessage::ResetForIgnitionOff;
    ...
    return Ok(());
}
```

After this deletion, `begin_fsm_turn` has exactly **two** decision branches:
- zone-directed (`zone_message_for_event` returns `Some`) → `TurnBarrier`
- passthrough (`zone_message_for_event` returns `None`) → `PassthroughBarrier`

**3. Delete `IgnitionOffReset` handling from `on_zone_ready` (lines ~397–453)**

Delete the `needs_ignition_off_reset: Option<Instant>` computation block and the subsequent
`if let Some(barrier_now) = needs_ignition_off_reset { ... }` block.

Simplified `on_zone_ready` (after deletion):

```rust
async fn on_zone_ready(...) -> Result<(), ActorProcessingErr> {
    let reply_hl = match reply {
        crate::digital_twin::ZoneReply::Headlamp(r) => r,
    };

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

    barrier.act_on_zone_reply(zone_id, crate::digital_twin::ZoneReply::Headlamp(reply_hl));
    Ok(())
}
```

The drain-loop call (`try_drain_barrier_queue`) follows immediately in the caller; no change
there.

**4. Update doc-comment on `begin_fsm_turn`** — delete path 2 ("Ignition-off reset") from the
three-path description.  The new comment describes two paths only:

```rust
/// Two mutually exclusive paths:
///
/// 1. **Zone-directed** — `zone_message_for_event` returns `Some`.  A [`TurnBarrier`] with
///    `Headlamp` pending is created; the zone gets a tell and a timer.
///
/// 2. **Passthrough** — `zone_message_for_event` returns `None`.  The [`PassthroughBarrier`]
///    is instantly drainable and keeps the queue ordered.
```

**5. Delete `initial_headlamp_ctx` usage** — remove the block that applied an initial headlamp
override in `pre_start` / actor initialization:

```rust
// DELETE this block from pre_start or wherever it is applied:
if let Some(hl_ctx) = args.runtime_options.initial_headlamp_ctx.clone() {
    runtime_state.twin_car.context_mut().headlamp = hl_ctx;
    // ... twinlet actor state override ...
}
```

**6. Delete `DomainAction::EnterMode` no-op arm** from `apply_committed_quiescence`:

```rust
// DELETE:
DomainAction::EnterMode(_) => {}
```

**7. Update imports** — remove imports of `fsm_step_lands_off`, `ZoneReplies` (if no longer
used after the `IgnitionOffReset` block deletion), `HeadlampMessage::ResetForIgnitionOff`,
`BarrierPhase`, `user_event_to_headlamp_tell` (replaced by `zone_message_for_event`).
Add import of `zone_message_for_event`.

### `crates/common/src/twin_runtime/turn_barrier.rs`

**1. Delete `BarrierPhase` enum entirely** — both `Primary` and `IgnitionOffReset` variants.

**2. Delete `phase` field from `TurnBarrier`**, the `phase()` getter, and the
`start_ignition_off_reset` method.

**3. Simplify `into_resolved_turn`** — remove the match on `BarrierPhase::IgnitionOffReset`;
the method now always builds `ZoneReplies` from the primary replies only.

Before (sketch):
```rust
let zone_replies = match self.phase {
    BarrierPhase::Primary => ZoneReplies::with_headlamp_ingress(...),
    BarrierPhase::IgnitionOffReset { primary_reply } => {
        ZoneReplies::with_headlamp(primary_reply, Some(reset_reply))
    }
};
```

After:
```rust
let zone_replies = ZoneReplies::with_headlamp_ingress(
    self.replies.get(&ZoneId::Headlamp).and_then(|r| match r {
        ZoneReply::Headlamp(hl) => Some(hl.clone()),
    }),
);
```

**4. Update module-level doc-comment** — remove the `IgnitionOffReset` phase from the
lifecycle diagram and the Phase-6 note.

### `crates/common/src/twin_runtime/twin_turn.rs`

**1. Delete `fsm_step_lands_off`** (lines 125–157).

**2. Delete the `IgnitionOffReset` block from `apply_external_hop`** (lines 194–203):
```rust
// DELETE:
if matches!(result.next_state, FsmState::Off) {
    let zone_reply = zone_replies.headlamp.ignition_off_reset.clone().unwrap_or_else(|| {
        result.modified_ctx.headlamp.on_receiving_message(
            HeadlampMessage::ResetForIgnitionOff, now,
        )
    });
    result.modified_ctx.headlamp = zone_reply.ctx;
    headlamp_outcomes.extend(zone_reply.outcomes);
}
```

**3. Delete the `DomainAction::EnterMode` filter from `apply_external_hop`** (lines 211–215):
```rust
// DELETE:
let recorded_actions: Vec<DomainAction> = result
    .actions
    .iter()
    .filter(|action| !matches!(action, DomainAction::EnterMode(_)))
    .cloned()
    .collect();
result.transition_record.actions = recorded_actions;
result.transition_record.current_ctx = result.modified_ctx.clone();
```

After deletion, `result.transition_record` is no longer overwritten here — its `actions` field
is already set correctly by `step.rs` (which retains the `StartAssemblies`/`StopAssemblies`
exclusion).  Remove the two assignment lines.

**4. Remove the unused `HeadlampMessage` import** if it is no longer referenced in this file.

### `crates/common/src/twin_runtime/zone_replies.rs`

**1. Delete `HeadlampReplies.ignition_off_reset` field**:

```rust
// BEFORE:
pub struct HeadlampReplies {
    pub ingress: Option<HeadlampZoneReply>,
    pub ignition_off_reset: Option<HeadlampZoneReply>,
}

// AFTER:
pub struct HeadlampReplies {
    pub ingress: Option<HeadlampZoneReply>,
}
```

**2. Delete `ZoneReplies::with_headlamp(ingress, ignition_off_reset)` constructor** entirely.
Update any call site that used it — after Phase 5 there should be none remaining (the
`IgnitionOffReset` logic in `on_zone_ready` and `begin_fsm_turn` used it and will be deleted
above).  Verify with `cargo build`.

**3. Keep `with_headlamp_ingress(ingress)` and `simulate_locally()`** unchanged.

**4. Update the module doc-comment** — remove the sentence "Actor path: brain fills this after
tell-back(s)" mention of `ignition_off_reset`.

### `crates/common/src/fsm/machineries.rs`

**1. Delete `ActorModeHintFromDomain` enum** (lines 84–88):
```rust
// DELETE:
pub enum ActorModeHintFromDomain {
    Normal,
    Transitioning,
}
```

**2. Delete `DomainAction::EnterMode(ActorModeHintFromDomain)` variant** (line 104).

**3. Delete `Operational::AssembliesReady` and `Operational::AssembliesStopped` variants**.
`Operational::LightingUnsafe` is retained; the `Operational` enum stays.

**4. Update module doc-comment** if it references `EnterMode` or the deleted `Operational` variants.

### `crates/common/src/fsm/step.rs`

**1. Remove the two `DomainAction::EnterMode(...)` pushes** (lines 87–90):
```rust
// DELETE both lines:
actions.push(DomainAction::EnterMode(ActorModeHintFromDomain::Transitioning));
actions.push(DomainAction::EnterMode(ActorModeHintFromDomain::Normal));
```

The `if / else` block around them becomes dead; delete the entire conditional too.

**2. Remove `DomainAction::EnterMode(_)` from the recorded-actions filter** (line 100):
```rust
// BEFORE:
.filter(|action| !matches!(
    action,
    DomainAction::EnterMode(_)
        | DomainAction::StartAssemblies
        | DomainAction::StopAssemblies
))

// AFTER:
.filter(|action| !matches!(
    action,
    DomainAction::StartAssemblies | DomainAction::StopAssemblies
))
```

**3. Remove the `ActorModeHintFromDomain` import** from the `use super::machineries` line.

### `crates/common/src/published.rs`

**Remove `DomainAction::EnterMode(_)` arm** from `impl From<&DomainAction> for PublishedDomainAction`
(line ~199) and from any associated doc-comment exclusion list (line ~176).

### `crates/common/src/twin_runtime/controller/actuation_manager.rs`

**Remove `DomainAction::EnterMode(_) => {}` no-op arm** (line 123).

### `crates/common/src/twin_runtime/controller/vehicle_controller.rs`

**Delete `initial_headlamp_ctx: Option<HeadlampContext>` field** from
`VehicleControllerRuntimeOptions` (line 35) and its `None` default (line 48).

Update the doc-comment on the struct to remove the `initial_headlamp_ctx` entry.

### `crates/common/src/fsm/mod.rs`

Remove `ActorModeHintFromDomain` from the re-export list (line 13).

---

## RED tests

### New functions in `crates/common/src/test/fsm_preparation_contract.rs`

These three tests call `zone_message_for_event`, which does not exist before Phase 6.  They
fail with a compile error (undefined symbol) and turn GREEN after the function is added.

#### `test_zone_message_for_event_returns_none_during_preparing_to_start`

```
Given: FsmEvent::UpdateAmbientLux(10), FsmState::PreparingToStart.
When:  zone_message_for_event(event, state) is called.
Then:  result is None.

RED: compile error — zone_message_for_event does not exist.
```

#### `test_zone_message_for_event_returns_none_during_preparing_to_stop`

```
Given: FsmEvent::UpdateAmbientLux(10), FsmState::PreparingToStop.
When:  zone_message_for_event(event, state) is called.
Then:  result is None.

Same RED cause.
```

#### `test_zone_message_for_event_returns_some_during_driving`

```
Given: FsmEvent::UpdateAmbientLux(10), FsmState::Driving.
When:  zone_message_for_event(event, state) is called.
Then:  result is Some(HeadlampMessage::AmbientLux(10)).

RED: compile error — zone_message_for_event does not exist.
```

### New file: `crates/common/src/test/zone_replies_contract.rs`

Three structural / behavioral tests for the simplified `ZoneReplies` and the deleted
ignition-off-reset path.

#### `test_zone_replies_simulate_locally_has_no_ignition_off_reset`

```
Given: ZoneReplies::simulate_locally().
When:  HeadlampReplies is constructed.
Then:  The struct has exactly one field: `ingress: None`.
       Constructing `HeadlampReplies { ingress: None, ignition_off_reset: None }` is a
       compile error — field `ignition_off_reset` does not exist.

RED: before Phase 6, HeadlampReplies still carries ignition_off_reset — the two-field
constructor compiles.  After Phase 6, the one-field form is the only valid one.
```

In practice, the test body verifies:
```rust
let r = ZoneReplies::simulate_locally();
assert_eq!(r.headlamp.ingress, None);
// Compile-time enforcement: this line must NOT compile after Phase 6:
// let _ = HeadlampReplies { ingress: None, ignition_off_reset: None };
```

The negative compile assertion can be expressed as a `compile_fail` doc-test on the
`HeadlampReplies` struct (optional), or simply as a behavioral assertion that
`ZoneReplies::with_headlamp_ingress(None)` is the only constructor available.

#### `test_power_off_does_not_speculatively_run_zone_turn`

```
Given: FsmState::Idle, VehicleContext::default() with a headlamp in Ready state,
       FsmEvent::PowerOff, ZoneReplies::simulate_locally().
When:  apply_external_hop (via twin_turn) is called once.
Then:  result.next_state == FsmState::PreparingToStop.
       result.modified_ctx.headlamp state is unchanged (no ResetForIgnitionOff applied).
       The result was produced by a single FSM step — no second zone_turn invocation.

RED in Phase 5: apply_external_hop still has the IgnitionOffReset block; but
fsm_step_lands_off returns false (PowerOff → PreparingToStop, not Off), so the block
does NOT fire.  The test actually passes in Phase 5 — meaning this is a verification
test (green already), not a RED→GREEN test.

Correct framing: this is a regression-guard test.  It turns RED only if the
IgnitionOffReset block is ever re-introduced.  Add it now as a guard.
```

#### `test_headlamp_replies_with_headlamp_ingress_is_the_only_constructor`

```
Given: A HeadlampZoneReply value.
When:  ZoneReplies::with_headlamp_ingress(reply) is called.
Then:  zone_replies.headlamp.ingress == Some(reply).
       No `with_headlamp(ingress, ignition_off_reset)` two-argument constructor exists
       (compile-time enforcement).

RED: with_headlamp still exists before Phase 6.
GREEN after Phase 6: with_headlamp is deleted; with_headlamp_ingress is the only
non-default constructor.
```

> **Module registration:** add `mod zone_replies_contract;` to the `#[cfg(test)]` block in
> `crates/common/src/test/mod.rs`.

---

## Existing tests: impact analysis

| Test file | Change required | Reason |
|---|---|---|
| `actor_contract.rs` | Remove `initial_headlamp_ctx: Some(...)` from 3 test setups | Field deleted; the BecomeOn flow from `power_on_to_idle` puts headlamp in `Ready` automatically |
| `quiescence_actor_contract.rs` | Remove `initial_headlamp_ctx: Some(...)` from 1 test setup | Same |
| `headlamp_ack_timer_contract.rs` | Remove `initial_headlamp_ctx: Some(...)` from 2 test setups | Same |
| `fsm_step_contract.rs` | Update `test_state_laws_hold_over_a_legal_journey_and_records_carry_intents` and `test_step_standard_commute_flow` | These tests assert that `EnterMode` is in the execution feed but not in the ledger record; after `EnterMode` is deleted both assertions change: the execution feed no longer contains `EnterMode`, and the filter becomes simpler. Replace the `EnterMode`-specific assertions with an assertion that `StartAssemblies` and `StopAssemblies` are excluded from the ledger record. |
| `fsm_preparation_contract.rs` | Remove `EnterMode` comments (line 114 reference) | Comment cleanup only; no test logic change |
| `published.rs` (not a test file) | Remove `EnterMode(_)` match arm | Compile fix after `DomainAction::EnterMode` deletion |
| `turn_barrier_contract.rs` | No change | `boot_silent` and constants are unaffected |
| `startup_barrier_contract.rs` | No change | Tests exercise `BecomeOn`/`BecomeOff` path, not ignition-off reset |
| `zone_tell_back_contract.rs` | No change | Silent-headlamp boot pattern is unaffected |
| `headlamp_reply_contract.rs` | No change | Already updated in Phase 5 |
| `scenarios_smoke.rs` | No change | Already uses `power_on_to_idle` |
| `controller_api_contract.rs` | No change | Already uses `power_on_to_idle` / `power_off_to_off` |
| `headlamp_lifecycle_contract.rs` | No change | Headlamp actor unit tests; unaffected |

### `actor_contract.rs` — `initial_headlamp_ctx` removal detail

Three tests currently set `initial_headlamp_ctx: Some(HeadlampContext { state: HeadlampState::Ready })`:

```rust
// BEFORE (all three):
let opts = VehicleControllerRuntimeOptions {
    initial_headlamp_ctx: Some(HeadlampContext { state: HeadlampState::Ready, ..Default::default() }),
    ..Default::default()
};

// AFTER:
let opts = VehicleControllerRuntimeOptions::default();
```

The headlamp starts in `Off`; `power_on_to_idle` sends `PowerOn`, the brain sends `BecomeOn`
to the headlamp actor, which replies `ZoneReady { state: Ready }`, and the FSM reaches `Idle`
with `headlamp.state == Ready`.  No test logic changes beyond removing the override.

### `fsm_step_contract.rs` — `EnterMode` assertion update

The test `test_state_laws_hold_over_a_legal_journey_and_records_carry_intents` currently
verifies two things about `EnterMode`:

```rust
// assertion 1 — currently passes:
assert!(step_result.actions.iter().any(|a| matches!(a, DomainAction::EnterMode(_))));

// assertion 2 — currently passes:
assert!(step_result.transition_record.actions.iter().all(|a| !matches!(a, DomainAction::EnterMode(_))));
```

After Phase 6, `EnterMode` does not exist.  Replace both assertions with equivalent checks
about `StartAssemblies`/`StopAssemblies`:

```rust
// NEW assertion 1 — StartAssemblies appears in execution feed on PowerOn:
assert!(step_result.actions.iter().any(|a| matches!(a, DomainAction::StartAssemblies)));

// NEW assertion 2 — StartAssemblies is excluded from the ledger record:
assert!(step_result.transition_record.actions.iter().all(|a| !matches!(a, DomainAction::StartAssemblies)));
```

The test's narrative intent (execution feed vs. ledger record difference) is preserved; only
the example action changes from `EnterMode` to `StartAssemblies`.

---

## Deletion checklist (from plan document Gap 3 table)

| Item deleted | File | Status after Phase 6 |
|---|---|---|
| `apply_external_hop` ignition-off block | `twin_turn.rs` | ✅ Deleted |
| `HeadlampReplies.ignition_off_reset` field | `zone_replies.rs` | ✅ Deleted |
| `ZoneReplies::with_headlamp(ingress, ignition_off_reset)` | `zone_replies.rs`, call sites | ✅ Deleted |
| `BarrierPhase::IgnitionOffReset` variant + `start_ignition_off_reset` method | `turn_barrier.rs` | ✅ Deleted |
| `fsm_step_lands_off` function | `twin_turn.rs` | ✅ Deleted |
| `DomainAction::EnterMode` + `ActorModeHintFromDomain` | `machineries.rs`, `step.rs`, `virtual_car_actor.rs`, `actuation_manager.rs`, `published.rs` | ✅ Deleted |

Additional Phase-6 deletions (not in the original plan table but arising from Phase 5):

| Item deleted | File |
|---|---|
| `initial_headlamp_ctx` field | `vehicle_controller.rs`, `virtual_car_actor.rs` (usage) |
| `Operational::AssembliesReady` and `Operational::AssembliesStopped` variants | `machineries.rs` |
| `IgnitionOffReset` handling block in `on_zone_ready` | `virtual_car_actor.rs` |
| `IgnitionOffReset` block in `begin_fsm_turn` | `virtual_car_actor.rs` |

---

## Discussion checkpoint after Phase 6

1. **`cargo test -p common` and `cargo build -p common` — full suite green, zero dead-code
   warnings.**  Specifically: all 6 new tests in `fsm_preparation_contract.rs` and
   `zone_replies_contract.rs` green.

2. **Walk the deletion checklist** — every item in the table above is gone.  Run
   `cargo build -p common 2>&1 | grep 'unused\|dead_code'` and confirm empty output.

3. **`begin_fsm_turn` has exactly two branches** — zone-directed and passthrough.  Trace
   `UpdateAmbientLux(10)` in `Idle` (zone-directed → `TurnBarrier`) and in `PreparingToStart`
   (passthrough → `PassthroughBarrier`) to confirm state-aware routing is live.

4. **HOB invariant confirmed** — `UpdateAmbientLux` in `PreparingToStart` becomes a
   `PassthroughBarrier` that drains trivially in arrival order behind any assembly barriers.
   Confirm by inspecting the `turn_barrier_contract.rs` test that exercises `boot_silent` +
   an ambient-lux event during startup: the lux ledger row must appear AFTER the
   `AssemblyZoneReady` row (not before).

5. **`handle()` has exactly four arms** — `Fsm`, `ZoneReady`, `ZoneTellBackTimeout`,
   `GetStatus`.  No new arms were added in Phase 6.

6. **Architecture review checkpoint** (from plan document Phase 6 checkpoint 5): run the
   property tests and e2e tests together before introducing the second assembly:
   ```
   cargo test -p common --features proptest
   cargo test -p gateway
   ```
   Both must pass before Phase 7 begins.

7. **Phase 7 pre-conditions agreed:**
   - `zone_message_for_event` signature will be lifted to `Option<(ZoneId, ZoneLifecycleMessage)>` in Phase 7.
   - `MANAGED_ASSEMBLIES` constant will be extended to `&[ZoneId::Headlamp, ZoneId::Wiper]`.
   - `ZoneReplies` will migrate to a map-based shape in Phase 7 (`HashMap<ZoneId, ZoneReply>`).
   - Sign off on the `ZoneLifecycleMessage` enum name and shape before Phase 7 starts.

---

## Files changed

| File | Change |
|---|---|
| `twin_runtime/zone_turn.rs` | Add `zone_message_for_event(event, state)` state-aware routing function |
| `twin_runtime/twin_turn.rs` | Delete `fsm_step_lands_off`; delete `IgnitionOffReset` block in `apply_external_hop`; remove `EnterMode` filter in `apply_external_hop` |
| `twin_runtime/turn_barrier.rs` | Delete `BarrierPhase` enum; delete `phase` field; delete `start_ignition_off_reset`; simplify `into_resolved_turn` |
| `twin_runtime/zone_replies.rs` | Delete `HeadlampReplies.ignition_off_reset`; delete `ZoneReplies::with_headlamp`; update doc-comment |
| `twin_runtime/controller/virtual_car_actor.rs` | Use `zone_message_for_event` in `begin_fsm_turn`; delete `IgnitionOffReset` blocks in `begin_fsm_turn` and `on_zone_ready`; delete `initial_headlamp_ctx` usage; delete `EnterMode` no-op arm |
| `twin_runtime/controller/vehicle_controller.rs` | Delete `initial_headlamp_ctx` field from `VehicleControllerRuntimeOptions` |
| `twin_runtime/controller/actuation_manager.rs` | Delete `EnterMode` no-op arm |
| `fsm/machineries.rs` | Delete `ActorModeHintFromDomain`; delete `DomainAction::EnterMode`; delete `Operational::AssembliesReady` and `Operational::AssembliesStopped` |
| `fsm/step.rs` | Remove `EnterMode` pushes and import; update recorded-actions filter |
| `fsm/mod.rs` | Remove `ActorModeHintFromDomain` from re-exports |
| `published.rs` | Remove `EnterMode` match arm |
| `test/mod.rs` | Register `mod zone_replies_contract` |
| `test/fsm_preparation_contract.rs` | Add 3 new `zone_message_for_event` tests |
| `test/zone_replies_contract.rs` | **NEW** — 3 structural/behavioral tests |
| `test/actor_contract.rs` | Remove `initial_headlamp_ctx` from 3 test setups |
| `test/quiescence_actor_contract.rs` | Remove `initial_headlamp_ctx` from 1 test setup |
| `test/headlamp_ack_timer_contract.rs` | Remove `initial_headlamp_ctx` from 2 test setups |
| `test/fsm_step_contract.rs` | Replace `EnterMode` assertions with `StartAssemblies` equivalents |

All other files unchanged in Phase 6.  All paths are relative to `crates/common/src/`.
