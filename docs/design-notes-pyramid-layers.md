# Library pyramid — dependency graph (work in progress)

**Status:** built incrementally via Q&A (2026-06-01).  
**Acid test:** each layer must compile as an independent library; nothing imports from layers above.

**Origin:** static analysis reported module cycles in `common` (`fsm ↔ engine`, `digital_twin → fsm → engine → digital_twin`, etc.). Those cycles were the **smell** that started this layering exercise — not an acceptable end state. Phase A tolerates them temporarily; **Phase B must eliminate them** (see [Packaging plan](#packaging-plan-agreed-2026-06-01)).

---

## Agreed layers

```text
L0  foundation
      constants, pure math, wire facts (std + serde)
      criterion: elemental — a change here justifies full downstream recompile

L1  vehicle state shapes
      WheelRpm, LightingState, VehicleContext, …
      imports L0 only
      • **VehicleContext is L1, not FSM-owned** — see [VehicleContext placement](#vehiclecontext--l1-not-fsm-only)

L2  pure decision core
      FsmState, FsmEvent, DomainAction, step(), transition rules
      imports L1, L0 — no actors, CAN, logging, serde mirror

L3  twin state capsule
      DigitalTwinCar, apply_step, snapshots
      “what the car IS now” — not how it got there
      imports L2; L2 never imports L3

L3′ published / observation types (optional sibling)
      PublishedTransitionRecord — serde projection for ledger/replay
      not live state; emitted async after commit

L4  twin runtime
      Actor: step → apply_step → hand off async (TransitionTx, DiagnosticTx, ActuatorManager)
      Executors behind traits; may be other threads/processes
      ACK/NACK re-enters Actor queue as new FsmEvent (never mid-turn callback)
      imports L3, L2; L2/L3 unaware of I/O

L5  Controller
      only doorway for L6; composition root (inject sinks, actuation, spawn Actor)
      owns projector wiring: PhysicalCarVocabulary → FsmEvent

L6  gateway / emulator / actuator
      wire adapters (DBC, CAN, future uProtocol/ProtoBuf) → PhysicalCarVocabulary
      never sends FsmEvent directly
```

---

## Physical rules: enforce + detect (same L0 constants)

Three roles, one constitution:

| Role | Layer | When |
| ---- | ----- | ---- |
| **Enforce** | L2 | transition / step — illegal states unreachable |
| **Announce** | L4 | diagnostics when clamped/rejected |
| **Detect** | L3 | `verify_state_laws` — oracle for tests/CI/replay; never hot path |

**Rule:** every threshold enforced in L2 must have a matching L3 law using the **same L0 constant**.

**First application (2026-06-01):** `RPM_DRIVING_THRESHOLD` in `vehicle_constants` — used by `transition_map` (Idle→Driving) and `law_rpm_above_threshold_holds`.

---

## Important TODO — table-driven law catalog

**Priority:** high (before / during actorification).

When each assembly becomes a child Actor, transition rules will scatter across modules. A **single named rule catalog** will prevent drift between:

- L2 enforce paths (per-assembly + parent orchestrator), and
- L3 detect paths (`STATE_LAWS`).

**Target shape (future):**

```text
Rule row: { name, L0 constant(s), enforce site, detect predicate }
  → L2 reads row for transitions
  → L3 reads same row for verify_state_laws
```

**Not doing yet:** shared table machinery — only paired constants + comments for now.

**Trigger to implement:** assembly child actors land, or second threshold pair drifts (like the old 1000 vs 500 mismatch).

---

## Target module naming (Phase B — agreed direction)

| Today | Target | Layer | Notes |
| ----- | ------ | ----- | ----- |
| `vehicle_constants`, `vehicle_kinematics` | `vehicle_physics/` (grouped) | L0 | Constants + pure formulas; **not** `car_physical_engine` (avoids “engine” ambiguity) |
| `fsm/assembly/*`, `VehicleContext` | `vehicle_state/` or keep under assemblies | L1 | State shapes independent of FSM pattern |
| `fsm/` (step, events, `op_strategy`) | `fsm/` | L2 | Pure decision core |
| `digital_twin/` | `digital_twin/` | L3 | Capsule, laws, snapshots |
| `engine/` | **`twin_runtime/`** | L4 | Actor, actuation, connectors |
| `facade/` | `facade/` | L5 | Gateway public API |

**Remove in Phase B (mandatory — all unnecessary code must go):**

Shims, unused aliases, and compatibility re-exports left over from namespace migrations are **not** kept “just in case”. Phase B deletes them; call sites use the canonical path.

| Remove | Replace with |
| ------ | ------------ |
| `engine/context/` + `VehicleControllerContext` alias | `fsm::VehicleContext` (or `vehicle_state::VehicleContext` after L1 move) |
| `fsm/engine.rs` shim | deleted — re-export from `fsm/mod.rs` via `transition_map` |
| `virtual_car_actor.rs` (crate root) re-export | `twin_runtime::controller::virtual_car_actor` |
| `test/engine_namespace_contract.rs` (alias tests) | removed with the alias |

**Deferred:** whether L0–L2 ship as one **`sdv_core`** crate or stay modules inside `common` — decide when `sdv_core` lands, not before.

---

## VehicleContext — L1, not FSM-only

**Agreed (2026-06-01):** `VehicleContext` is **not** an FSM-only type.

It is the aggregate of **what the car IS** in the twin’s world — sensed and derived assembly state:

```text
VehicleContext
├── powertrain   (WheelRpm, speed, mode)
├── health       (fuel, oil, tyre)
├── visibility   (ambient lux)
└── headlamp     (LightingState, ACK-wait)
```

The FSM **consumes and updates** this state via `step()`, but if the FSM were replaced (if-then-else, rules engine, per-zone actors), **`VehicleContext` would still exist**. The capsule (`DigitalTwinCar`) would still hold it; only the decision machinery changes.

| Type | Layer | Survives without FSM? |
| ---- | ----- | ----------------------- |
| `VehicleContext`, assemblies | **L1** | Yes |
| `FsmState`, `FsmEvent`, `step()` | **L2** | No — these *are* the FSM (or whatever replaces it) |
| `DigitalTwinCar` | **L3** | Yes — holds mode + context; mode type might rename |

**Done (Phase B Step 6):** `VehicleContext` and assemblies live in `vehicle_state/` — **imports L0, imported by L2/L3, not owned by L2**.

---

## Pinned boundaries (L5 / L6)

| Decision | Rule |
| -------- | ---- |
| **Decoders** | Stay in **L6** (CAN today; DBC / uProtocol / ProtoBuf later). Volatile wire format churn must not leak into L2/L3. |
| **Ingress vocabulary** | `PhysicalCarVocabulary` defined below L5, **surfaced through L5 facade**. Gateway depends on **one twin library**, not six. |
| **Wire crate** | `vehicle_device_bus` sits **beside** the twin stack (headlamp codec). Gateway may depend on L5 + wire crate + `socketcan` / `tokio`. |
| **Gateway ingress** | Never sends `FsmEvent` directly. CAN bytes → L6 decode → `PhysicalCarVocabulary` → `VehicleController`. |
| **Gateway tests** | Assert via **public diagnostics** and **`CarSnapshot`** first. Use `digital_twin` **`test` feature gate** only when diagnostics are insufficient. |

**L6 binaries and L5:**

| Binary | Needs L5? |
| ------ | --------- |
| **gateway** | Yes — full facade (`VehicleController`, ingress types) |
| **emulator** | No — L0 constants + CAN encode only |
| **front_headlamp_actuator** | No — `vehicle_device_bus` + CAN only |

---

## Packaging plan (agreed 2026-06-01)

Phased migration from today’s monolithic `common` crate toward compiler-enforced layers.

### Phase A — now (one crate, strict facade)

Keep `common` (rename to `digital_twin` when convenient). Enforce **L5 public surface** without splitting crates yet.

```text
digital_twin / common (one crate)
├── pub  facade/              ← gateway imports ONLY this (`common::facade`)
│     VehicleController, PhysicalCarVocabulary, CarSnapshot, …
├── pub(crate) fsm, engine, … ← internal; not for L6
└── integration tests inside crate; gateway e2e uses facade + snapshot/diagnostics
```

**Gateway `Cargo.toml` target:** `common` + `vehicle_device_bus` + I/O deps only — **no** `ractor`, **no** direct `fsm` / `engine` imports.

**Enforcement:** `scripts/check-gateway-facade-imports.sh` (run after gateway changes).

**Phase A done (2026-06-01):**

- [x] `common::facade` module — L5 public re-exports
- [x] Gateway source + tests import via `common::facade` only
- [x] Removed unused `ractor` from gateway `Cargo.toml`
- [x] Layering check script

**Phase A follow-up (optional, not blocking):**

- [ ] Rename `common` → `digital_twin` when convenient

Cycles **remain visible** in Phase A; they are **not** accepted as permanent.

### Phase B — next iteration (**mandatory cycle break + first split**)

This phase is **required**, not optional. It delivers what the pyramid exercise was for.

1. **Break all known module cycles** (see `design-notes-circular-dependencies.md`):
   - [x] Move `transition_map` from `engine/` → `fsm/transition_map.rs` (2026-06-01 — Step 1)
   - [x] Move `VehicleContext` + assemblies → `vehicle_state/` (2026-06-01 — Step 6)
   - [x] Delete shims: `engine/context/`, root `virtual_car_actor.rs`, `engine_namespace_contract` tests
   - [x] Rename `engine/` → **`twin_runtime/`** (2026-06-01 — Step 4)
   - Re-run circular-dependency inspection — expect **zero** cycles within the pure core.
2. **Split first independent library** (crate boundary — **decide when `sdv_core` lands**):

```text
sdv_core (L0–L2)       foundation + fsm — no actor, no tokio, no ractor
digital_twin (L3–L5)   depends on sdv_core only
vehicle_device_bus     unchanged
gateway (L6)           depends on digital_twin + vehicle_device_bus
```

3. Offline ledger verifier / CI law checks can depend on **`sdv_core` alone**.

**Phase B TODO (gateway tests — no code until Phase B):**

- [ ] **Gateway e2e → pure diagnostic assertions:** migrate `front_headlamp_e2e` away from
  `CarSnapshot` + `LightingState` + `FRONT_HEADLAMP_ON_ACK_WAIT` sleep choreography.
  Assert ACK/NACK/timeout outcomes via the public diagnostic stream (and `CarSnapshot` only
  where diagnostics are insufficient). Remove `LightingState` and `FRONT_HEADLAMP_ON_ACK_WAIT`
  from `common::facade` once tests no longer need them.

**Exit criterion:** `sdv_core` builds and tests with zero upward imports; static analysis reports no `fsm ↔ engine` triangle; **no shim or unused compatibility modules remain** in the twin stack.

### Phase C — actorification (split runtime when seams are real)

When PowerTrain / FrontHeadLamp become child Actors, revisit splitting L4 (internal modules or `digital_twin_runtime` crate). **Do not split L4 early** — actor boundaries should drive the cut.

---

## Open items (deferred to Phase B)

Not required for Phase A. Resolve when breaking cycles and extracting `sdv_core`.

- [ ] Confirm remaining thresholds (lux, speed, headlamp) get L2/L3 pairs
- [ ] Finalize L3 vs L3′ crate split for `published`
- [x] Move `VehicleContext` + assemblies to `vehicle_state/` (Step 6)
- [x] Delete shims: `engine/context/`, root `virtual_car_actor.rs`, `engine_namespace_contract` (Step 3)
- [x] Rename `engine/` → `twin_runtime/` (Step 4)
- [x] Move `transition_map` to `fsm/transition_map.rs`; delete `engine/op_strategy/` and `fsm/engine.rs`
- [ ] Break all module cycles + extract `sdv_core` (**required** — crate split decided at landing time)
- [ ] Gateway e2e → pure diagnostic assertions (see Phase B TODO above)
