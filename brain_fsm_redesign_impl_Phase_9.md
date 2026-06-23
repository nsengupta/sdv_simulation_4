# Brain FSM Redesign — Phase 9 Implementation
## FSM State Owns the Countdown; `VehicleContext::remaining_assemblies` Deleted

**Status:** COMPLETED (2026-06-23, same session as Phase 8).  
**Depends on:** Phase 8 complete (`AssemblyId` rename; `ALL_ASSEMBLIES`; struct variants
with `&'static [AssemblyId]`; `VehicleContext::remaining_assemblies`; 179 tests green).  
**Emerged from:** Phase 8 code review — a temporal mismatch in `transition_map.rs` was
identified and a cleaner design was proposed and implemented immediately.  
**Next phase:** Phase 10 — Documentation consolidation; remaining simulation-4 objectives.

---

## Problem Phase 9 solves

After Phase 8, the FSM had two parallel sources of countdown truth:

| Source | Role |
|---|---|
| `FsmState::PreparingToStart { assemblies: &'static [AssemblyId] }` | Static topology — always `ALL_ASSEMBLIES`, never shrinks |
| `VehicleContext::remaining_assemblies: BTreeSet<AssemblyId>` | Live countdown — shrinks on each `AssemblyZoneReady` |

This created a **temporal mismatch** in `transition_map.rs`: `transition()` had to peek ahead
into a future value of `ctx.remaining_assemblies` (the mutation happened *later* in `step.rs`)
to decide whether to go to `Idle` or self-loop:

```rust
// Phase 8 — peek-ahead arithmetic in transition_map.rs
let remaining_after = current_ctx.remaining_assemblies.len()
    - usize::from(current_ctx.remaining_assemblies.contains(assembly_id));
if remaining_after == 0 {
    TransitionResult { next_state: Idle, note: None }
} else {
    // Reset to ALL_ASSEMBLIES — loses the "how many have acked?" information
    TransitionResult { next_state: PreparingToStart { assemblies: ALL_ASSEMBLIES }, note: None }
}
```

Additionally, every self-loop in `PreparingToStart` re-stamped the `assemblies` field with
`ALL_ASSEMBLIES`, discarding the progress information that `ctx.remaining_assemblies` was
separately tracking.

---

## What Phase 9 delivers

The countdown moves **into** the FSM state variant itself.
`FsmState::PreparingToStart` becomes a tuple variant holding a `BTreeSet<AssemblyId>` that
shrinks on every acknowledgement:

```
Off + PowerOn
    → PreparingToStart({Headlamp, Wiper})

PreparingToStart({Headlamp, Wiper}) + AssemblyZoneReady(Headlamp)
    → PreparingToStart({Wiper})

PreparingToStart({Wiper}) + AssemblyZoneReady(Wiper)
    → Idle
```

`VehicleContext::remaining_assemblies` is deleted. `transition()` is a fully self-contained
pure function — no external bookkeeping required.

---

## Design decisions

### D1 — Tuple variant with `BTreeSet<AssemblyId>`

```rust
// Phase 8 (intermediate)
PreparingToStart { assemblies: &'static [AssemblyId] }

// Phase 9 (final)
PreparingToStart(BTreeSet<AssemblyId>)
```

`BTreeSet<AssemblyId>` was chosen over:

- `&'static [AssemblyId]` — a static slice can only ever hold `ALL_ASSEMBLIES`; it cannot
  represent a shrinking subset.
- Fixed array + length counter — requires manual shift-left bookkeeping, order-dependent
  `PartialEq`, and off-by-one risk. Suits embedded (no-alloc) targets but not this simulation.

For embedded targets, `arrayvec::ArrayVec<AssemblyId, MAX_ASSEMBLIES>` is the correct
replacement — a two-line change in `machineries.rs`. See `brain_fsm_redesign_impl_Phase_8.md`
"Revisit" section for the full trade-off analysis.

### D2 — `transition()` becomes self-contained

`transition()` reads the remaining set from the current FSM state, produces a filtered copy
as the next state, and calls `is_empty()` to decide the target state.
It makes zero reads from `VehicleContext` for countdown purposes.

```rust
// Phase 9 — no peek-ahead
PreparingToStart(remaining) => match event {
    AssemblyZoneReady(assembly_id) => {
        let new_remaining: BTreeSet<AssemblyId> =
            remaining.iter().copied().filter(|a| a != assembly_id).collect();
        if new_remaining.is_empty() {
            TransitionResult { next_state: Idle, note: None }
        } else {
            TransitionResult { next_state: PreparingToStart(new_remaining), note: None }
        }
    }
    _ => TransitionResult { next_state: PreparingToStart(remaining.clone()), note: None },
},
```

### D3 — `output()` intra-mode guard

With `BTreeSet` semantics, `PreparingToStart({Headlamp, Wiper})` ≠ `PreparingToStart({Wiper})`,
so the catch-all `(old, new) if old != new => vec![PublishStateSync]` would fire on every
intermediate acknowledgement — emitting a spurious external state-sync notification.

Explicit guards added before the catch-all:

```rust
// Intra-mode steps: an assembly acked but peers still pending.
// The FSM mode has not changed; no external event to publish.
(PreparingToStart(_), PreparingToStart(_)) => vec![],
(PreparingToStop(_), PreparingToStop(_)) => vec![],
```

### D4 — `DomainAction` payload changed to `Vec<AssemblyId>`

`StartAssemblies` and `StopAssemblies` payloads changed from `&'static [AssemblyId]` to
`Vec<AssemblyId>` (owned). The actor receives the full list exactly once on entry; no lifetime
constraints needed.

### D5 — `VehicleContext::remaining_assemblies` deleted

Renamed from `pending_assemblies` in Phase 8. Deleted entirely in Phase 9.
`VehicleContext` now carries only assembly-domain state (sensors, actuators) — zero FSM
lifecycle bookkeeping.

---

## Implementation steps

### Step 1 — Change variant types in `machineries.rs`

Add `use std::collections::BTreeSet;`.  
Change `FsmState::PreparingToStart` / `PreparingToStop` from struct to tuple variants.  
Change `FsmAction` / `DomainAction` `StartAssemblies` / `StopAssemblies` payloads to `Vec<AssemblyId>`.  
Update `From<&FsmState> for VehicleState`: `PreparingToStart { .. }` → `PreparingToStart(_)`.

### Step 2 — Rewrite `transition()` and `output()` in `transition_map.rs`

Add `use std::collections::BTreeSet;` and `use super::machineries::AssemblyId;`.

- Entry transitions: `PreparingToStart(ALL_ASSEMBLIES.iter().copied().collect())`
- `AssemblyZoneReady` arm: filter the state's `BTreeSet`, check `is_empty()`
- Self-loop arms: `PreparingToStart(remaining.clone())`
- `output()`: add intra-mode guards; entry arm emits `StartAssemblies(ALL_ASSEMBLIES.to_vec())`

### Step 3 — Delete mutation block from `step.rs`

Remove the entire `remaining_assemblies` initialisation / decrement block (was ~15 lines).
Change `let mut modified_ctx` to `let modified_ctx` (no longer mutated).

### Step 4 — Delete `remaining_assemblies` from `VehicleContext`

Remove the field and the `use std::collections::BTreeSet;` import from `vehicle_state/mod.rs`.

### Step 5 — Match-arm sweep

Update all `FsmState::PreparingToStart { .. }` patterns to `FsmState::PreparingToStart(_)` and
`FsmState::PreparingToStop { .. }` to `FsmState::PreparingToStop(_)` in:
`zone_turn.rs`, `diagnostic/mod.rs`, `published.rs`, and all test files.

### Step 6 — Full suite green

```bash
cargo test
```
179 tests, 0 warnings.

---

## Tests

Five new Phase 9 tests added to `crates/common/src/test/fsm_preparation_contract.rs`:

| Test | Asserts |
|---|---|
| `test_preparing_to_start_carries_assembly_ids` | `PreparingToStart(remaining)` contains `Headlamp` and `Wiper` after `PowerOn` |
| `test_preparing_to_stop_carries_assembly_ids` | Same for `PreparingToStop` after `PowerOff` |
| `test_state_and_action_agree_on_assembly_set` | State inner `BTreeSet` equals `StartAssemblies` action payload |
| `test_assembly_zone_ready_shrinks_state_not_context` | After Headlamp acks: state is `PreparingToStart({Wiper})`; `VehicleContext` untouched |
| `test_start_assemblies_action_carries_assembly_list` | `StartAssemblies(Vec<AssemblyId>)` contains both assemblies |

All prior tests in `fsm_engine_contract.rs`, `fsm_step_contract.rs`, and `wiper_zone_contract.rs`
updated to tuple-variant patterns and direct state construction (no `ctx.remaining_assemblies`).

---

## Deletion checklist

| Item deleted | File |
|---|---|
| `VehicleContext::remaining_assemblies` field | `vehicle_state/mod.rs` |
| `use std::collections::BTreeSet` import (no longer needed) | `vehicle_state/mod.rs` |
| `remaining_assemblies` mutation block in `step()` | `fsm/step.rs` |
| Peek-ahead arithmetic (`remaining_after` calculation) | `fsm/transition_map.rs` |
| `ALL_ASSEMBLIES` re-stamp on every self-loop | `fsm/transition_map.rs` |
| `ctx_with_headlamp_pending()` test helper (was setting `remaining_assemblies`) | `test/fsm_preparation_contract.rs` |

---

## File change summary

| File | Change |
|---|---|
| `crates/common/src/fsm/machineries.rs` | Tuple variants `(BTreeSet<AssemblyId>)`; action payloads `Vec<AssemblyId>`; `BTreeSet` import |
| `crates/common/src/fsm/transition_map.rs` | Shrinking-set logic in `transition()`; intra-mode guards in `output()` |
| `crates/common/src/fsm/step.rs` | Mutation block deleted; `let mut` → `let` |
| `crates/common/src/vehicle_state/mod.rs` | `remaining_assemblies` field deleted; `BTreeSet` import removed |
| `crates/common/src/twin_runtime/zone_turn.rs` | `{ .. }` → `(_)` in `PreparingToStart`/`PreparingToStop` guards |
| `crates/common/src/test/fsm_preparation_contract.rs` | Full rewrite: `ctx` field assertions → state inner `BTreeSet` |
| `crates/common/src/test/fsm_engine_contract.rs` | `ctx.remaining_assemblies` blocks → direct state construction |
| `crates/common/src/test/fsm_step_contract.rs` | Struct-variant pattern → tuple-variant |
| `crates/common/src/test/wiper_zone_contract.rs` | Struct-variant pattern → tuple-variant |

---

## Discussion checkpoint after Phase 9

1. `cargo test` — 179 tests green, 0 warnings. ✅
2. `rg "remaining_assemblies" --type rust` → zero. ✅
3. `rg 'PreparingToStart {' --type rust` → zero (all usages are tuple-variant). ✅
4. `transition()` reads zero countdown state from `VehicleContext`. ✅
5. **Deferred — embedded target:** If this FSM is ported to a bare-metal ECU (no allocator),
   replace `BTreeSet<AssemblyId>` with `arrayvec::ArrayVec<AssemblyId, MAX_ASSEMBLIES>` —
   a two-line change in `machineries.rs`, no logic changes elsewhere.
   See `brain_fsm_redesign_impl_Phase_8.md` "Revisit" section for full trade-off analysis.
6. **Remaining simulation-4 objectives not addressed in Phases 1–9:**
   CAN emulation, non-blocking actuation, code commenting pass, actor-level fuzz tests.
   Captured in Phase 10 — see `brain_fsm_redesign_plan.md` and
   `brain_fsm_redesign_impl_Phase_10.md`.
