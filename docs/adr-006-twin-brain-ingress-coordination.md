# ADR-6 — Twin brain, ingress vocabulary, and power coordination

**Status:** `ACCEPTED` (target architecture; implementation phased)  
**Date:** 2026-06-03  
**Series:** Architecture decision records — follows [`adr-005-assembly-alphabet.md`](adr-005-assembly-alphabet.md)  
**Related:** [`design-notes-pyramid-layers.md`](design-notes-pyramid-layers.md), [`design-notes-runtime-observation.md`](design-notes-runtime-observation.md)

**Audience:** README / blog source material; agent context for TODOs on `sdv_simulation_3`.

---

## Context

Milestone 1 (ADR-5) placed **L1 alphabets** (`{Zone}State`, `{Zone}Message`, `{Zone}Outcome`).
The remaining monolith debt is **where turns are orchestrated** and **what the controller
sends the twin**. This ADR records the **target** shape: one **brain** actor, assembly
**twinlets**, controller-only addressing, and a **replay-complete ledger** — without
multiplying doc files.

Implementation may lag (pyramid demux on `main` first); this document is the **north star**.

---

## Decision summary

| Topic | Choice |
| ----- | ------ |
| Controller knows | **Brain actor only** (prototype) |
| Ingress packaging | **B1** — one `send` per external fact, **envelope** to brain |
| Mailbox vocabulary | **B** — per-zone messages; nested **`TwinIngress`** enum |
| Zone apply vs FSM | Brain dispatches to twinlets, then **operational FSM** when needed |
| Assembly → brain replies (PowerOn/Off) | **R1** — separate ingress messages (e.g. `*Complete`); revisit R2 only if sync gain is real |
| Power coordination evidence | **S2** — assemblies **reply**; not snapshot-only guards |
| Correlation | **C1** — one coordination in flight; **`BTreeSet<CoordinationParticipant>`** `pending` |
| Barrier state | **S2** — `pending` lives **beside** `FsmState`, not inside fat enum variants |
| Ignored ingress while waiting | **I1** — do not apply; **D2/L1** — ledger row with **`applied: false`** + reason |
| PowerOff participants (first) | **P2** — **Headlamp + Powertrain** (not Visibility) |
| Visibility + power | Visibility is a **lux store**; not in PowerOn/Off barrier |

---

## Layered picture (pyramid)

```text
L6  Controller / gateway
      wire → TwinIngress (and future batches)
      inner outcomes → CAN / uProtocol / diagnostics / ledger projection

L4  Twin brain (actor, single-threaded)
      dispatch TwinIngress → twinlets
      coordination barrier (pending set)
      operational FSM (table-driven mode)
      map zone outcomes → egress channels (via controller wiring)

L3  DigitalTwinCar capsule — summated view (VehicleContext + FsmState)

L1  Twinlets / assemblies — zone state + Message/Outcome alphabet
      actuator zones emit outcomes; brain does not call L1 via L2 DomainAction
```

**Controller** owns outer vocabulary and channels (“tell me what to do through this pipe”).
**Brain** owns inner vocabulary and coordination. **No CAN policy inside the brain** — only
mapped facts (`TwinIngress`) and actuation **results** (e.g. ACK as `HeadlampMessage::AckOn`).

---

## `TwinIngress` (enum of enums)

```rust
// Target shape (illustrative)
pub enum TwinIngress {
    Powertrain(PowertrainMessage),
    Visibility(VisibilityMessage),
    Headlamp(HeadlampMessage),
    Brain(BrainMessage),
}

pub enum BrainMessage {
    PowerOn,
    PowerOff,
    TimerTick,
    SystemReset,
    // grow: operator/HMI triggers mapped by controller
}
```

**Fan-out example (one lux frame):** controller sends **one envelope** to the brain, e.g.
`[Visibility(AmbientLux(lux)), Headlamp(AmbientLux(lux))]`. Brain dispatches; visibility
stays a dumb store; headlamp runs policy.

---

## One turn (B1)

```text
1. Controller → Brain: TwinIngress (or batch)
2. If in WAITING_* and ingress not allowed → ledger(applied=false); return (I1 + D2)
3. Else dispatch to twinlets → update L1 state; collect zone outcomes
4. Operational FSM step when event affects global mode (table-driven)
5. If coordination pending empty → commit mode transition
6. Map outcomes + brain actions → controller egress (actuation, diag, published record)
7. apply_step on capsule; advance record_seq
```

**In-place mutation** in the actor is fine (single-threaded); clones today serve pure
`step` tests and `old_ctx`/`current_ctx` audit — not a hard requirement.

---

## PowerOn / PowerOff (brain actuations)

- PowerOn/PowerOff are **`BrainMessage`** — meaning is **coordination**, not a zone lux tick.
- Brain enters thin FSM modes (e.g. `WaitingAckForPowerOff`) and holds:

```rust
// Beside FsmState (S2)
coordination: Option<PowerBarrier> {
    phase: PowerPhase::Off | On,
    pending: BTreeSet<CoordinationParticipant>,  // P2: Headlamp, Powertrain
}
```

- Brain dispatches **prepare** messages to twinlets (zone-private vocabulary).
- Each zone answers with **R1** ingress, e.g. `HeadlampMessage::PowerOffComplete`.
- On each complete: remove from `pending`; ledger **applied: true**.
- When `pending.is_empty()`: FSM commits **Off** (or **On**).
- **Duplicate** complete: idempotent remove.
- **Visibility** does not participate in power barrier unless a future ADR adds a rule.

**Replay (C1):** total order on `record_seq` + per-row `applied` + FSM/coordination snapshot
on applied rows is enough; correlation IDs deferred until overlapping coordinations exist.

---

## Ledger row (L1 — D2)

Extend the transition / published record (exact field names TBD at implementation):

| Field | Role |
| ----- | ---- |
| `ingress` | `TwinIngress` (or serialized mirror) |
| `applied` | `true` if state/mode advanced; `false` if suppressed during `WAITING_*` |
| `suppressed_reason` | e.g. `CoordinatingPowerOff` when `applied == false` |
| `fsm_state` / `coordination` | enough to replay barrier progress |

Suppressed rows keep **`old_ctx == current_ctx`** (no silent drops).

---

## Leakage rule (brain actions only)

- **L1** emits **`{Zone}Outcome`**, never `DomainAction`.
- **L2 FSM** emits only **brain actions** (buzzer, mode hints, brain-level warnings).
- **L4** maps outcomes → actuation/diagnostics; controller maps to wire.
- **Leakage** returns if L2 calls zone `apply` or L1 imports L2 types.

Reading assembly **state** in FSM guards (e.g. health for PowerOn) is orchestration, not leakage,
if transitions stay in the table.

---

## Adding an assembly to power coordination (checklist)

Use this when extending **P2** beyond Headlamp + Powertrain (blog/README how-to):

1. **L1** — define `{Zone}Message` / `{Zone}Outcome`; add `PreparePowerOff` and `PowerOffComplete` (R1).
2. **Participant** — add `CoordinationParticipant::{Zone}` to the `BTreeSet` for PowerOff/On.
3. **Controller** — map any new wire facts → `TwinIngress::{Zone}(...)`.
4. **Brain** — dispatch prepare on `BrainMessage::PowerOff`; handle `*Complete` only in `WAITING_*`.
5. **FSM** — new transitions in `transition_map`; avoid ad-hoc `if` chains in the actor.
6. **Ledger** — extend published mirror; replay fixture with one suppressed + one complete event.
7. **Egress** — controller translates new outcomes (if actuator).

---

## Implementation phasing (TODO on `sdv_simulation_3`)

| Phase | Scope | Status |
| ----- | ----- | ------ |
| **M1** | L1 alphabets (ADR-5) | Done on `main` |
| **M2** | Demux: twinlets without `fsm → vehicle_state` `DomainAction`; optional `TwinIngress` shim still from `FsmEvent` | Next pyramid work |
| **M3** | Child actors (headlamp first) | After pyramid gate |
| **M4** | Full `TwinIngress` at controller; brain barrier + `applied` ledger | Target of this ADR |
| **M5** | Offline replay tool consuming `applied` + `ingress` | Future |

Do not block M2 on full power coordination; introduce `TwinIngress` and barrier incrementally.

---

## Consequences

**Positive:** Clear prototype story (controller → brain → twinlets); blog can show deterministic
coordination and replay; pyramid stays strict.

**Negative:** More types and ledger fields before replay tool exists; M2–M4 are staged work.

**Supersedes:** Informal “fresh clone” actorification handoff for **brain** design — active repo is
`sdv_simulation_3` (see ADR-5 implementation policy).

---

## References

- [`adr-005-assembly-alphabet.md`](adr-005-assembly-alphabet.md) — L1 alphabets, fan-out (A), demux
- [`design-notes-pyramid-layers.md`](design-notes-pyramid-layers.md) — L0–L6
- [`design-notes-runtime-observation.md`](design-notes-runtime-observation.md) — ledger, diagnostics, WI-*
