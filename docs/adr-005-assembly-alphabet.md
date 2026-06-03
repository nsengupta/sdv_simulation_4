# ADR-5 — L1 assemblies own State, Message, and Outcome alphabets

**Status:** `ACCEPTED` (design locked; implementation deferred)  
**Date:** 2026-06-01  
**Related:** `docs/design-notes-pyramid-layers.md`, ADR-3/4 in `docs/design-notes-runtime-observation.md`, [`adr-006-twin-brain-ingress-coordination.md`](adr-006-twin-brain-ingress-coordination.md) (target brain / ingress / power barrier)

---

## Context

Zone assemblies (Powertrain, Visibility, Headlamp, Health) represent **what the vehicle IS**
in each domain. The operational FSM, actors, gateway, and offline verifier are replaceable
machinery. Assembly **meaning** must survive without them.

Today, assemblies live under `vehicle_state/` (L1), but FrontHeadlamp still emits L2
`DomainAction` from assembly methods called inline from `fsm::step` — causing the remaining
TangleGuard cycle (`fsm → vehicle_state → fsm`). That coupling is **Step 1 monolith debt**,
not the target architecture.

This ADR locks the **alphabet model** for **`sdv_simulation_3`** (post–`sdv_simulation_2`).
Code changes follow the [implementation policy](#implementation-policy) below.

---

## Decision

### 1. Layer assignment

| Layer | Owns |
| ----- | ---- |
| **L0** `vehicle_physics` | Shared thresholds, pure kinematics |
| **L1** `vehicle_state/{zone}/` | Per-assembly **State**, **Message**, **Outcome**; zone law rows |
| **L1** `vehicle_state/mod.rs` | `VehicleContext` — aggregate of zone **State** snapshots only |
| **L2** `fsm` | Operational mode (`FsmState`), ledger events (`FsmEvent`), `step` / `transition_map` |
| **L3** `digital_twin` | Capsule, `verify_state_laws` (composes L1 law catalogs) |
| **L4** `twin_runtime` | Demux: ingress → `{Zone}Message`; `{Zone}Outcome` → actuation / diagnostics / `FsmEvent` |
| **L5** `facade` | Gateway-facing re-exports from L1 + controller API |
| **L6** | Wire adapters (CAN, future uProtocol / ProtoBuf) |

**Pyramid stricture:** L1 assemblies **must not** import L2, L4, or sibling assemblies.
Cross-zone coupling only via **L4 orchestration** or **aggregate reads** (see Health).

Operational mode (`Off` / `Idle` / `Driving` / …) is **L2**, not assembly-local state.
Assembly-local modes (e.g. `PowertrainMode`, headlamp request states) stay in L1.

### 2. Assembly alphabet (three types per zone)

Each assembly defines assembly-owned vocabulary (rename from today’s FSM-centric names):

| Type | Role | Example (Headlamp) |
| ---- | ---- | -------------------- |
| `{Zone}State` | Snapshot — what the zone **IS** | `HeadlampState` (today `LightingState`) |
| `{Zone}Message` | Inputs — future actor mailbox alphabet | `HeadlampMessage::AmbientLux(u16)`, `AckOn`, `TimerTick`, … |
| `{Zone}Outcome` | Outputs for the world to translate — **not** `DomainAction` | `HeadlampOutcome::RequestOn`, `LogWarning(…)` |

L4 maps `{Zone}Outcome` → `ActuationCommand`, diagnostic lines, and summarized `FsmEvent`
values. L2 **must not** call assembly methods inline in the long term; it consumes **summaries /
outcomes / notifications** only (target architecture).

### 3. Cross-assembly data flow

**Now (monolith / single parent actor): parent fan-out (A).**

One ingress fact (e.g. ambient lux) is translated at L4, then the parent applies:

```text
VisibilityMessage::AmbientLux(lux)  →  visibility state updated
HeadlampMessage::AmbientLux(lux)    →  headlamp zone updated
```

Visibility remains a **dumb store**. Headlamp owns policy (hysteresis, ACK-wait). No
`visibility` → `headlamp` import.

**Later (zone child actors): pub/sub via L4 router (C).**

```text
VisibilityActor  --VisibilityNotification{lux}-->  L4 router
                                                      |
HeadlampActor    <--- HeadlampMessage::AmbientLux ----+
```

(C) is **not** an L1 import between assemblies. Each zone publishes/consumes **its own**
alphabet; L4 translates between them.

**Rule:** Ingress is always translated into per-zone **Messages** at L4. Zones never consume
each other’s alphabets directly.

### 4. Published types and facade

`published` and `facade` re-export headlamp (and other zone) types from **`vehicle_state`**,
**not** from `fsm`. Temporary `fsm` re-exports (e.g. `LightingState`) are legacy; remove in
the implementation phase.

### 5. Laws and invariants (extends ADR-3)

Each zone **contributes** law catalog rows at L1:

```text
Row: { name, L0 constant(s), zone enforce site, L3 detect predicate }
```

- **L3 detect:** `digital_twin::verify_state_laws` **composes** zone catalogs (offline oracle).
- **L2 enforce:** operational FSM + `transition_map`; zone-local rules inside zone handler/actor.
- **L4 announce:** diagnostics when clamped/rejected.

Assembly-owned law rows are required for the L1 alphabet model. **Actorification amplifies**
the need (rules scatter across actors) but is **not** a prerequisite to define catalogs in L1.

### 6. Health — hybrid (H3), ingress complexity-gated

Health is **not** a full actuation-style zone like Headlamp.

| Aspect | Decision |
| ------ | -------- |
| **Stored sensors** | `HealthState` / fields in `VehicleContext` (fuel, oil, tyre) |
| **`HealthMessage` / CAN ingress** | **Conditional** — add only if overall software complexity budget allows new ingress messages; otherwise keep static defaults until a later iteration |
| **`is_healthy()` / PowerOn gate** | **Derived predicate** over the aggregate `VehicleContext` (and stored health fields when present), not headlamp-style outbound outcomes |

Health is unlikely to become a heavy child actor unless sensor logic grows materially.

### 7. Known Step 1 debt (accepted until pyramid demux on `sdv_simulation_3`)

- Inline headlamp calls from `fsm::step` with `DomainAction` out-param.
- TangleGuard: one unique cycle `fsm → vehicle_state → fsm` (reported twice).
- ~~`LightingState` under `fsm::machineries`~~ — **done (milestone 1):** `HeadlampState` +
  `{Zone}Message` / `{Zone}Outcome` in `vehicle_state/`; facade exports L1 headlamp state.

**Do not** invest in `HeadlampEffect` → `DomainAction` bridge in this repo; L4 demux
(milestone 2) supersedes it.

---

## Open questions

| ID | Question | Blocks |
| -- | -------- | ------ |
| **Q5** | After headlamp is a child actor, does `VehicleContext` **embed** full `HeadlampState`, hold a **snapshot cache**, or only a **handle**? | L3 snapshot semantics, `GetStatus`, journey laws on headlamp fields |
| **Health ingress** | Introduce `HealthMessage` from CAN when complexity allows | Gateway ingress surface, health actor (if any) |

---

## Consequences

### Positive

- Clear `sdv_core` boundary on paper: L0–L2 + L1 alphabet types; no L4 in core.
- Zone actors reuse `{Zone}Message` verbatim as mailbox vocabulary.
- Facade and blog narrative: “assemblies tell the world their alphabet; the world adapts at L4.”

### Negative / accepted

- Step 1 code remains cycle-bearing until pyramid demux (milestone 2) on sim_3.
- Renaming (`LightingState` → `HeadlampState`, etc.) deferred with code; docs use target names.

### Supersedes / defers

- Minimal cycle-break plan (`HeadlampEffect` in monolith) — **deferred / superseded** by this ADR.
- TangleGuard zero-cycle target — **not a blocking gate** between milestones; run at milestone
  completion for verification (see [Layering discipline](#layering-discipline)).

---

## Layering discipline

Zero cycles is a **desired outcome** of alphabet + demux work, **not** a prerequisite to keep
designing or branching. The known Step 1 cycle is accepted until pyramid demux removes it.

**During design and every milestone**, still aim to **prevent new cycles** and keep coupling
low and cohesion high:

| Do | Avoid |
| -- | ----- |
| L1 zones import L0 only; expose State / Message / Outcome | L1 importing L2 (`DomainAction`, `FsmEvent`) or L4 |
| Cross-zone data via L4 fan-out or router (ADR-5 §3) | Sibling assembly imports; “just this once” back-edges |
| L2 imports L1 types for ledger/orchestration | L2 calling zone internals inline (target: summaries only) |
| New ingress → L4 demux → `{Zone}Message` | Gateway or zones speaking each other’s wire or FSM types directly |
| Per-zone law rows composed at L3 | Scattered thresholds with no L0 pairing |

When adding a module or milestone feature, ask: **does this import only from layers below?**
If not, stop and route through L4 or move the type down a layer.

TangleGuard at milestone completion **confirms** what design should already respect; a new cycle
report is a signal to refactor before merge, not merely to note and ship.

---

## Implementation policy

Design is locked in this ADR and cross-linked docs. **Code is a separate phase.**

| Rule | Detail |
| ---- | ------ |
| **Branches** | One branch per **primary milestone**; branch name reflects the milestone (e.g. `milestone/actorification-headlamp-alphabet`, `milestone/sdv-core-split`) |
| **Tests** | Strict: all tests passing on every milestone branch, always |
| **Docs** | Update design docs on each milestone (feeds README / blog later) |
| **Merge** | Milestone branches merge to `main` when the milestone is complete — not ad hoc |
| **TangleGuard** | Run `check-circles` **when a milestone completes** (with tests green). Design should already avoid **new** cycles (see [Layering discipline](#layering-discipline)); a new report before merge means refactor, not defer |

Suggested milestone order on **`sdv_simulation_3`** (pyramid on `main` until ~clean; actor work after gate; adjust as needed):

1. L1 alphabet modules (State / Message / Outcome) + re-exports; no behavior change.
2. L4 demux; remove inline `step` → headlamp calls; `{Zone}Outcome` mapping.
3. Zone child actors (Headlamp first; Powertrain / others follow).
4. `sdv_core` crate split (boundary per this ADR + `design-notes-pyramid-layers.md`).
5. Facade / published import paths; gateway e2e diagnostic assertions.

---

## References

- `docs/design-notes-pyramid-layers.md` — L0–L6 pyramid, Phase C actorification
- `docs/design-notes-circular-dependencies.md` — original cycle analysis
- `docs/design-notes-runtime-observation.md` — ADR-3 (enforce / announce / detect), ADR-4 (sole mutator), child-actor rules
