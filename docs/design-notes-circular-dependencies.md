# Circular dependencies in `common` — analysis & refactor notes

**Status:** analysis only — no code changes in Iteration 2.  
**Captured:** 2026-05-31  
**Context:** static inspection tool reported 8 circular dependencies (4 unique cycles, each duplicated). This document preserves the discussion for a likely refactor in the next iteration.

---

## Tool output (as reported)

Found 8 circular dependencies (4 unique):

1. `common::digital_twin → common::fsm → common::engine → common::digital_twin`
2. `common::engine → common::fsm → common::engine`
3. `common::engine → common::published → common::fsm → common::engine`
4. `common::fsm → common::vehicle_kinematics → common::fsm`

Cycles 1–2 and 3–4 appear twice in the tool output (duplicate listings).

Key file references from the tool:

| Cycle | Files cited |
| ----- | ----------- |
| digital_twin ↔ engine (via fsm) | `digital_twin/mod.rs:10`, `digital_twin/car_behaviour_checker.rs:15`, `fsm/step.rs:24`, `engine/connectors/physical_to_digital.rs:2`, `engine/controller/actuation_manager.rs:4`, `engine/controller/vehicle_controller.rs:1` |
| engine ↔ fsm | `engine/connectors/physical_to_digital.rs:4`, `engine/context/vehicle_context.rs:1`, `engine/controller/actuation_manager.rs:6`, `engine/controller/vehicle_controller.rs:4`, `engine/op_strategy/transition_map.rs:1–2`, `fsm/step.rs:24` |
| engine ↔ published ↔ fsm | `engine/controller/vehicle_controller.rs:5`, `published.rs:22`, `fsm/step.rs:24` |
| fsm ↔ vehicle_kinematics | `fsm/assembly/powertrain.rs:9`, `vehicle_kinematics.rs:7` |

**Note:** within a single Rust crate, module cycles compile. The tool flags **architectural** layering, not build failures. Cycles still matter because they contradict documented intent, complicate mental models, and block a future crate split (e.g. `fsm` as a standalone library for offline verification).

---

## Stated intent vs. current reality

`crates/common/src/lib.rs` declares sibling order and an acyclic rule:

```rust
// Sibling order is *dependee before dependent* (foundation first), not "flow" order.
// `digital_twin` imports `fsm`; `fsm` does not import `digital_twin`.
```

The second comment is **partially stale**: `fsm` still does not import `digital_twin` directly, but it **does** import `engine`, and `engine` imports `digital_twin`. So the effective graph is `fsm → engine → digital_twin`, which closes a triangle with `digital_twin → fsm`.

`digital_twin/mod.rs` repeats the intended one-way dependency:

> Depends on `crate::fsm` for `FsmState`, `FsmEvent`, and `VehicleContext` only — the FSM crate module does not reference this layer.

That was true for a direct `fsm ↔ digital_twin` edge, but not for the indirect path through `engine`.

Related design precedent (acyclic, deliberate): `docs/design-notes-runtime-observation.md` documents `diagnostic → fsm + vehicle_constants` as intentional “good coupling” at the producer, with no cycle. The cycles below are different: they make the **core depend on the shell**.

---

## Target layering (Iteration 2 README intent)

```text
foundation     domain_types, signals, vehicle_constants, vehicle_kinematics (pure)
                    ↓
pure core      fsm  (assemblies, machineries, step, transition_map)
                    ↓
projections    digital_twin, published, diagnostic
                    ↓
runtime        engine, transition_sink
```

Runtime data flow (ingress/egress) is not the same as compile-time dependency direction. The pure FSM core should sit **below** runtime orchestration, not depend on it.

---

## Cycle-by-cycle verdict

| Cycle | Justifiable? | Root cause | Proposed action |
| ----- | ------------ | ---------- | --------------- |
| `fsm ↔ engine` | **No** | `transition_map` lives under `engine` but is pure FSM logic; `fsm::step` imports it | Move `transition_map` into `fsm` |
| `digital_twin → fsm → engine → digital_twin` | **No** (derived) | Same hinge as above | Fixed by moving `transition_map` |
| `engine → published → fsm → engine` | **Partially** — `published → fsm` and `engine → published` are fine; `fsm → engine` is not | Same hinge | Fixed by moving `transition_map` |
| `fsm ↔ vehicle_kinematics` | **No** (convenience, not domain) | Pure math module imports `VehicleContext` for a thin wrapper | Make `vehicle_kinematics` pure math; drop or relocate `refresh_context_speed` |

**Priority:** fix the `fsm → engine` edge first (unblocks three reported cycles). The kinematics cycle is a small, independent cleanup.

---

## Cycle 1 & 2: `fsm ↔ engine` and the big triangle through `digital_twin`

### The hinge (only `fsm → engine` imports in the tree)

Within all of `fsm/`, only two lines reach into `engine`:

```rust
// fsm/step.rs
use crate::engine::op_strategy::transition_map::{output, transition, TransitionNote};

// fsm/engine.rs (shim)
pub use crate::engine::op_strategy::transition_map::{output, transition, TransitionNote, TransitionResult};
```

`engine` naturally depends on `fsm` everywhere (`FsmEvent`, `DomainAction`, `VehicleContext`, etc.), which closes `fsm → engine → fsm`.

`transition_map` is already documented as a **pure decision function** (no I/O, no actors). It only imports `fsm` types and `vehicle_constants`. It belongs in the FSM layer, not under `engine`.

### Why `digital_twin → fsm → engine → digital_twin` closes

```text
digital_twin ──(1)──► fsm ──(2)──► engine ──(3)──► digital_twin
```

**Edge 1 — `digital_twin → fsm` (intended)**

- `digital_twin/mod.rs` and `car_behaviour_checker.rs` import `FsmState`, `FsmEvent`, `VehicleContext`.
- No `use crate::engine` anywhere under `digital_twin/`.

**Edge 2 — `fsm → engine` (problem)**

- Only via `fsm/step.rs` and `fsm/engine.rs` as above.
- No other file under `fsm/` imports `digital_twin` or `engine`.

**Edge 3 — `engine → digital_twin` (intended)**

| File | Why |
| ---- | --- |
| `engine/controller/virtual_car_actor.rs` | Actor mailbox `DigitalTwinCarVocabulary` |
| `engine/controller/vehicle_controller.rs` | RPC to twin actor |
| `engine/controller/actuation_manager.rs` | Reads twin state for actuation |
| `engine/connectors/physical_to_digital.rs` | Projects physical signals into twin vocabulary |

`digital_twin` never imports `engine`. So the **only** path from `fsm` back to `digital_twin` is:

```text
fsm → engine → digital_twin   (no direct fsm → digital_twin import exists)
```

Remove edge 2 and the triangle cannot form.

### Fix: move `transition_map` into `fsm`

**Today:**

```text
fsm/step.rs  ──imports──►  engine/op_strategy/transition_map.rs
                                    │
                                    └──imports──►  fsm (types) + vehicle_constants
```

**After move:**

```text
fsm/step.rs  ──imports──►  fsm/op_strategy/transition_map.rs
                                    │
                                    └──imports──►  vehicle_constants (+ fsm types, same module)
```

Concrete steps (next iteration):

1. Move `crates/common/src/engine/op_strategy/transition_map.rs` → `crates/common/src/fsm/op_strategy/transition_map.rs` (or `fsm/strategy/`).
2. Update `fsm/step.rs` to import from the local module.
3. Repoint or remove `fsm/engine.rs`; re-export `output`, `transition`, `TransitionNote` from `fsm/mod.rs` if needed for public API stability.
4. Update tests that import `crate::engine::op_strategy::transition_map` (e.g. `test/op_strategy_contract.rs`) to use `fsm` paths.
5. Remove empty `engine/op_strategy/` if nothing remains.
6. Update stale comments in `lib.rs` and `digital_twin/mod.rs`.

**Resulting acyclic graph:**

```text
                    vehicle_constants
                           ▲
                           │
              ┌────────────┴────────────┐
              │          fsm            │
              │  (no engine import)     │
              └────────────┬────────────┘
                           │
              ┌────────────┴────────────┐
              ▼                         ▼
        digital_twin                  engine
              ▲                         │
              └─────────────────────────┘
```

One-way edges only: `digital_twin → fsm`, `engine → fsm`, `engine → digital_twin`.

**What does not need to change for this cycle:**

- `digital_twin → fsm` — twin is defined over FSM state/context.
- `engine → digital_twin` — runtime drives the twin actor.
- `transition_map` logic — behaviour unchanged; only module placement changes.

---

## Cycle 3: `engine → published → fsm → engine`

| Edge | Verdict |
| ---- | ------- |
| `published → fsm` | **Correct.** `published` is the serde/wall-clock mirror of FSM types (`From` impls, `SessionEpoch` projection). |
| `engine → published` | **Correct.** `VehicleController` and actor runtime use `PublishedTransitionRecord` / transition sink types. (`vehicle_controller.rs` imports via `transition_sink`, which re-exports `published`.) |
| `fsm → engine` | **Incorrect** — same root cause as Cycle 1. |

After moving `transition_map`: `engine → published → fsm` with no back-edge. No change required to `published` itself.

---

## Cycle 4: `fsm ↔ vehicle_kinematics`

**Forward edge (fine):** `fsm/assembly/powertrain.rs` calls `vehicle_kinematics::calculate_speed_from_rpm` inside `PowertrainContext::refresh_speed()`.

**Back-edge (accidental):** `vehicle_kinematics.rs` imports `VehicleContext` only for:

```rust
pub fn refresh_context_speed(ctx: &mut VehicleContext) {
    ctx.powertrain.refresh_speed();
}
```

Powertrain already owns `refresh_speed()`. The pure function `calculate_speed_from_rpm` needs no FSM types.

**Fix (minimal, independent of transition_map move):**

1. Make `vehicle_kinematics` **pure math only** — drop the `VehicleContext` import.
2. Remove or stop exporting `refresh_context_speed`; call sites use `ctx.powertrain.refresh_speed()` directly.
   - Tests: `test/fsm_engine_contract.rs`, `test/op_strategy_contract.rs`, `test/fsm_properties.rs`, `test/fsm_step_contract.rs`.
3. Keep `calculate_speed_from_rpm` public for the emulator (`crates/emulator/src/car_physics.rs`).

Alternative (less preferred): duplicate the RPM constant in powertrain, or move the formula to `vehicle_constants` — avoids duplication poorly compared to a dependency-free kinematics module.

---

## Why the tool lists many file:line pairs

The inspector walks **every import on the cycle**, not claiming each file is independently wrong:

- `digital_twin/mod.rs:10` — start of leg 1
- `fsm/step.rs:24` — leg 2 (the hinge)
- Multiple `engine/...` lines — different branches of leg 3 that all reach `digital_twin`

Fixing leg 2 (`fsm → engine`) is sufficient; legs 1 and 3 remain valid one-way dependencies.

---

## Refactor checklist (next iteration)

- [ ] Move `engine/op_strategy/transition_map.rs` → `fsm/op_strategy/transition_map.rs`
- [ ] Fix `fsm/step.rs` and `fsm/engine.rs` imports / re-exports
- [ ] Update `test/op_strategy_contract.rs` and any other `engine::op_strategy` references
- [ ] Run full `cargo test` in workspace
- [ ] Re-run circular-dependency inspection tool; expect 0–1 cycles (kinematics only, until fixed)
- [ ] Split `vehicle_kinematics` into pure math; migrate off `refresh_context_speed`
- [ ] Update `lib.rs` comment (`fsm` does not import `engine` or `digital_twin`)
- [ ] Update `digital_twin/mod.rs` module doc if wording still implies strict non-reference from FSM side (indirect path will be gone)

---

## References in repo

- `crates/common/src/lib.rs` — module sibling order comment
- `crates/common/src/fsm/step.rs` — orchestrator; current `engine` import
- `crates/common/src/fsm/engine.rs` — shim re-exporting from `engine`
- `crates/common/src/engine/op_strategy/transition_map.rs` — pure transition table (misplaced)
- `crates/common/src/digital_twin/mod.rs` — twin layer; depends on `fsm` only
- `crates/common/src/published.rs` — serializable projection of `fsm` types
- `crates/common/src/vehicle_kinematics.rs` — RPM→km/h; spurious `VehicleContext` dep
- `docs/design-notes-runtime-observation.md` — ADR-1/3, acyclic coupling examples (`diagnostic`, `ledger_audit` placement)
