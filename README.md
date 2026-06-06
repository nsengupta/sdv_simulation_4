# SDV simulation 3 (`sdv_simulation_3`)

**Operational truth for this repository.** Blog posts and `blog-inputs/` are supplementary
narrative — they may lag or simplify; when in doubt, trust this file and the linked `docs/`.

---

## Lineage and iterations

| Iteration | Repository | Role |
| --------- | ---------- | ---- |
| 1 | [`sdv_simulation_1`](https://github.com/nsengupta/sdv_simulation_1) | Control loop works |
| 2 | [`sdv_simulation_2`](https://github.com/nsengupta/sdv_simulation_2) | Decompose, observe, prove (frozen) |
| 3 | **`sdv_simulation_3` (this repo)** | Pyramid cleanup → actorification |

Cloned from sim_2; umbilical cut — sim_2 stays the comparison tree.

---

## What this is

A Rust workspace prototyping a **software-defined vehicle control path**: telemetry and actuation
on a **shared CAN bus**, a **gateway** that projects wire traffic into twin vocabulary, and a
**digital twin** that maintains vehicle state, decides when to actuate, and closes the loop when
the body ECU acknowledges (or fails).

Educational / demonstrator — not a product stack.

---

## What is on `main` now

**Runtime (unchanged user-visible shape):** three processes on SocketCAN (`vcan0`) — emulator,
gateway (twin + CAN I/O), front-headlamp actuator.

**Pyramid (module layers in `common`):** L0–L6 layout, ADR-5 zone alphabets, L4 demux /
`zone_turn` → `fsm::step`, **TangleGuard clean**. Tagged `pyramid-m2-complete`. Detail:
[`docs/library-reorg.md`](docs/library-reorg.md).

**Headlamp actorification (merged):** the **headlamp assembly** is an independent
**`HeadlampActor`** — a [`ractor`](https://crates.io/crates/ractor/0.15.12) child of
**`VirtualCarActor`** (brain). Brain and assembly communicate only through **enumerated mailbox
types** (tell / tell-back / zone deadlines), not shared in-process L1 on the production path.
Commit uses **`commit_resolved_turn`** → **`run_to_quiescence`** (ADR-7); zone tell-back embeds
live in **`ZoneReplies`**. The twinlet owns **ACK wait** via `send_after` and reports timeout via
**`HeadlampZoneSpontaneous`**. Milestone handoff (historical):
[`docs/milestone-actor-headlamp-scope.md`](docs/milestone-actor-headlamp-scope.md).

**Still ahead:** zone #2 twinlet (copy headlamp template), ADR-6 power barrier, actuation child
actor, offline ledger verifier, optional `sdv_core` crate split.

---

## Runtime shape

| Process | Role |
| ------- | ---- |
| **emulator** | Publishes engine RPM and ambient lux on CAN (~10 Hz). |
| **gateway** | CAN ingress/egress, projection, **`VirtualCarActor`** + **`HeadlampActor`**, actuation CMD TX. |
| **front_headlamp_actuator** | Body ECU stand-in: CMD → ACK/NACK (~150 ms). |

Inside the gateway, **`VirtualCarActor`** is the brain mailbox. **`HeadlampActor`** is the first
zone twinlet. Outcomes appear on **stdout** (stand-in for dashboard / cloud).

Signals use a **VSS-inspired** enum (`EngineRpm`, `AmbientLux`; `VehicleSpeed` decoded but not
fed to the twin). Wire layout: **`vehicle_device_bus`**.

### Demo screenshots

**Daylight (`lux ≥ 860`):** headlamp stays off; no CMD/ACK traffic.

![Gateway run in daylight — headlamp off](assets/runtime-screenshot-no-headlamp.png)

**Tunnel (`lux ≤ 840`):** twin requests ON → CMD on CAN → actuator ACK/NACK/timeout.

![Gateway run in low lux — headlamp ON requested and ACK'd](assets/runtime-screenshot-with-headlamp.png)

---

## Architecture

![SDV prototype — emulator, gateway, actuator, vcan0](assets/SDV-Blog-Inputs-4.jpg)

**Ingress:** CAN → `PhysicalCarVocabulary` → `PhysicalToDigitalProjector` →
`DigitalTwinCarVocabulary::Fsm` → **brain** → (if demux routes headlamp) **tell `HeadlampActor`**
→ tell-back → **`commit_resolved_turn`** / `fsm::step`.

**Egress:** `DomainAction` → `DefaultActuationManager` → `ActuationCommand` → CAN CMD → actuator
→ ACK/NACK → ingress again as `FrontHeadlampOnAck` / incomplete events.

### Twin state by assembly (zone)

```text
VehicleContext
├── powertrain : PowertrainContext    // WheelRpm, derived speed, mode
├── health     : VehicleHealthContext
├── visibility : VisibilityContext    // ambient lux
└── headlamp   : HeadlampContext      // HeadlampState, ACK-wait
```

L1 behaviour: `{Zone}Context::on_receiving_message` → `{Zone}ZoneReply` (ADR-5). L2 **`fsm::step`**
runs **after** L4 **`zone_turn`** merges zone outcomes. Powertrain, health, visibility are still
**in-process** at demux; headlamp runs in **`HeadlampActor`** on the actor path.

> **Embed (phase A):** after tell-back, the brain copies **`HeadlampZoneReply.ctx`** into
> `VehicleContext.headlamp` — it does not call L1 in parallel with the child. Toward phase C the
> embed may shrink to a handle; tests (ledger / `GetStatus`) surface gaps.

### Brain ↔ headlamp — enumerated mailboxes

| Direction | Message |
| --------- | ------- |
| Brain → headlamp | `HeadlampActorMsg::Apply(HeadlampActorVocabulary { message, turn_id, tell_attempt, … })` |
| Headlamp → brain (correlated) | `DigitalTwinCarVocabulary::HeadlampZoneReady { turn_id, tell_attempt, reply }` |
| Headlamp → brain (ACK deadline) | `HeadlampZoneSpontaneous` → commit as `FrontHeadlampActuationIncomplete` |
| Brain (silent twinlet) | `TellBackTimeout` → retry tell → synthetic embed |

**Tell** is fire-and-forget; the brain mailbox stays free until tell-back or tell-back timeout.

### Commit contract (quiescence)

One **FSM ingress** (or spontaneous zone message) → one **quiescent commit**:

1. Receive ingress or zone tell-back.
2. If demux routes headlamp: tell twinlet → wait for **`HeadlampZoneReady`** (deadline + retries).
3. **`commit_resolved_turn`** → **`run_to_quiescence`** (zone merge + `step` + optional internal hops from **detectors**).
4. **`apply_committed_quiescence`**: **one ledger row per hop** → single **`apply_step`** on final cut → merged actuation.

Example (driving in dark, ACK never arrives): hop 1 = incomplete embed (lamp `Off`); detector
synthesizes `Internal(LightingUnsafe)`; hop 2 = **`DrivingDangerously`** + buzzer.

**Actuation nuance:** `RequestFrontHeadlampOn` does not settle the lamp in the action phase —
lux moves to `OnRequested`; `On` only after a later ACK ingress.

Implementation: [`twin_turn.rs`](crates/common/src/twin_runtime/twin_turn.rs),
[`virtual_car_actor.rs`](crates/common/src/twin_runtime/controller/virtual_car_actor.rs).

---

## Library layout (summary)

`common` is a **layered pyramid** inside one crate (L0–L5); gateway/emulator/actuator are L6.
The acid test: **no upward imports**; the pure FSM never sees actors or CAN.

| Layer | Modules (today) | Notes |
| ----- | ----------------- | ----- |
| L0 | `vehicle_physics` | Shared thresholds |
| L1 | `vehicle_state`, `signals` | Zone contexts + alphabets |
| L2 | `fsm` | Pure `step`, transition table |
| L3 | `digital_twin`, `published` | Twin capsule + ledger mirror |
| L4 | `twin_runtime`, sinks | Brain, headlamp twinlet, actuation |
| L5 | `facade` | Gateway public API |

**TangleGuard:** module graph must stay **acyclic**; verified at milestone merges. Full layer
rules, history, and verification: **[`docs/library-reorg.md`](docs/library-reorg.md)**.

Gateway imports **`common::facade` only** — see `scripts/check-gateway-facade-imports.sh`.

---

## Observability

| | `transition_tx` — fact ledger | `diagnostic_tx` — presentation |
| --- | --- | --- |
| Delivery | bounded, lossless-or-error | unbounded, best-effort |
| Ordering | total by `record_seq` | none guaranteed |
| Cadence | one **`PublishedTransitionRecord` per quiescence hop** | many (init, ticks, meta) |
| Audience | replay, invariant checks | humans / stdout |

Twin emits via **`TransitionRecordSink`** and **`DiagnosticSink`** traits (not raw channels).
Records carry **intended** `DomainAction`s; outcomes (ACK, timeout) are separate ingress facts.
**`GetStatus`** returns `CarSnapshot { as_of_seq }` — snapshots are *as-of* a ledger sequence.

### Sink injection at runtime init (not per emission)

**Who picks stdout vs discard is decided once**, when the gateway constructs
`VehicleControllerRuntimeOptions` and starts the twin — not inside the actor on every log line.

```text
CLI flags  →  gateway `run()` wires channels + observer/drainer tasks  →  twin gets sinks (or not)
```

The twin only knows whether a sink was **injected**. Optional `if let Some(sink)` guards in
`VirtualCarActor` are the **mechanical consequence** of “runtime did not inject a sink” — they
are not routing policy and not ledger-mode conditionals. When no sink is wired, the actor skips
building the message (no allocation, no channel traffic). When a sink is wired, the runtime
owns what happens on the RX side (stdout printer, future file/GUI, etc.).

| Gateway launch | `diagnostic_tx` → twin | `transition_tx` → twin | Typical stdout |
| --- | --- | --- | --- |
| default | wired → stdout observer | not wired | diagnostics only |
| `--print-transitions-only` | **not wired** | wired → coloured ledger task | ledger rows only |

Ledger-only also suppresses actuation ingress log lines and most startup banners — again
gateway init choices, not twin policy. CAN actuation (`actuation_command_tx`) stays wired in
all modes.

See **`blog-inputs/episode-02-runtime-wiring-and-actuation-path.md`** (§6) and
**`docs/design-notes-runtime-observation.md`** for the full observation design.

---

## Correctness model

1. **Enforce** in L2 — illegal operational cuts rejected/clamped.
2. **Announce** via L4 diagnostics when clamped.
3. **Detect** with L3 **`verify_state_laws`** / **`STATE_LAWS`** — oracle only, never hot path.

**`DigitalTwinCar`** is correct-by-construction: private fields, checked `new`, single
**`apply_step`** mutator after FSM commit.

---

## Vehicle states

**Operational FSM (`FsmState`):**

| State | Meaning |
| ----- | ------- |
| **`Off`** | Ignition off; speed frozen 0; lighting cleared. |
| **`Idle`** | Powered, RPM ≤ 1000. |
| **`Driving`** | RPM > 1000, moving. |
| **`ExtremeOperationWarning`** | Speed/RPM stress band; 5 s cooldown to exit. |
| **`DrivingDangerously`** | Driving in dark without confirmed headlamp ON (latched); buzzer until recovery. |

```text
Off ──PowerOn──► Idle ◄──stationary── Driving ◄────────────────────────┐
                  ▲                    │                                  │
                  │                    ├── stress ──► ExtremeOperationWarning
                  │                    │                                  │
                  │                    └── dark + failed lamp ──► DrivingDangerously
                  │                         (Internal LightingUnsafe)     │
                  └──────────────── recovery (lamp ON, bright lux, idle) ─┘
```

**Headlamp (`HeadlampState`):** orthogonal sub-state — `Off` → `OnRequested` → `On` →
`OffRequested` → … — not extra top-level FSM modes.

### Anti-flap

| Boundary | Mechanism |
| -------- | --------- |
| Lux → headlamp | Value deadband: ON at `lux ≤ 840`, OFF at `lux ≥ 860`, hold between |
| Speed → warning | Temporal latch: enter over 160 km/h; exit after ≥ 5 s **and** hazard cleared (`TimerTick`) |
| Headlamp ACK | Actor-owned timer in **`HeadlampActor`** (not gateway tick on actor path) |

---

## Iteration 2 improvements (vs sim_1)

| Area | sim_1 → sim_2+ |
| ---- | -------------- |
| Twin context | Flat struct → per-assembly **`VehicleContext`** |
| FSM | Monolith → orchestrator + assembly behaviour |
| Ledger | State delta → actions + **`published`** mirror + **`as_of_seq`** |
| Invariants | Inline checks → **`STATE_LAWS`** oracle |
| Twin mutation | Public fields → **`apply_step`** only |
| Logging | Sync on hot path → bounded diagnostic side-channel |

---

## Software map

| Crate | Role |
| ----- | ---- |
| [**`common`**](crates/common/) | L0–L5 pyramid; brain + headlamp actor; FSM; facade |
| [**`vehicle_device_bus`**](crates/vehicle_device_bus/) | Headlamp CAN codec |
| [**`emulator`**](crates/emulator/) | RPM/lux world model |
| [**`gateway`**](crates/gateway/) | CAN loop, twin install, timer tick |
| [**`front_headlamp_actuator`**](crates/front_headlamp_actuator/) | CMD/ACK/NACK loop |

| What | Path |
| ---- | ---- |
| FSM table | [`fsm/transition_map.rs`](crates/common/src/fsm/transition_map.rs) |
| Headlamp L1 | [`vehicle_state/front_headlamp.rs`](crates/common/src/vehicle_state/front_headlamp.rs) |
| Headlamp twinlet | [`twin_runtime/headlamp_actor.rs`](crates/common/src/twin_runtime/headlamp_actor.rs) |
| Brain | [`twin_runtime/controller/virtual_car_actor.rs`](crates/common/src/twin_runtime/controller/virtual_car_actor.rs) |
| State laws | [`digital_twin/car_behaviour_checker.rs`](crates/common/src/digital_twin/car_behaviour_checker.rs) |

---

## Wire protocol (reference)

| Signal | CAN ID | Twin ingress |
| ------ | ------ | ------------ |
| Vehicle speed | `0x101` | Decoded, **not** fed (RPM-derived speed) |
| Engine RPM | `0x102` | `UpdateRpm` |
| Ambient lux | `0x103` | `UpdateAmbientLux` |
| Front headlamp | `0x204` | CMD egress; ACK/NACK ingress |

---

## Tests

```bash
cargo test -p common
cargo test -p gateway --lib
cargo test -p vehicle_device_bus
cargo test --workspace
cargo test -p common --features proptest   # optional
```

With `vcan0` up: `cargo test -p gateway --test front_headlamp_e2e`.

Contract tests live under `crates/common/src/test/` (quiescence, zone replies, ACK timer,
headlamp reply, operational policy, …).

---

## How to run (Linux)

```bash
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan 2>/dev/null || true
sudo ip link set up vcan0
```

Three terminals: `cargo run -p emulator`, `cargo run -p front_headlamp_actuator`, `cargo run -p gateway`.

Useful gateway flags (combine as needed):

```bash
cargo run -p gateway                              # diagnostics on stdout (default)
cargo run -p gateway -- --print-transitions-only  # coloured ledger on stdout; no diagnostic sink
cargo run -p gateway -- --print-timer-tick        # disabled in ledger-only mode
cargo run -p gateway -- --trace-actuation-ingress # disabled in ledger-only mode
```

Each flag only changes **which sinks the gateway injects at startup** — see Observability above.
There is no mixed “ledger + diagnostics on stdout” mode; pick default or ledger-only.

Actuator demo env: `FRONT_HEADLAMP_ACTUATOR_DROP_RESPONSE_PROB`, `FRONT_HEADLAMP_ACTUATOR_ACK_NACK_RESPONSE_PROB`.
Emulator: `EMULATOR_TUNNEL_PROB` (tunnel frequency). See prior README tables for values.

---

## Documentation index

| Document | Use when |
| -------- | -------- |
| **[`docs/library-reorg.md`](docs/library-reorg.md)** | Pyramid layers, TangleGuard, module map |
| [`docs/design-notes-pyramid-layers.md`](docs/design-notes-pyramid-layers.md) | Historical layering Q&A |
| [`docs/design-notes-runtime-observation.md`](docs/design-notes-runtime-observation.md) | Ledger, cut, observation design |
| [`docs/adr-005-assembly-alphabet.md`](docs/adr-005-assembly-alphabet.md) | Zone alphabets |
| [`docs/adr-006-twin-brain-ingress-coordination.md`](docs/adr-006-twin-brain-ingress-coordination.md) | Target brain / power barrier |
| [`docs/adr-007-fsm-quiescence-and-cut.md`](docs/adr-007-fsm-quiescence-and-cut.md) | Quiescence, internal events |
| [`docs/milestone-actor-headlamp-scope.md`](docs/milestone-actor-headlamp-scope.md) | Completed headlamp milestone handoff |

---

## Roadmap

| Track | Status |
| ----- | ------ |
| Pyramid + TangleGuard | Done (`pyramid-m2-complete`) |
| Headlamp twinlet template | Done on `main` |
| Zone #2 actor, ADR-6 power barrier | Next |
| Ledger actor, correlation IDs, actuation child | Planned (WI-8–13) |
| Offline ledger verifier | Designed, unbuilt |
| `sdv_core` crate split | Deferred (modules sufficient) |

---

*Update this README when behaviour or layout changes. Blog narrative follows milestones; it does not lead them.*
