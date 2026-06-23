# Brain FSM Redesign — Phase 8 Implementation
## FSM State Embeds Assembly IDs; `MANAGED_ASSEMBLIES` Deleted

**Status:** COMPLETED (2026-06-23). Implemented as an intermediate design; immediately superseded
by Phase 9 in the same session. The Phase 8 design (`&'static [AssemblyId]` struct variants +
`VehicleContext::remaining_assemblies`) is the correct description of what was built and then
refined — it is not the final codebase state. See `brain_fsm_redesign_impl_Phase_9.md` for the
final design.  
**Depends on:** Phase 7 complete (`ZoneId::Wiper`; `WiperActor`; `MANAGED_ASSEMBLIES = &[Headlamp, Wiper]`; 158 tests green).  
**Next phase:** Phase 9 — BTreeSet countdown; `remaining_assemblies` deleted (same session).

---

## What Phase 8 delivers

After Phase 7, the coordinator topology (`&[Headlamp, Wiper]`) exists in **two places**:

| Location | Role |
|---|---|
| `const MANAGED_ASSEMBLIES` in `virtual_car_actor.rs` | Actor: which zones to tell `BecomeOn`/`BecomeOff` |
| `step.rs` hardcoded `BTreeSet::from([Headlamp, Wiper])` | FSM: initialises the `pending_assemblies` countdown |

The comment in `step.rs` already flags the fragility: `// Keep in sync with MANAGED_ASSEMBLIES`.

Phase 8 makes the FSM the **single authoritative source** of coordinator topology.
It does four things:

1. **`AssemblyId` rename** — `ZoneId` becomes `AssemblyId` everywhere in the codebase.
2. **`FsmState` embedding** — `PreparingToStart` and `PreparingToStop` become struct variants
   carrying `assemblies: &'static [AssemblyId]`.
3. **`DomainAction` data** — `StartAssemblies` and `StopAssemblies` gain a
   `&'static [AssemblyId]` payload; `step.rs` reads from the action to initialise
   `ctx.remaining_assemblies`; `virtual_car_actor.rs` reads from the action to build barriers.
4. **Rename `pending_assemblies`** — `VehicleContext::pending_assemblies` becomes
   `remaining_assemblies` to make the countdown semantics explicit.

Adding a third assembly after Phase 8 requires editing **one place only**: `ALL_ASSEMBLIES`
in `machineries.rs`.

---

## Resolved design decisions

### D1 — Full rename: `ZoneId` → `AssemblyId`

`ZoneId` is defined in `crates/common/src/fsm/machineries.rs` and used throughout the
codebase.  It is renamed to `AssemblyId` everywhere — no type alias, no shim.
The rename is compiler-guided: change the definition, fix every compile error.

```rust
// crates/common/src/fsm/machineries.rs  (rename)
pub enum AssemblyId {   // was: ZoneId
    Headlamp,
    Wiper,
}
```

All call sites that reference `ZoneId` (twin_runtime, fsm, tests, gateway) update to
`AssemblyId`.

---

### D2 — Payload syntax: `&'static [AssemblyId]`

All assembly lists are compile-time constants; a static slice is zero-allocation, stored in
`.rodata`, and compared element-by-element by `PartialEq`.

```rust
// crates/common/src/fsm/machineries.rs
pub enum FsmState {
    Off,
    PreparingToStart { assemblies: &'static [AssemblyId] },   // was: unit variant
    Idle,
    Driving,
    // … all other variants unchanged …
    PreparingToStop  { assemblies: &'static [AssemblyId] },   // was: unit variant
}
```

---

### D3 — Constant location: `machineries.rs` (the vocabulary layer)

`machineries.rs` is lower in the dependency order than `transition_map.rs`; the constant
belongs next to the type it feeds.

```rust
// crates/common/src/fsm/machineries.rs  (addition)

/// The compile-time assembly topology.  Every managed zone must be listed here.
/// Adding a new assembly: add its `AssemblyId` variant and append it to this slice.
pub(crate) const ALL_ASSEMBLIES: &[AssemblyId] = &[AssemblyId::Headlamp, AssemblyId::Wiper];
```

`MANAGED_ASSEMBLIES` in `virtual_car_actor.rs` is deleted — no forwarding alias.

---

### D4 — Dual role: state field (query) + action payload (effect)

`FsmState::PreparingToStart { assemblies }` answers **"what does this FSM manage?"** — a
queryable, observable fact about the state the system is in.

`DomainAction::StartAssemblies(assemblies)` answers **"what must the actor do now?"** — a
command carried from the FSM output function to its consumers.

Both carry the same `ALL_ASSEMBLIES` slice from `machineries.rs`.  They have different
readers:

| Reader | Source | Purpose |
|---|---|---|
| `step.rs` | `DomainAction::StartAssemblies(assemblies)` | Initialise `ctx.remaining_assemblies` |
| `virtual_car_actor.rs` | `DomainAction::StartAssemblies(assemblies)` | Build startup barriers |
| Tests / observers | `FsmState::PreparingToStart { assemblies }` | Assert topology |

**`DomainAction` changes:**

```rust
// crates/common/src/fsm/machineries.rs  (modifications)
pub enum DomainAction {
    // … unchanged variants …
    StartAssemblies(&'static [AssemblyId]),   // was: StartAssemblies (unit)
    StopAssemblies(&'static [AssemblyId]),    // was: StopAssemblies  (unit)
    // … unchanged variants …
}
```

**`output()` in `transition_map.rs`:**

```rust
// (Off, PreparingToStart { .. }) arm
(Off, PreparingToStart { .. }) => vec![StartAssemblies(ALL_ASSEMBLIES)],
// (Idle, PreparingToStop { .. }) arm
(Idle, PreparingToStop { .. }) => vec![StopAssemblies(ALL_ASSEMBLIES)],
```

**`step.rs` — read from action, not from event guard:**

```rust
// Replace the two hardcoded BTreeSet::from([...]) blocks with:
for action in &actions {
    match action {
        DomainAction::StartAssemblies(assemblies)
        | DomainAction::StopAssemblies(assemblies) => {
            modified_ctx.remaining_assemblies = assemblies.iter().copied().collect();
        }
        _ => {}
    }
}
// AssemblyZoneReady decrement is unchanged:
if let FsmEvent::AssemblyZoneReady(assembly_id) = event {
    modified_ctx.remaining_assemblies.remove(assembly_id);
}
```

**`virtual_car_actor.rs` — read from action:**

```rust
DomainAction::StartAssemblies(assemblies) => {
    let now = Instant::now();
    let brain = runtime_state.self_ref.clone();
    for &assembly_id in assemblies {            // was: for &zone_id in MANAGED_ASSEMBLIES
        let turn_id = runtime_state.alloc_turn_id();
        let msg = Self::become_on_message_for(assembly_id);
        let wait = TellBackWait::new(turn_id);
        Self::tell_zone(runtime_state, &brain, assembly_id, &msg, turn_id, 0, now)?;
        let timer = Self::arm_tell_back_timer(&brain, assembly_id, turn_id, 0);
        let barrier = TurnBarrier::new_for_assembly_zone(turn_id, assembly_id, msg, wait, timer, now);
        runtime_state.barrier_queue.push_back(BarrierEntry::Waiting(barrier));
    }
}
// Identically for StopAssemblies / PreparingToStop.
```

---

### D5 — `PublishedFsmState`: deferred to implementation time

When the compiler requires a change in `From<&FsmState>` for `PublishedFsmState`, decide
at that point whether `PublishedFsmState::PreparingToStart` stays data-free (flatten) or
mirrors the assembly list.  The default is flatten; override only if observability requires
it.

---

### D6 — Rename `ctx.pending_assemblies` → `ctx.remaining_assemblies`

`remaining_assemblies` makes the countdown semantics explicit and aligns with the new
`AssemblyId` terminology.

```rust
// crates/common/src/vehicle_state/mod.rs
pub struct VehicleContext {
    pub headlamp:              HeadlampContext,
    pub wiper:                 WiperContext,
    pub visibility:            VisibilityContext,
    pub powertrain:            PowertrainContext,
    pub gps:                   GpsContext,
    pub remaining_assemblies:  BTreeSet<AssemblyId>,   // was: pending_assemblies: BTreeSet<ZoneId>
}
```

Every reference to `ctx.pending_assemblies` (step.rs, transition_map.rs, tests) is updated.
`cargo fix` can apply most instances once the struct field is renamed.

---

## RED tests

New tests in `crates/common/src/test/fsm_preparation_contract.rs`.

They are RED **as compile errors** before Step 3 (the variant change): pattern
`FsmState::PreparingToStart { assemblies }` will not compile against a unit variant.

| Test | Asserts |
|---|---|
| `test_preparing_to_start_carries_assembly_ids` | `twin_turn(Off, ctx, PowerOn).next_state` pattern-matches `FsmState::PreparingToStart { assemblies }` where `assemblies` contains both `Headlamp` and `Wiper` |
| `test_preparing_to_stop_carries_assembly_ids` | Same for `PowerOff → PreparingToStop` |
| `test_remaining_assemblies_initialised_from_action` | After `twin_turn(Off, ctx, PowerOn)`: `result.modified_ctx.remaining_assemblies == BTreeSet::from_iter(result_assemblies.iter().copied())` — countdown equals the action's list |
| `test_assembly_zone_ready_decrements_not_reinitialises` | After PowerOn (remaining = {H, W}), `twin_turn(PreparingToStart{ALL}, ctx_with_both, AssemblyZoneReady(Headlamp))` → `remaining_assemblies == {Wiper}` only |
| `test_start_assemblies_action_carries_assembly_list` | `twin_turn(Off, ctx, PowerOn).actions` contains `DomainAction::StartAssemblies(assemblies)` where `assemblies == ALL_ASSEMBLIES` |

**Additional test in `fsm_step_contract.rs`** (one new function):

| Test | Asserts |
|---|---|
| `test_step_standard_commute_uses_state_embedded_assemblies` | Full `Off → PreparingToStart → Idle → Driving → Idle → PreparingToStop → Off` journey; intermediate `PreparingToStart` state has non-empty `assemblies` slice |

---

## Implementation steps (sequenced)

### Step 1 — Write RED tests

Add the five tests to `fsm_preparation_contract.rs`; one test to `fsm_step_contract.rs`.

Confirm: `cargo test -p common` shows **compile errors** referencing
`FsmState::PreparingToStart { assemblies }` — correct RED state.  None of these tests
should produce a logic failure (test assertion) before Step 3.

---

### Step 2 — Rename `ZoneId` → `AssemblyId`

In `crates/common/src/fsm/machineries.rs`:
```rust
pub enum AssemblyId { Headlamp, Wiper }
```

Compiler enumerates every broken reference.  Fix mechanically:

- `fsm/machineries.rs` — definition + `VehicleContext` mention
- `fsm/transition_map.rs` — all `ZoneId::` references
- `fsm/step.rs` — `ZoneId` in `pending_assemblies` handling (also renamed in Step 6)
- `twin_runtime/zone_turn.rs`, `turn_barrier.rs`, `virtual_car_actor.rs`, `wiper_actor.rs`,
  `headlamp_actor.rs`, `zone_replies.rs`, `outcome_map.rs`
- `digital_twin/mod.rs` — `ZoneReply`, `ZoneMessage`
- `vehicle_state/mod.rs` — `VehicleContext::pending_assemblies` (field rename in Step 6)
- All test files

`pub enum ZoneId` is deleted with no alias.

**Checkpoint:** `cargo build -p common` passes with zero `ZoneId` references.
`rg "ZoneId" --type rust` returns zero results.

---

### Step 3 — Add `assemblies` field to `FsmState`; define `ALL_ASSEMBLIES`

In `machineries.rs`:
```rust
pub(crate) const ALL_ASSEMBLIES: &[AssemblyId] = &[AssemblyId::Headlamp, AssemblyId::Wiper];

pub enum FsmState {
    PreparingToStart { assemblies: &'static [AssemblyId] },
    PreparingToStop  { assemblies: &'static [AssemblyId] },
    // …
}
```

Compiler now lists every file with a broken match arm.

**Checkpoint:** RED tests compile and reach assertion failures (no longer compile errors).

---

### Step 4 — `transition_map.rs`: construct variants; add data to `DomainAction`

In `machineries.rs`, change `DomainAction`:
```rust
StartAssemblies(&'static [AssemblyId]),
StopAssemblies(&'static [AssemblyId]),
```

In `transition_map.rs`:
- Every `→ PreparingToStart` returns `PreparingToStart { assemblies: ALL_ASSEMBLIES }`
- Every `→ PreparingToStop` returns `PreparingToStop { assemblies: ALL_ASSEMBLIES }`
- Self-loop arms capture and re-emit the field:
  ```rust
  FsmState::PreparingToStart { assemblies } => match event {
      _ => TransitionResult { next_state: PreparingToStart { assemblies }, note: None },
  }
  ```
- `output()`: `StartAssemblies(ALL_ASSEMBLIES)`, `StopAssemblies(ALL_ASSEMBLIES)`

**Checkpoint:** `test_preparing_to_start_carries_assembly_ids`,
`test_preparing_to_stop_carries_assembly_ids`, and
`test_start_assemblies_action_carries_assembly_list` are GREEN.

---

### Step 5 — `step.rs`: read from action; remove hardcoded sets

Replace the two `BTreeSet::from([Headlamp, Wiper])` blocks with the action-reading pattern
from D4.

**Checkpoint:** `test_remaining_assemblies_initialised_from_action` and
`test_assembly_zone_ready_decrements_not_reinitialises` are GREEN.

---

### Step 6 — Rename `pending_assemblies` → `remaining_assemblies`

In `crates/common/src/vehicle_state/mod.rs`:
```rust
pub remaining_assemblies: BTreeSet<AssemblyId>,   // was: pending_assemblies: BTreeSet<ZoneId>
```

Update all references: `step.rs`, `transition_map.rs`, tests.
`cargo fix` handles most instances after the struct field is renamed.

---

### Step 7 — `virtual_car_actor.rs`: delete `MANAGED_ASSEMBLIES`; read from action

- Delete `const MANAGED_ASSEMBLIES` and its module-level doc comment.
- Replace both `for &zone_id in MANAGED_ASSEMBLIES` loops with `for &assembly_id in assemblies`
  reading from `DomainAction::StartAssemblies(assemblies)` / `StopAssemblies(assemblies)`.
- Update all `ZoneId`-typed variables in this file to `AssemblyId` (Step 2 may have done this
  already if Step 2 and Step 7 are processed together).

**Checkpoint:** `rg MANAGED_ASSEMBLIES --type rust` returns zero results.

---

### Step 8 — `published.rs` and `domain_types.rs` match-arm updates (D5)

Decide at this point whether `PublishedFsmState::PreparingToStart` stays data-free.
Default: flatten with `{ .. }` wildcard.

```rust
FsmState::PreparingToStart { .. } => PublishedFsmState::PreparingToStart,
FsmState::PreparingToStop  { .. } => PublishedFsmState::PreparingToStop,
```

Same for `domain_types.rs` `From<&FsmState>` for `VehicleState`.

---

### Step 9 — Mechanical match-arm sweep for remaining test files

Fix every remaining compile error using the translation table:

| Pattern before | Pattern after | When |
|---|---|---|
| `FsmState::PreparingToStart` | `FsmState::PreparingToStart { .. }` | Not reading `assemblies` |
| `== FsmState::PreparingToStart` | `== FsmState::PreparingToStart { assemblies: ALL_ASSEMBLIES }` | Equality with known constant |
| `matches!(s, FsmState::PreparingToStart)` | `matches!(s, FsmState::PreparingToStart { .. })` | Boolean check |

`ALL_ASSEMBLIES` must be visible in test modules: either use `use crate::fsm::ALL_ASSEMBLIES`
or the equivalent literal `&[AssemblyId::Headlamp, AssemblyId::Wiper]`.

Also: update comment references to `MANAGED_ASSEMBLIES` in `turn_barrier_contract.rs`.

---

### Step 10 — Full suite green

```bash
cargo test -p common
cargo test -p gateway
cargo test -p vehicle_device_bus
```

All 6 new RED tests must be GREEN.  All 158 prior tests must remain GREEN.
`cargo build -p common` emits zero warnings.  
`rg "ZoneId" --type rust` → zero.  `rg "MANAGED_ASSEMBLIES" --type rust` → zero.
`rg "pending_assemblies" --type rust` → zero.

---

## Deletion checklist

| Item deleted | File |
|---|---|
| `pub enum ZoneId` | `fsm/machineries.rs` |
| All `ZoneId` references | everywhere |
| `const MANAGED_ASSEMBLIES` | `virtual_car_actor.rs` |
| Module-level doc comment referencing `MANAGED_ASSEMBLIES` | `virtual_car_actor.rs` |
| Hardcoded `BTreeSet::from([Headlamp, Wiper])` × 2 | `step.rs` |
| `// Keep in sync with MANAGED_ASSEMBLIES` comment | `step.rs` |
| `MANAGED_ASSEMBLIES` comment references | `turn_barrier_contract.rs` |
| `pending_assemblies` field | `vehicle_state/mod.rs` (renamed, not deleted) |

---

## File change summary

| File | Action | Reason |
|---|---|---|
| `crates/common/src/fsm/machineries.rs` | Modify | `AssemblyId` rename; `ALL_ASSEMBLIES` const; struct variants; `DomainAction` data |
| `crates/common/src/fsm/transition_map.rs` | Modify | Construct struct variants with `ALL_ASSEMBLIES`; `output()` data |
| `crates/common/src/fsm/step.rs` | Modify | Read from action (D4); delete hardcoded sets; `remaining_assemblies` rename |
| `crates/common/src/vehicle_state/mod.rs` | Modify | `remaining_assemblies` field rename; `AssemblyId` type |
| `crates/common/src/published.rs` | Modify | `{ .. }` wildcard or mirror decision (D5) |
| `crates/common/src/domain_types.rs` | Modify | `{ .. }` wildcard in `From<&FsmState>` |
| `crates/common/src/twin_runtime/zone_turn.rs` | Modify | `AssemblyId` rename; `{ .. }` wildcard in guards |
| `crates/common/src/twin_runtime/turn_barrier.rs` | Modify | `AssemblyId` rename |
| `crates/common/src/twin_runtime/controller/virtual_car_actor.rs` | Modify | Delete `MANAGED_ASSEMBLIES`; read from action; `AssemblyId` rename |
| `crates/common/src/twin_runtime/wiper_actor.rs` | Modify | `AssemblyId` rename |
| `crates/common/src/twin_runtime/headlamp_actor.rs` | Modify | `AssemblyId` rename (if any direct refs) |
| `crates/common/src/digital_twin/mod.rs` | Modify | `AssemblyId` rename in `ZoneReply`, `ZoneMessage` |
| `crates/common/src/twin_runtime/zone_replies.rs` | Modify | `AssemblyId` rename |
| `crates/common/src/test/fsm_preparation_contract.rs` | Modify | 5 new RED→GREEN tests |
| `crates/common/src/test/fsm_step_contract.rs` | Modify | 1 new test; `AssemblyId` rename |
| `crates/common/src/test/turn_barrier_contract.rs` | Modify | `AssemblyId` rename; comment cleanup |
| ~20 other test files | Modify | `AssemblyId` rename; `{ .. }` / `ALL_ASSEMBLIES` match updates |

All paths relative to workspace root unless shown.

---

---

## Revisit — countdown data structure: `BTreeSet` vs. fixed array

**Context (raised 2026-06-23, post Phase 8/9 implementation).**

Phase 9 changed `PreparingToStart` / `PreparingToStop` to carry a `BTreeSet<AssemblyId>`
as the authoritative countdown of assemblies still awaiting acknowledgement.
The question was raised: would a fixed-size array (with shift-left removal and a sentinel
or length counter) be more efficient?

### Where a fixed array wins

| Property | `BTreeSet` | Fixed array + len |
|---|---|---|
| Heap allocation per step | One alloc per `AssemblyZoneReady` | Zero — fully stack-resident |
| Clone cost | Tree copy + allocator call | `memcpy` of N words |
| `no_std` / no-alloc compatible | No | Yes |

For a 2-assembly topology the BTreeSet allocations are two per commute lifecycle — unmeasurable in practice, but real.

### Where `BTreeSet` is better

| Property | `BTreeSet` | Fixed array + len |
|---|---|---|
| Order-independent equality | Automatic (set semantics) | Convention-enforced (must guarantee canonical insertion order) |
| `is_empty()` correctness | Built-in | Requires comparing `len`, not the full array |
| Off-by-one surface | None | Shift-left bookkeeping |
| `PartialEq` correctness | Automatic | Must derive on `(slots[0..len], len)`, not raw array |

The `output()` intra-mode guard (`(PreparingToStart(_), PreparingToStart(_)) => vec![]`)
relies on `PartialEq` detecting a shrinking set.  With a raw array the equality implementation
must not compare stale tail slots — a subtle invariant to maintain by hand.

### Recommendation

Keep `BTreeSet` for this simulation project where an allocator is always present.

If the FSM is ever ported to a **bare-metal ECU (no allocator)**, replace `BTreeSet<AssemblyId>`
with `arrayvec::ArrayVec<AssemblyId, MAX_ASSEMBLIES>`.  `ArrayVec` gives:
- stack-only storage with a fixed capacity,
- a correct `len()` / `is_empty()`,
- an `Iterator` and correct `PartialEq` (compares only the live prefix),
- the shift-left removal semantic via `.remove(pos)`.

That swap is a two-line change in `machineries.rs` — no logic elsewhere changes.

---

## Discussion checkpoint after Phase 8

1. `cargo test -p common && cargo test -p gateway && cargo test -p vehicle_device_bus` —
   full suite across all crates green (target: 164+ tests).
2. `rg "ZoneId" --type rust` → zero.  `rg "MANAGED_ASSEMBLIES" --type rust` → zero.
   `rg "pending_assemblies" --type rust` → zero.
3. **Single-source confirmation:** walk through Phase 8's stated invariant: "Adding a third
   assembly (`AssemblyId::Radar`) requires editing `ALL_ASSEMBLIES` in `machineries.rs` and
   implementing `RadarActor` — zero other coordination plumbing changes."
4. `FsmState::PreparingToStart { assemblies }` and `DomainAction::StartAssemblies(assemblies)`
   carry the same slice from the same constant.  Verify no divergence is possible by
   inspection: both reference `ALL_ASSEMBLIES` and there is no other write path.
   *(Note: this struct-variant syntax was the Phase 8 intermediate design; Phase 9 changed it
   to `PreparingToStart(BTreeSet<AssemblyId>)`.)*
5. `handle()` in `virtual_car_actor.rs` still has the same arms as Phase 7.
6. **Superseded by Phase 9 (same session):** The `&'static [AssemblyId]` struct-variant design
   was found to require peek-ahead arithmetic in `transition_map.rs` (reading
   `ctx.remaining_assemblies` to predict the next state before `step.rs` mutated it).
   Phase 9 replaced the static-slice struct variants with `BTreeSet<AssemblyId>` tuple variants,
   making `transition()` self-contained and deleting `VehicleContext::remaining_assemblies`
   entirely.  See `brain_fsm_redesign_impl_Phase_9.md`.
