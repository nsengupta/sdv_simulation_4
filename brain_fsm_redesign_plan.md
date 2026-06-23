# Brain Actor FSM Redesign — RED-to-GREEN Staged Plan

## Ground Rules

- **One phase = one compilable, testable unit.** The codebase must compile and all prior tests must pass before a phase is declared done.
- **RED first.** Write the failing test(s) before touching implementation code. Confirm they fail for the right reason.
- **GREEN minimum.** Only the code required to pass the new tests. No speculative cleanup ahead of the phase.
- **Discussion gate.** After each GREEN, a discussion checkpoint is held. The next phase begins only after explicit sign-off.
- **Test files** live in `crates/common/src/test/`. New test modules are added to the `mod test` block in `crates/common/src/lib.rs`. Integration tests that require actors run with `tokio::test` via the `test/mod.rs` helpers.

---

## Phase 1 — FSM Vocabulary: `PreparingToStart`, `PreparingToStop`, new Internal events

**Scope:** `crates/common/src/fsm/machineries.rs` and `crates/common/src/fsm/transition_map.rs`

### RED tests (new file: `test/fsm_preparation_contract.rs`)

| Test | Asserts |
|------|---------|
| `test_power_on_transitions_to_preparing_to_start` | `step(Off, ctx, PowerOn)` → `next_state == PreparingToStart` |
| `test_assemblies_ready_from_preparing_to_start_transitions_to_idle` | `step(PreparingToStart, ctx, Internal(AssembliesReady))` → `Idle` |
| `test_power_off_from_idle_transitions_to_preparing_to_stop` | `step(Idle, ctx, PowerOff)` → `PreparingToStop` |
| `test_assemblies_stopped_from_preparing_to_stop_transitions_to_off` | `step(PreparingToStop, ctx, Internal(AssembliesStopped))` → `Off` |
| `test_external_events_are_no_ops_during_preparing_to_start` | `step(PreparingToStart, ctx, UpdateRpm(3000))` → `next_state == PreparingToStart` |
| `test_external_events_are_no_ops_during_preparing_to_stop` | `step(PreparingToStop, ctx, UpdateAmbientLux(10))` → `next_state == PreparingToStop` |
| `test_start_assemblies_action_emitted_on_power_on` | `output(Off, PowerOn)` → includes `DomainAction::StartAssemblies` |
| `test_stop_assemblies_action_emitted_on_power_off` | `output(Idle, PowerOff)` → includes `DomainAction::StopAssemblies` |

**Existing tests that will break** (update them, do not delete):
- `fsm_engine_contract.rs`: `test_transition_illegal_shutdown_attempt` — expects `PowerOn → Idle` directly; update to assert `→ PreparingToStart`
- `fsm_properties.rs`: `test_power_off_only_valid_from_idle` — must chain through `PreparingToStop`
- `scenarios_smoke.rs`: `scenario_power_on_then_drive_rpm_enters_driving` — prefix with `Internal(AssembliesReady)` step

**Code changes:**

- [`crates/common/src/fsm/machineries.rs`](crates/common/src/fsm/machineries.rs):
  - Add `FsmState::PreparingToStart` and `FsmState::PreparingToStop`
  - Add `Operational::AssembliesReady` and `Operational::AssembliesStopped` to the `Internal(Operational)` arm
  - Add `DomainAction::StartAssemblies` and `DomainAction::StopAssemblies`
  - Delete `DomainAction::EnterMode` and its associated `ActorModeHintFromDomain` / `ActorMode` types (these are still referenced in `virtual_car_actor.rs` — stub out the reference with `let _ =` until Phase 6 cleans them up)

- [`crates/common/src/fsm/transition_map.rs`](crates/common/src/fsm/transition_map.rs):
  - `Off + PowerOn` → `PreparingToStart` (was `→ Idle`)
  - `PreparingToStart + Internal(AssembliesReady)` → `Idle`
  - `Idle + PowerOff` → `PreparingToStop` (was `→ Off`)
  - `PreparingToStop + Internal(AssembliesStopped)` → `Off`
  - External events in `PreparingToStart` / `PreparingToStop` → self-loop, no actions

### Discussion checkpoint after Phase 1

Verify before proceeding:
1. All 8 new tests green; all prior tests updated and green.
2. `cargo test -p common` passes with zero warnings on the changed files.
3. Confirm: does `DomainAction::EnterMode` still compile (with the stub `let _ =`)? If yes, safe to continue.
4. Decision: are the `Operational::AssembliesReady/Stopped` variants a good fit, or should they live as top-level `FsmEvent::Internal` variants at the same level as `LightingUnsafe`? Agree before Phase 2 starts.

---

## Phase 2 — Assembly Startup/Shutdown Vocabulary: `BecomeOn`, `BecomeOff`, `ZoneId`

**Scope:** `crates/common/src/vehicle_state/front_headlamp.rs`, `crates/common/src/twin_runtime/headlamp_actor.rs`, new `ZoneId` type.

### RED tests (new file: `test/headlamp_lifecycle_contract.rs`)

| Test | Asserts |
|------|---------|
| `test_become_on_message_transitions_headlamp_to_starting` | `HeadlampActor` receives `BecomeOn`; sends back `ZoneReady { zone_id: ZoneId::Headlamp, reply: HeadlampZoneReply { state: Starting } }` |
| `test_become_off_message_transitions_headlamp_to_off` | Headlamp in any state receives `BecomeOff`; replies with `state: Off` |
| `test_zone_id_headlamp_is_constructible` | `ZoneId::Headlamp` compiles and `Debug`-prints |
| `test_headlamp_actor_reply_carries_zone_id` | Generic `ZoneReady` carries `zone_id: ZoneId::Headlamp` |

**Code changes:**

- [`crates/common/src/vehicle_state/front_headlamp.rs`](crates/common/src/vehicle_state/front_headlamp.rs):
  - Add `HeadlampMessage::BecomeOn` and `HeadlampMessage::BecomeOff`

- [`crates/common/src/twin_runtime/headlamp_actor.rs`](crates/common/src/twin_runtime/headlamp_actor.rs):
  - Handle `BecomeOn`: transition internal state to a "Starting" or "BecameOn" state (exact name TBD at checkpoint), reply with `ZoneReady { zone_id: ZoneId::Headlamp, ... }`
  - Handle `BecomeOff`: reset state to `Off`, reply with `ZoneReady`
  - `tell_headlamp_zone` now accepts `ZoneId` in its reply path

- New type `ZoneId` in `crates/common/src/twin_runtime/mod.rs` (or a new `zone.rs`):
  ```rust
  pub enum ZoneId { Headlamp }
  ```

### Discussion checkpoint after Phase 2

1. All new tests green; `cargo test -p common` clean.
2. Confirm the exact `HeadlampState` variant that `BecomeOn` transitions to — is it a new `Starting` variant or does it reuse `OnRequested`? This decision propagates to Phase 5.
3. Confirm: `ZoneReply` is still `HeadlampZoneReply` at this point (the generic `ZoneReply` enum comes in Phase 3). Is the `ZoneReady` message in `DigitalTwinCarVocabulary` still the old `HeadlampZoneReady`? Yes — Phase 3 will replace it. Both phases touch different layers; no conflict.

---

## Phase 3 — Generic Zone Envelope in `DigitalTwinCarVocabulary`

**Scope:** `crates/common/src/digital_twin/mod.rs`, `crates/common/src/twin_runtime/controller/virtual_car_actor.rs`.

### RED tests (new test functions in `test/actor_contract.rs`)

| Test | Asserts |
|------|---------|
| `test_zone_ready_message_routes_to_on_zone_ready_handler` | Brain `handle()` receives `ZoneReady { zone_id: Headlamp, ... }` without panicking |
| `test_zone_tell_back_timeout_message_carries_zone_id` | `ZoneTellBackTimeout { zone_id: Headlamp, turn_id, tell_attempt }` is handled |
| `test_handle_has_exactly_four_arms_compilation_check` | Compile-time: the match in `handle()` has exactly `Fsm / ZoneReady / ZoneTellBackTimeout / GetStatus` — enforced by exhaustiveness |

**Code changes:**

- [`crates/common/src/digital_twin/mod.rs`](crates/common/src/digital_twin/mod.rs):
  - Add `ZoneReply` enum: `Headlamp(HeadlampZoneReply)`
  - Replace `HeadlampZoneReady { turn_id, tell_attempt, reply: HeadlampZoneReply }` with `ZoneReady { zone_id: ZoneId, turn_id: u64, tell_attempt: u32, reply: ZoneReply }`
  - Replace `TellBackTimeout { turn_id, tell_attempt }` with `ZoneTellBackTimeout { zone_id: ZoneId, turn_id: u64, tell_attempt: u32 }`
  - Replace `HeadlampZoneSpontaneous { ... }` with `ZoneSpontaneous { zone_id: ZoneId, event: ZoneSpontaneousEvent }`

- [`crates/common/src/twin_runtime/controller/virtual_car_actor.rs`](crates/common/src/twin_runtime/controller/virtual_car_actor.rs):
  - Update `handle()` match arms to use the new vocabulary; dispatch to renamed handlers `on_zone_ready(zone_id, turn_id, ...)`, `on_zone_timeout(zone_id, ...)`, `on_zone_spontaneous(zone_id, ...)`
  - `on_zone_ready` unpacks `ZoneReply::Headlamp(r)` before calling the existing headlamp logic (behavior unchanged in this phase)

- [`crates/common/src/twin_runtime/headlamp_actor.rs`](crates/common/src/twin_runtime/headlamp_actor.rs): Update reply sends to use `ZoneReady { zone_id: ZoneId::Headlamp, reply: ZoneReply::Headlamp(...) }`

**No behavior change in this phase** — `pending_turn` still gates everything. The four-arm `handle()` is structural scaffolding; the internal routing still delegates to the same functions as before.

### Discussion checkpoint after Phase 3

1. All tests green. The existing `actor_contract`, `quiescence_actor_contract`, `zone_tell_back_contract` pass unmodified.
2. Confirm `ZoneSpontaneousEvent` shape — what fields does it carry to replace the old `{ direction, cause }` pair? Agree before Phase 4 so Phase 4 can reference the final type.
3. Are there any remaining references to `HeadlampZoneReady` outside the changed files? `cargo grep HeadlampZoneReady` must return zero.

---

## Phase 4 — `VecDeque<TurnBarrier>` Replaces `pending_turn`

**Scope:** New `TurnBarrier` struct, `virtual_car_actor.rs` (actor state), drain loop.

### RED tests (new file: `test/turn_barrier_contract.rs`)

| Test | Asserts |
|------|---------|
| `test_two_zone_directed_events_committed_in_arrival_order` | Inject `UpdateAmbientLux(20)` then `UpdateAmbientLux(100)` via Brain actor. Zone replies arrive in reverse order. Ledger sequence matches event arrival order, not reply order. |
| `test_backlogged_event_committed_after_barrier_drains` | Inject zone-directed event then non-zone event. Non-zone event commits after the zone reply arrives, with `old_ctx.headlamp` reflecting the committed lux event. |
| `test_zone_tell_back_timeout_retries_per_zone_not_per_turn` | Synthetic timeout for zone A does not cancel a pending wait for zone B on the same turn. |
| `test_drain_loop_stops_at_first_incomplete_barrier` | Two barriers; only B's zone replies arrive. Only A drains (because A is front and has all replies); B stays in queue. |

**Code changes:**

- New struct in `crates/common/src/twin_runtime/zone_tell_back.rs` (or a new `turn_barrier.rs`):
  ```rust
  pub(crate) struct TurnBarrier {
      pub turn_id:     u64,
      pub event:       FsmEvent,
      pub now:         Instant,
      pub pending:     BTreeSet<ZoneId>,
      pub zone_waits:  HashMap<ZoneId, TellBackWait>,
      pub zone_timers: HashMap<ZoneId, TellBackTimer>,
      pub replies:     HashMap<ZoneId, ZoneReply>,
  }
  ```
  `TellBackWait` is **unchanged** — it becomes one value per zone inside the map.

- [`crates/common/src/twin_runtime/controller/virtual_car_actor.rs`](crates/common/src/twin_runtime/controller/virtual_car_actor.rs):
  - Remove `pending_turn: Option<PendingBrainTurn>` and `fsm_backlog: VecDeque<(FsmEvent, Instant)>` from `VirtualCarRuntimeState`
  - Add `barrier_queue: VecDeque<TurnBarrier>`
  - `begin_fsm_turn`: if a zone message is needed, push a new `TurnBarrier` with `pending = {ZoneId::Headlamp}`, send the tell, arm the per-zone timer; else if no zone needed, push a barrier with `pending = {}` → immediately drainable
  - Drain loop (`try_drain_barrier_queue`):
    ```
    while let Some(front) = barrier_queue.front() {
        if !front.pending.is_empty() { break; }
        let committed = barrier_queue.pop_front().unwrap();
        commit_resolved_turn(committed.event, committed.now, committed.replies);
    }
    ```
  - Remove `PendingBrainTurn` entirely; remove `pump_fsm_backlog`
  - `on_zone_ready`: locate barrier by `turn_id`, move `zone_id` from `pending` to `replies`, run drain loop
  - `on_zone_timeout`: locate barrier, call per-zone retry or synthetic-reply logic (reusing `TellBackWait`)

### Discussion checkpoint after Phase 4

1. All tests green. Specifically: `quiescence_actor_contract`, `zone_tell_back_contract`, `actor_contract` all pass.
2. The new ordering tests in `turn_barrier_contract.rs` pass — inspect the ledger sequences manually in the test output to confirm the `old_ctx` values.
3. The `fsm_backlog` drain path is gone. Walk through Case 3 from Section 2 of the design doc mentally (or trace a test) to confirm the ROB pattern handles it correctly.
4. Confirm: `IgnitionOffReset` variant is still present in the code at this point (it will be deleted in Phase 6). No new behavior is attached to it in this phase — it just must not panic. Verify this via `scenario_cold_start_get_status_shows_off` (which exercises PowerOff).

---

## Phase 5 — Wire `apply_committed_quiescence` to Startup/Shutdown Barriers

**Scope:** See `brain_fsm_redesign_impl_Phase_5.md` for the full design and implementation plan.
This entry is a high-level summary only; the detail document is authoritative.

**Design settled (2026-06-22):**

- `FsmEvent::AssemblyZoneReady(ZoneId)` is a proper **external** event (not `Internal`).
- `VehicleContext.pending_assemblies: BTreeSet<ZoneId>` tracks remaining assemblies in the FSM layer.
- Both `Off → PreparingToStart` and `Idle → PreparingToStop` initialise `pending_assemblies`.
- The `PreparingToStart + Internal(AssembliesReady)` and `PreparingToStop + Internal(AssembliesStopped)` transition arms are **deleted** (become dead code after Phase 5).
- `TurnBarrier::new_passthrough` is deleted; `PassthroughBarrier` type introduced.
- `TurnBarrier::new_for_assembly_zone(turn_id, zone_id, msg, wait, timer, now)` added.
- `fsm_event_headlamp_message` renamed to `user_event_to_headlamp_tell` (interim; superseded in Phase 6).
- `fsm_preparation_contract.rs` must be updated: replace `Internal(AssembliesReady/Stopped)` with `AssemblyZoneReady(Headlamp)`.

### Discussion checkpoint after Phase 5

1. Run `cargo test -p common` — full suite green; 4 new `startup_barrier_contract` tests green.
2. Walk through the cold-start sequence: `PowerOn → PreparingToStart → (BecomeOn) → Idle → drive → PowerOff → PreparingToStop → (BecomeOff) → Off`.
3. The `IgnitionOffReset` path in `on_zone_ready` is now dead. Add `unreachable!()` to that arm and run the full suite — it must pass. This is the gate for Phase 6.
4. Sign off on the `MANAGED_ASSEMBLIES` constant design before Phase 8 replaces it.

---

## Phase 6 — State-Aware Zone Routing; Delete Speculative Execution

**Scope:** `zone_turn.rs`, `twin_turn.rs`, `zone_replies.rs`, `virtual_car_actor.rs`, `machineries.rs`.

### RED tests (new functions in `test/fsm_preparation_contract.rs` and `test/zone_replies_contract.rs`)

| Test | Asserts |
|------|---------|
| `test_zone_message_for_event_returns_none_during_preparing_to_start` | `zone_message_for_event(UpdateAmbientLux(10), PreparingToStart)` → `None` |
| `test_zone_message_for_event_returns_none_during_preparing_to_stop` | Same for `PreparingToStop` |
| `test_zone_message_for_event_returns_some_during_driving` | `zone_message_for_event(UpdateAmbientLux(10), Driving)` → `Some((ZoneId::Headlamp, AmbientLux(10)))` |
| `test_events_during_preparing_to_start_are_ledgered_applied_false` | Inject `UpdateAmbientLux(10)` while FSM is in `PreparingToStart`; ledger row exists with `applied: false`; FSM stays in `PreparingToStart` |
| `test_power_off_does_not_speculatively_run_zone_turn` | No double-execution: run a `PowerOff` through `twin_turn` and count FSM invocations = 1 |
| `test_zone_replies_simulate_locally_has_no_ignition_off_reset` | `ZoneReplies::simulate_locally()` returns a struct with only `headlamp: None`; `ignition_off_reset` field does not exist |

**Code changes:**

- [`crates/common/src/twin_runtime/zone_turn.rs`](crates/common/src/twin_runtime/zone_turn.rs):
  - Add `pub(crate) fn zone_message_for_event(event: &FsmEvent, state: &FsmState) -> Option<(ZoneId, ZoneMessage)>`
  - The inner state-unaware function is now `user_event_to_headlamp_tell` (renamed from `fsm_event_headlamp_message` in Phase 5); reference it by the new name
  - In `PreparingToStart` / `PreparingToStop` branches, return `None`

- [`crates/common/src/twin_runtime/twin_turn.rs`](crates/common/src/twin_runtime/twin_turn.rs):
  - Delete `fsm_step_lands_off`
  - Delete the `IgnitionOffReset` block inside `apply_external_hop` (lines ~184–193 per the design doc)

- [`crates/common/src/twin_runtime/zone_replies.rs`](crates/common/src/twin_runtime/zone_replies.rs):
  - Delete `HeadlampReplies.ignition_off_reset` field
  - Simplify `ZoneReplies` to: `pub struct ZoneReplies { pub headlamp: Option<HeadlampZoneReply> }`
  - Delete `with_headlamp(ingress, ignition_off_reset)` constructor; keep `with_headlamp_ingress` and `simulate_locally`

- [`crates/common/src/twin_runtime/controller/virtual_car_actor.rs`](crates/common/src/twin_runtime/controller/virtual_car_actor.rs):
  - Delete `PendingBrainTurn::IgnitionOffReset` (now confirmed unreachable from Phase 5 checkpoint)
  - Delete `ActorMode`, `ActorModeHintFromDomain`
  - Delete `initial_headlamp_ctx` option from `VehicleControllerRuntimeOptions` (deferred from Phase 5: all tests now boot through the `BecomeOn` automatic flow)

- [`crates/common/src/fsm/machineries.rs`](crates/common/src/fsm/machineries.rs):
  - Delete `DomainAction::EnterMode` (the `let _ =` stub is gone)
  - Decide and act: remove `Operational::AssembliesReady` and `Operational::AssembliesStopped` enum variants (their transition arms were deleted in Phase 5; only doc-comment references remain — remove those too, or keep as tombstones per team agreement)

**Complete deletion checklist** (from Gap 3 table in design doc):

| Item deleted | File |
|---|---|
| `apply_external_hop` ignition-off block | `twin_turn.rs` |
| `HeadlampReplies.ignition_off_reset` field | `zone_replies.rs` |
| `ZoneReplies::with_headlamp(ingress, ignition_off_reset)` second argument | `zone_replies.rs`, call sites |
| `PendingBrainTurn::IgnitionOffReset` variant | `virtual_car_actor.rs` |
| `fsm_step_lands_off` function | `twin_turn.rs` |
| `DomainAction::EnterMode` + `ActorMode` + `ActorModeHintFromDomain` | `machineries.rs`, `virtual_car_actor.rs` |

### Discussion checkpoint after Phase 6

1. `cargo test -p common` and `cargo test -p gateway` — full suite green.
2. Walk through the deletion checklist above: confirm every item is gone. `cargo build -p common` must emit zero dead-code warnings.
3. `begin_fsm_turn` now has exactly **two** decision branches: zone-message → barrier wait; no zone-message → passthrough (drains immediately). Trace Case 1 and Case 2 from Section 2 of the design doc through the new code path.
4. **HOB invariant confirmed:** after state-aware zone routing, `UpdateAmbientLux` in `PreparingToStart` becomes a passthrough barrier. Confirm the HOB drain loop is NOT relaxed to let passthrough barriers skip ahead of zone-directed ones — they drain in arrival order and trivially fast. No bypass is needed or allowed (would violate ledger ordering).
5. **Architecture review**: the intermediate design for a single assembly is complete. The system is correct, deterministic, and `handle()` has four arms. Run the property tests (`cargo test -p common --features proptest`) and the e2e tests together before introducing the second assembly.

---

## Phase 7 — Wiper as Second Assembly

**Scope:** New `WiperActor`, `zone_turn.rs` routing, `virtual_car_actor.rs` startup barrier, `MANAGED_ASSEMBLIES`.

### RED tests (new file: `test/wiper_zone_contract.rs`)

| Test | Asserts |
|------|---------|
| `test_wiper_zone_id_exists_and_is_distinct_from_headlamp` | `ZoneId::Wiper != ZoneId::Headlamp` |
| `test_update_windshield_rain_routes_to_wiper_zone` | `zone_message_for_event(UpdateWindshieldRain(Heavy), Driving)` → `Some((ZoneId::Wiper, WiperMessage::Rain(Heavy)))` |
| `test_concurrent_headlamp_and_wiper_events_commit_in_arrival_order` | Two zone-directed events to different assemblies; replies arrive in reverse order; ledger sequence = arrival order; `old_ctx` values are accurate |
| `test_wiper_included_in_startup_barrier` | After `PowerOn`, both `BecomeOn` tells are sent (headlamp and wiper) before `Internal(AssembliesReady)` |
| `test_slow_wiper_does_not_delay_headlamp_event_commit` | Headlamp barrier completes; headlamp event commits even if wiper barrier is still pending (different turns) |

**Code changes:**

- New `FsmEvent::UpdateWindshieldRain(RainIntensity)` in `crates/common/src/fsm/machineries.rs`
- New `WiperMessage`, `WiperZoneReply`, `WiperActor` (parallel structure to `headlamp_actor.rs`) in `crates/common/src/twin_runtime/wiper_actor.rs`
- `ZoneId`: add `Wiper`; `ZoneReply`: add `Wiper(WiperZoneReply)`
- `zone_turn.rs`: add wiper routing in `zone_message_for_event`
- `virtual_car_actor.rs`: update `MANAGED_ASSEMBLIES` to include `ZoneId::Wiper`; wire wiper actor reference in `VirtualCarRuntimeState`; `on_zone_ready` dispatches `ZoneReply::Wiper(...)` to wiper-specific handler
- `turn_barrier.rs`: generalise `new_for_assembly_zone`'s `message` parameter from `HeadlampMessage` to a generic zone lifecycle message type (e.g. `ZoneLifecycleMessage` enum). The `HeadlampMessage` narrowing introduced in Phase 5 must not survive Phase 7.
- [`crates/common/src/twin_runtime/zone_replies.rs`](crates/common/src/twin_runtime/zone_replies.rs): migrate to the Phase 7 map-based shape:
  ```rust
  pub struct ZoneReplies { pub replies: HashMap<ZoneId, ZoneReply> }
  impl ZoneReplies {
      pub fn simulate_locally() -> Self { Self { replies: HashMap::new() } }
      pub fn get(&self, id: ZoneId) -> Option<&ZoneReply> { self.replies.get(&id) }
  }
  ```

### Discussion checkpoint after Phase 7

1. Full test suite green including new wiper tests.
2. The multi-assembly ordering invariant holds (verified by `test_concurrent_headlamp_and_wiper_events_commit_in_arrival_order`). Examine the ledger sequence in that test output to confirm `old_ctx` values.
3. Confirm: `handle()` still has exactly four arms; adding Wiper required zero new arms in `handle()`.
4. This phase validates the intermediate architecture (Sections 7 + 8 of the design doc). Before Phase 8, agree on the exact Rust syntax for embedding `&'static [AssemblyId]` in the FSM state variants — `const` generic vs. `&'static` slice vs. another approach.

---

## Phase 8 — FSM State Embeds Assembly IDs (Section 10 Target Design)

**Scope:** `machineries.rs`, `transition_map.rs`, `virtual_car_actor.rs` (`apply_committed_quiescence`).

### RED tests (new functions in `test/fsm_preparation_contract.rs`)

| Test | Asserts |
|------|---------|
| `test_preparing_to_start_state_carries_assembly_ids` | `FsmState::PreparingToStart` value can be pattern-matched to extract `assemblies: &[AssemblyId]`; the slice contains `AssemblyId::Headlamp` and `AssemblyId::Wiper` |
| `test_preparing_to_stop_state_carries_assembly_ids` | Same for `PreparingToStop` |
| `test_barrier_pending_set_derived_from_fsm_state_not_actor_constant` | Startup barrier's `pending` set equals what the FSM state declares; `MANAGED_ASSEMBLIES` constant no longer exists |
| `test_all_phase_1_through_7_tests_remain_green` | Implicit — run full suite |

**Code changes:**

- [`crates/common/src/fsm/machineries.rs`](crates/common/src/fsm/machineries.rs):
  ```rust
  // illustrative — exact syntax agreed at Phase 7 checkpoint
  FsmState::PreparingToStart { assemblies: &'static [AssemblyId] }
  FsmState::PreparingToStop  { assemblies: &'static [AssemblyId] }
  ```

- [`crates/common/src/fsm/transition_map.rs`](crates/common/src/fsm/transition_map.rs): constructs these variants with a compile-time constant slice `&[AssemblyId::Headlamp, AssemblyId::Wiper]`.

- [`crates/common/src/twin_runtime/controller/virtual_car_actor.rs`](crates/common/src/twin_runtime/controller/virtual_car_actor.rs):
  - Delete `MANAGED_ASSEMBLIES` constant
  - In `apply_committed_quiescence`, when matching `DomainAction::StartAssemblies`, read `assemblies` from the current FSM state (`FsmState::PreparingToStart { assemblies }`) to build the set of zone barriers
  - Actor no longer holds a parallel copy of the coordination topology

**Note on `ctx.pending_assemblies`:** this field is NOT removed in Phase 8.
`FsmState::PreparingToStart { assemblies }` carries the **initial topology** (which assemblies to start).
`ctx.pending_assemblies` carries the **countdown** (which assemblies have not yet replied).
They serve different purposes: the state variant answers "what must be started?"; the context answers "what is still waiting?".
Phase 8 eliminates `MANAGED_ASSEMBLIES` from the actor, not `pending_assemblies` from the context.

### Discussion checkpoint after Phase 8

1. `cargo test -p common --features proptest && cargo test -p gateway && cargo test -p vehicle_device_bus` — full suite across all crates green.
2. Walk through the Design Summary table from Section 11 of `findings/brain_fsm_turn_explanation.md` row by row and confirm each "Target" column is satisfied.
3. `MANAGED_ASSEMBLIES` no longer exists anywhere in the codebase. The FSM is the single queryable source of which assemblies a Digital Twin manages.
4. Final architecture review: the `VecDeque<TurnBarrier>` drain loop, `zone_message_for_event` routing, and `handle()` four-arm structure are all unchanged from their Phase 4 / Phase 6 form. Phase 8 touched only the source of the `pending` set for startup/shutdown barriers.

---

## Phase 9 — FSM State Owns the Countdown (`BTreeSet` replaces `remaining_assemblies`)

**Status: COMPLETED (2026-06-23, same session as Phase 8).**  
**Depends on:** Phase 8 complete (`AssemblyId` rename; `ALL_ASSEMBLIES`; struct variants; 179 tests green).  
**Implemented as a Phase 8 refinement:** During Phase 8 review, a "temporal mismatch" was identified in
`transition_map.rs` — `transition()` had to peek ahead into a future value of
`ctx.remaining_assemblies` because the actual mutation happened later in `step.rs`.  The fix
merged the countdown directly into the FSM state.

### What Phase 9 changes

| Before (Phase 8 design) | After (Phase 9 — final state) |
|---|---|
| `PreparingToStart { assemblies: &'static [AssemblyId] }` — static slice, reset to `ALL_ASSEMBLIES` on every self-loop | `PreparingToStart(BTreeSet<AssemblyId>)` — owned shrinking set; each `AssemblyZoneReady` removes one entry |
| `VehicleContext::remaining_assemblies: BTreeSet<AssemblyId>` — parallel countdown in context | **Deleted** — the state's inner `BTreeSet` is the sole authoritative countdown |
| `step.rs` mutation block: reads from `DomainAction::StartAssemblies` to init `ctx.remaining_assemblies`; `AssemblyZoneReady` removes from `ctx` | **Deleted** — `step.rs` performs zero countdown mutations |
| `transition()` peek-ahead: subtracts 1 from `ctx.remaining_assemblies.len()` to decide next state | **Deleted** — reads `remaining.is_empty()` directly from the state's own `BTreeSet` |
| `DomainAction::StartAssemblies(&'static [AssemblyId])` | `DomainAction::StartAssemblies(Vec<AssemblyId>)` — owned, not a static borrow |
| `output()` catch-all `if old != new` fired on every intra-mode step | Explicit `(PreparingToStart(_), PreparingToStart(_)) => vec![]` guard added before catch-all |

### Key design rationale

`FsmState` is now the complete machine state — no external bookkeeping is needed.
`transition()` is a fully self-contained pure function: it reads the remaining set from the
current state, produces a filtered copy as the next state, and checks `is_empty()` to decide
whether to cross to `Idle` / `Off`.  `VehicleContext` carries only assembly-domain state
(sensors, actuators) — no FSM lifecycle bookkeeping.

### RED tests written / updated

All tests in `fsm_preparation_contract.rs` and `fsm_engine_contract.rs` that previously
asserted `ctx.remaining_assemblies` were rewritten to assert the inner `BTreeSet` of the
`PreparingToStart` / `PreparingToStop` state variant directly.  Five new Phase 9 tests added:

| Test | Asserts |
|---|---|
| `test_preparing_to_start_carries_assembly_ids` | `PreparingToStart(remaining)` contains both `Headlamp` and `Wiper` after `PowerOn` |
| `test_preparing_to_stop_carries_assembly_ids` | Same for `PreparingToStop` after `PowerOff` |
| `test_state_and_action_agree_on_assembly_set` | The state's inner set and `StartAssemblies` action payload name the same assemblies |
| `test_assembly_zone_ready_shrinks_state_not_context` | After Headlamp acks, state is `PreparingToStart({Wiper})`; `VehicleContext` untouched |
| `test_start_assemblies_action_carries_assembly_list` | `StartAssemblies(Vec<AssemblyId>)` payload contains both assemblies |

### Files changed

| File | Change |
|---|---|
| `crates/common/src/fsm/machineries.rs` | Variant type changed to `(BTreeSet<AssemblyId>)`; action payload changed to `Vec<AssemblyId>`; `BTreeSet` import added |
| `crates/common/src/fsm/transition_map.rs` | `transition()` rewrites: filter BTreeSet from state; `output()` intra-mode guards added |
| `crates/common/src/fsm/step.rs` | Mutation block deleted; `let mut modified_ctx` → `let modified_ctx` |
| `crates/common/src/vehicle_state/mod.rs` | `remaining_assemblies` field deleted; `BTreeSet` import removed |
| `crates/common/src/twin_runtime/zone_turn.rs` | Pattern updated: `PreparingToStart { .. }` → `PreparingToStart(_)` |
| `crates/common/src/test/fsm_preparation_contract.rs` | Full rewrite: struct-variant assertions → tuple-variant; `ctx.remaining_assemblies` → state inner set |
| `crates/common/src/test/fsm_engine_contract.rs` | `ctx.remaining_assemblies` blocks replaced with direct state construction |
| `crates/common/src/test/fsm_step_contract.rs` | Struct-variant pattern in RED test fixed |
| `crates/common/src/test/wiper_zone_contract.rs` | Struct-variant pattern fixed |

### Discussion checkpoint after Phase 9

1. `cargo test` — 179 tests green, zero warnings across all crates. ✅
2. `rg "remaining_assemblies" --type rust` → zero. ✅
3. `rg "PreparingToStart {" --type rust` → zero (all usages are tuple-variant). ✅
4. Confirm: `transition()` has no reference to `VehicleContext` fields other than `is_healthy()`,
   `powertrain`, and `visibility` — it reads all countdown state from `FsmState` directly.
5. **Data structure revisit (deferred):** If the FSM is ever ported to a bare-metal ECU
   (no allocator), `BTreeSet<AssemblyId>` should be replaced with
   `arrayvec::ArrayVec<AssemblyId, MAX_ASSEMBLIES>`.  That is a two-line change in
   `machineries.rs`; no logic elsewhere changes.  See `brain_fsm_redesign_impl_Phase_8.md`
   "Revisit" section for the full trade-off analysis.
6. **Documentation debt:** `brain_fsm_redesign_impl_Phase_8.md` was written as a plan and
   describes the superseded `&'static [AssemblyId]` design.  Before README / blog compilation
   it must be updated to reflect the final Phase 9 state.

---

## Phase 10 — Documentation Consolidation and Remaining simulation-4 Objectives

**Status: PENDING (next iteration).**  
**Depends on:** Phases 1–9 complete; 179 tests green; all deprecated symbols zero.

### Scope

Phase 10 is not a single architectural change but a collection of deferred items that are
independent of each other and can be done in any order.  Each item is small enough to be
a standalone PR.

### Item A — Update `brain_fsm_redesign_impl_Phase_8.md` *(documentation)*

The Phase 8 implementation doc was written as a plan and still contains the superseded
`&'static [AssemblyId]` design as if it were the final state.  Update every code snippet
and description to match what was actually built (the intermediate design), and cross-link
clearly to Phase 9 as the resolution.

**Done:** Header and Discussion checkpoint updated in the commit that introduced Phase 9.
Remaining: update code snippets inside D2, D4, Step 3, Step 5 to note they are intermediate.

---

### Item B — README and blog compilation *(documentation)*

Compile the staged design story (Phases 1–9) into:
- `README.md` — architecture overview, FSM state machine diagram, assembly topology
- `blog/draft.md` — narrative walkthrough of the redesign journey

Source material: `brain_fsm_redesign_plan.md`, `brain_fsm_redesign_impl_Phase_{2..9}.md`,
`findings/`, `diagrams/`.

---

### Item C — CAN emulation for `PowerOn` / `PowerOff` *(feature)*

CAN frame ID `0x100`:
- Payload `01 00 00 00 00 00 00 00` → `FsmEvent::PowerOn`
- Payload `00 00 00 00 00 00 00 00` → `FsmEvent::PowerOff`

A `can_emulator` module (or actor) that maps raw frames to FSM events and forwards them to
`VirtualCarActor`.  See `analysis_4_response.md` Stage 1 for the original design sketch.

---

### Item D — Non-blocking actuation *(refactor)*

`actuation_manager.execute()` is currently `.await`-ed directly inside `virtual_car_actor.rs`,
holding the actor's thread during CAN transmission.  Move actuation into the `HeadlampActor`'s
own thread so `VirtualCarActor` does not block.

See `findings/single-thread-guarantee.md` Category 2 and `analysis_4_response.md` Stage 5.

---

### Item E — Code commenting pass *(quality)*

Systematic doc-comment pass over the core call tree:
`begin_fsm_turn` → `zone_message_for_event` → `TurnBarrier` drain loop → `apply_committed_quiescence`.
Each public and `pub(crate)` function must have a doc comment explaining its invariants and
relationship to adjacent functions.

See `findings/Code-commenting-plan.md`.

---

### Item F — Actor-level fuzz / steady-state tests *(quality)*

`fsm_properties.rs` covers FSM-level random event sequences via `proptest`.
Missing: actor-level tests that spawn `VirtualCarActor`, fire random `ractor` messages
(including `Fsm(...)`, `ZoneReady`, `ZoneTellBackTimeout`), and assert:
- The actor never panics.
- `barrier_queue` drains to empty within a bounded number of events.
- The final FSM state is a valid steady state (`Off`, `Idle`, `Driving`, etc.).

See `analysis_4_response.md` Stage 6.

---

### Item G — Embedded readiness: `ArrayVec` migration *(optional)*

If the FSM is ported to a bare-metal ECU (no allocator), replace `BTreeSet<AssemblyId>`
in `FsmState::PreparingToStart` / `PreparingToStop` with
`arrayvec::ArrayVec<AssemblyId, MAX_ASSEMBLIES>`.  This is a two-line change in
`machineries.rs`; no logic elsewhere changes.

See `brain_fsm_redesign_impl_Phase_8.md` "Revisit" section.

### Discussion checkpoint after Phase 10

1. `README.md` accurately describes the Phase 9 final design.
2. `cargo test` still 179+ tests green (no regressions from Item D refactor).
3. `rg "actuation_manager.execute" --type rust` shows zero calls in `virtual_car_actor.rs`.
4. `cargo doc --no-deps -p common` emits zero "missing documentation" warnings for `pub` items.

---

## File Change Summary by Phase

| Phase | Files Changed | Files Added |
|-------|--------------|-------------|
| 1 | `fsm/machineries.rs`, `fsm/transition_map.rs` | `test/fsm_preparation_contract.rs` |
| 2 | `vehicle_state/front_headlamp.rs`, `twin_runtime/headlamp_actor.rs` | `test/headlamp_lifecycle_contract.rs`, `twin_runtime/zone.rs` |
| 3 | `digital_twin/mod.rs`, `twin_runtime/controller/virtual_car_actor.rs`, `twin_runtime/headlamp_actor.rs` | — |
| 4 | `twin_runtime/controller/virtual_car_actor.rs`, `twin_runtime/zone_tell_back.rs` | `test/turn_barrier_contract.rs`, optionally `twin_runtime/turn_barrier.rs` |
| 5 | `twin_runtime/controller/virtual_car_actor.rs` | — |
| 6 | `twin_runtime/zone_turn.rs`, `twin_runtime/twin_turn.rs`, `twin_runtime/zone_replies.rs`, `twin_runtime/controller/virtual_car_actor.rs`, `fsm/machineries.rs` | — |
| 7 | `fsm/machineries.rs`, `twin_runtime/zone_turn.rs`, `twin_runtime/zone_replies.rs`, `twin_runtime/controller/virtual_car_actor.rs` | `twin_runtime/wiper_actor.rs`, `test/wiper_zone_contract.rs` |
| 8 | `fsm/machineries.rs`, `fsm/transition_map.rs`, `twin_runtime/controller/virtual_car_actor.rs` | — |
| 9 | `fsm/machineries.rs`, `fsm/transition_map.rs`, `fsm/step.rs`, `vehicle_state/mod.rs`, `twin_runtime/zone_turn.rs`, `test/fsm_preparation_contract.rs`, `test/fsm_engine_contract.rs`, `test/fsm_step_contract.rs`, `test/wiper_zone_contract.rs` | — |
| 10 | `README.md`, `virtual_car_actor.rs` (actuation), new `can_emulator` module | `brain_fsm_redesign_impl_Phase_10.md` |

All paths are relative to `crates/common/src/`.
