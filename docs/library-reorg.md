# Library layout and layering (`common` pyramid)

**Purpose:** operational reference for how `crates/common` is organised, why, and how we
keep it acyclic. The README summarises this; **this document is the detail**.

**Historical Q&A:** [`design-notes-pyramid-layers.md`](design-notes-pyramid-layers.md) (decisions,
Phase A/B/C timeline, open items).

**Related ADRs:** [`adr-005-assembly-alphabet.md`](adr-005-assembly-alphabet.md) (L1 alphabets),
[`adr-006-twin-brain-ingress-coordination.md`](adr-006-twin-brain-ingress-coordination.md) (target
brain / ingress), [`adr-007-fsm-quiescence-and-cut.md`](adr-007-fsm-quiescence-and-cut.md) (cut,
quiescence, internal events).

---

## Why the pyramid exists

Early `common` had **module import cycles** (`fsm ↔ vehicle_state ↔ twin runtime`). Cycles
hide layering violations: lower layers start importing actors, I/O, or decision logic from above.

The pyramid exercise fixed that by assigning every module a **layer (L0–L6)** and an **acid
test**:

> Nothing imports from layers above it. The pure decision core never imports actors, CAN, or
> runtime sinks.

**TangleGuard** (see below) is the milestone gate that confirms the module graph stays acyclic
after each major change.

---

## Layer map (L0–L6)

```text
L0  vehicle_physics          constants, pure kinematics (no FSM, no I/O)
L1  vehicle_state            assemblies: powertrain, health, visibility, headlamp
                              VehicleContext aggregate; zone alphabets (ADR-5)
L2  fsm                      FsmState, FsmEvent, step(), transition_map — pure
L3  digital_twin, published  DigitalTwinCar capsule; serde ledger projection
L4  twin_runtime, sinks      VirtualCarActor, HeadlampActor, zone_turn, actuation
L5  facade                   gateway-facing public API (VehicleController, …)
L6  gateway, emulator,       wire adapters; never send FsmEvent directly
    front_headlamp_actuator
```

| Layer | Depends on | Must not depend on |
| ----- | ---------- | ------------------- |
| L0 | `std` | L1+ |
| L1 | L0 | L2, L3, L4, actors |
| L2 | L0, L1 | L3, L4, L5, L6, `ractor` |
| L3 | L0–L2 | L4, L5, L6 |
| L4 | L0–L3 | L5, L6 (wire) |
| L5 | L0–L4 | L6 |
| L6 | L5 (+ wire crate) | direct `fsm` / internal `twin_runtime` (gateway uses `facade` only) |

**Correctness constitution (same L0 constants everywhere):**

| Role | Layer | When |
| ---- | ----- | ---- |
| **Enforce** | L2 | `step` / transition — illegal cuts unreachable |
| **Announce** | L4 | diagnostics when clamped or rejected |
| **Detect** | L3 | `verify_state_laws` / `STATE_LAWS` — oracle for tests/CI/replay; never hot path |

---

## Module map in `common` today

From [`crates/common/src/lib.rs`](../crates/common/src/lib.rs):

| Module | Layer | Responsibility |
| ------ | ----- | -------------- |
| `vehicle_physics` | L0 | Thresholds, kinematics, headlamp/lux timing constants |
| `vehicle_state` | L1 | Per-assembly contexts; `HeadlampContext::on_receiving_message` |
| `domain_types`, `signals` | L1 | Ingress vocabulary helpers, VSS-inspired signals |
| `fsm` | L2 | Operational FSM table, `step`, internal operational events |
| `digital_twin` | L3 | `DigitalTwinCar`, mailbox vocabulary, state laws |
| `published` | L3′ | Serializable ledger mirror (`PublishedTransitionRecord`) |
| `twin_runtime` | L4 | Brain + headlamp twinlet, `zone_turn`, `twin_turn`, detectors |
| `diagnostic`, `transition_sink` | L4 | Sink traits (stdout / channels) |
| `facade` | L5 | Re-exports for gateway and integration tests |

**L6 binaries:**

| Binary | Depends on |
| ------ | ---------- |
| `gateway` | `common::facade`, `vehicle_device_bus`, I/O |
| `emulator` | L0 constants + CAN encode (no twin) |
| `front_headlamp_actuator` | `vehicle_device_bus` + CAN |

Enforcement: [`scripts/check-gateway-facade-imports.sh`](../scripts/check-gateway-facade-imports.sh).

---

## TangleGuard — acyclic module graph

**TangleGuard** is our name for the **zero-cycle** requirement on the `common` module graph.

### What was wrong (historical)

The blocking cycle was roughly:

```text
fsm → vehicle_state → fsm
```

(`vehicle_state` imported FSM types for headlamp direction/cause; L1 must not import L2.)

### What we did (M2, tag `pyramid-m2-complete`)

- Moved `transition_map` into `fsm/`.
- Moved assemblies + `VehicleContext` into `vehicle_state/` (L1 owns shapes; L2 consumes).
- Renamed `engine/` → `twin_runtime/`; removed shims (`engine/context/`, root re-exports).
- L4 demux: `zone_turn` → slim `fsm::step` → `twin_turn` / `commit_resolved_turn`.

**Status:** TangleGuard **clean** as of M2 (2026-06-03). Re-check at each milestone merge.

### How to verify

Run circular-dependency analysis on `crates/common` (tooling name in ADR-5: **`check-circles`**
— use your IDE / `cargo-modules` / project script when available). **Gate:** tests green **and**
no new cycles before merging a milestone.

**Discipline between milestones:** design should already avoid upward imports; TangleGuard at
milestone end **confirms** — a new cycle means refactor, not “document and defer”.

---

## Packaging phases

| Phase | Status | Outcome |
| ----- | ------ | ------- |
| **A** — one crate, strict facade | Done | `common::facade`; gateway facade-only imports |
| **B** — cycle break + optional `sdv_core` split | Modules done; crate split **deferred** | Acyclic modules; tag `pyramid-m2-complete` |
| **C** — actorification | **In progress** | Headlamp twinlet on `main`; template for zone #2 |

**Deferred:** extracting `sdv_core` (L0–L2) as a separate crate — boundaries already hold as
modules; split when packaging pain justifies it.

---

## L1 zone pattern (ADR-5) and L4 demux

Each assembly owns:

| Alphabet | Example (headlamp) |
| -------- | ------------------ |
| `{Zone}State` | `HeadlampState` |
| `{Zone}Message` | `HeadlampMessage` (inputs) |
| `{Zone}Outcome` | `HeadlampOutcome` (zone egress) |
| `{Zone}ZoneReply` | `HeadlampZoneReply` (`ctx` + `outcomes`) |

L4 [`zone_turn`](../crates/common/src/twin_runtime/zone_turn.rs) demuxes `FsmEvent` → zone
messages. Actorified zones (headlamp) run L1 in the **child actor**; brain merges
[`ZoneReplies`](../crates/common/src/twin_runtime/zone_replies.rs) at commit.

---

## Where to read next

| Topic | Document |
| ----- | -------- |
| Headlamp actor milestone (completed) | [`milestone-actor-headlamp-scope.md`](milestone-actor-headlamp-scope.md) |
| Runtime observation / ledger design | [`design-notes-runtime-observation.md`](design-notes-runtime-observation.md) |
| Circular dependency archaeology | [`design-notes-circular-dependencies.md`](design-notes-circular-dependencies.md) (if present) |
