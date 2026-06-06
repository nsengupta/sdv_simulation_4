# SDV simulation 3 (`sdv_simulation_3`) — repository overview

---

**Repository:** `sdv_simulation_3` (local / GitHub name). **Lineage:** cloned from
[`sdv_simulation_2`](https://github.com/nsengupta/sdv_simulation_2) (Iteration 2 baseline);
umbilical to sim_2 is cut — sim_2 stays the frozen comparison tree.

**Blog / narrative iterations** (written afresh after repo milestones, not ahead of them):

| Iteration (blog) | Repository        | Role |
| ---------------- | ----------------- | ---- |
| 1                | `sdv_simulation_1` | Control loop works |
| 2                | `sdv_simulation_2` | Decompose, observe, prove (frozen) |
| 3                | **`sdv_simulation_3`** | Pyramid cleanup → actorification |

This README is the **current truth** for running and reading the code. Prior iteration:
**[Iteration 1 — README](https://github.com/nsengupta/sdv_simulation_1#readme)**.

---

A Rust workspace that prototypes a **software-defined vehicle control path**: telemetry and actuation on a **shared CAN bus**, a **gateway** that translates wire traffic into domain
events, and a **digital twin** that maintains vehicle state, decides when to actuate, and
closes the loop when the body ECU acknowledges (or fails).

This is an educational / demonstrator codebase, not a product stack.

---

## What this repository is doing now

**Inherited from Iteration 2 (sim_2):** same three processes, CAN wire, and user-visible
behaviour — zone assemblies, transition ledger, diagnostics, correct-by-construction twin.

**Pyramid (module layers in `common`) — complete on `main`:** L0–L6 layout, ADR-5 alphabets (M1),
L4 demux / `twin_turn` (M2), **TangleGuard clean**. Tagged: `pyramid-m2-complete` (see below).
Layer map: [`docs/design-notes-pyramid-layers.md`](docs/design-notes-pyramid-layers.md).
**Deferred:** `sdv_core` crate split (packaging only; same boundaries already hold as modules).

**ADR series:** `docs/adr-005-*.md` (L1 alphabets) → `docs/adr-006-*.md` (brain & ingress) → [`docs/adr-007-fsm-quiescence-and-cut.md`](docs/adr-007-fsm-quiescence-and-cut.md) (**cut**, internal FSM events, quiescence at commit).

**Actor track (branch `milestone/actor-headlamp`; blog Iteration 3):** the **headlamp assembly**
is an independent **`HeadlampActor`** — a ractor child of **`VirtualCarActor`** (brain). Brain and
assembly communicate only through **enumerated mailbox types** (tell / tell-back / zone deadlines),
not shared in-process L1 calls on the production path. Step-7b adds quiescent commit, `ZoneReplies`,
and actor-owned ACK timers. Handoff: [`docs/milestone-actor-headlamp-scope.md`](docs/milestone-actor-headlamp-scope.md).

**Still ahead on the actor track:** zone #2 twinlet, ADR-6 power barrier, actuation child actor.

**Not in scope yet:** full actorification of every zone; offline file-writer + folding verifier (designed, unbuilt).

---

## What's new since Iteration 1 (the short list)

| Area                  | Iteration 1                                           | Iteration 2                                                                                                                            |
| --------------------- | ----------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| **Twin context**      | one flat `VehicleContext` struct                      | aggregate of **per-assembly** contexts (`powertrain`, `health`, `visibility`, `headlamp`), each owning its data *and* behaviour        |
| **FSM `step`**        | ~238-line monolith with inline lighting/timeout logic | ~158-line **orchestrator**; the *how* lives on the assemblies, `step` decides *when*                                                   |
| **Powertrain**        | single `rpm: u16`                                     | `WheelRpm { fl, fr, rl, rr }` (broadcast for now) + derived `PowertrainMode` (exposed, not yet consumed)                               |
| **Transition record** | state delta only, in-process `Instant`                | carries **intended `DomainAction`s**; serializable **`published`** mirror (`Instant`→`Duration`-since-`UNIX_EPOCH`) for offline replay |
| **Sequencing**        | one `sequence_no`                                     | disambiguated **`record_seq`** (ledger) vs command-correlation seq; snapshot stamped with **`as_of_seq`**                              |
| **Invariants**        | `pub(super)` law fns called inline                    | public **`verify_state_laws`** + named **`STATE_LAWS`** catalog; an offline **oracle**, never a hot-path gate                          |
| **Twin construction** | public fields, free mutation                          | **correct-by-construction**: private fields, checked `new(...)`, single `apply_step(...)` mutator                                      |
| **Diagnostics**       | NACK/timeout only                                     | **silent-success ACK** now surfaced; `LogWarning` reclassified to the diagnostic sink                                                  |
| **Logging**           | synchronous on the protocol path                      | **bounded side-channel**, drop-on-full, off the hot path (a live `Ctrl-S`/XOFF freeze proved this mattered)                            |
| **Emulator**          | fixed tunnel cadence                                  | `EMULATOR_TUNNEL_PROB` env knob                                                                                                        |

**Deferred until pyramid gate, then actor track:** single-writer ledger actor, end-to-end
correlation IDs, diagnostics-as-projection, actuation child actor, actuation resilience
(retry/backoff/dedup) — see Roadmap and
[`docs/design-notes-runtime-observation.md`](docs/design-notes-runtime-observation.md).

---

## Runtime shape (unchanged from Iteration 1)

**Three processes** share Linux **SocketCAN** (`vcan0` by default):

| Process                     | Role                                                                                                                                    |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| **emulator**                | Stand-in for powertrain + ambient-light sensing: publishes **engine RPM** and **ambient lux** on CAN (~10 Hz).                          |
| **gateway**                 | Owns ingress (read CAN), **projection** into twin vocabulary, the **digital twin** runtime, and egress (write headlamp **CMD** frames). |
| **front_headlamp_actuator** | Stand-in body ECU: receives CMD, replies with **ACK** or **NACK** on the same bus (~150 ms later).                                      |

Inside the gateway, the twin is a **`VirtualCarActor`** ([`ractor`](https://crates.io/crates/ractor/0.15.12)
mailbox, single-threaded handling) driving a pure **FSM** plus zone assemblies. The **headlamp
zone** runs in a child **`HeadlampActor`**; the brain tell/tell-backs the twinlet over enumerated
vocabulary before committing ledger rows. Outcomes are visible on **stdout** — a deliberate stand-in for a dashboard or cloud stream.

Signals are modeled in a **VSS-inspired** Rust enum (`EngineRpm`, `AmbientLux`, and a
decoded-but-unused `VehicleSpeed` slot for a future observed-speed ECU). Payload layout and headlamp wire kinds live in **`vehicle_device_bus`** so the gateway stays thin and the actuator binary stays independent.

### Demo screenshots

Still frames from a live three-process run on `vcan0`. Both show the same gateway **stdout** surface — FSM transitions, RPM/lux telemetry, and (when lighting rules fire) headlamp actuation — under different ambient-light conditions.

**Daylight band (headlamp off)** — ambient lux stays above the OFF threshold (`LUX_OFF` 860).

The twin keeps `HeadlampState::Off`; the gateway log has no headlamp CMD or ACK/NACK lines.

![Gateway run in daylight — headlamp off, no actuation traffic](assets/runtime-screenshot-no-headlamp.png)

**Tunnel / dim band (headlamp on)** — lux falls through the ON threshold (`LUX_ON` 840), the twin requests ON, the gateway sends `📤🔆` CMD on CAN, and the actuator reply appears as `✅💡` (or `❌🔆` / `⏱️` on failure paths).

![Gateway run in low lux — headlamp ON requested and ACK'd](assets/runtime-screenshot-with-headlamp.png)

---

## Architecture

Telemetry and commands meet on one virtual CAN interface; the gateway is the only component that speaks both "wire" and "twin."

![SDV prototype — emulator, gateway (digital twin), front-headlamp actuator, and vcan0](assets/SDV-Blog-Inputs-4.jpg)

**Ingress path:** CAN frame → `VssSignal` / headlamp payload → `PhysicalCarVocabulary` → `PhysicalToDigitalProjector` → `DigitalTwinCarVocabulary::Fsm` → **`VirtualCarActor`** (brain) → tell **`HeadlampActor`** when demux routes a headlamp message → tell-back → `commit_resolved_turn` / `fsm::step`.

**Egress path:** `DomainAction` (e.g. request headlamp ON) → `DefaultActuationManager` →`ActuationCommand` → gateway encodes CMD → CAN → actuator → ACK/NACK → same reader thread →policy correlation → twin ACK/NACK/incomplete events.

### The twin's state is now organised by assembly (zone)

`VehicleContext` is no longer a flat bag of fields; it is an **aggregate of self-sufficient
assemblies**, each owning its own data *and* the rules over that data:

```text
VehicleContext
├── powertrain : PowertrainContext   // WheelRpm (4 wheels) + derived speed_kph + PowertrainMode
├── health     : VehicleHealthContext// fuel / oil / tyre
├── visibility : VisibilityContext   // ambient lux
└── headlamp   : HeadlampContext     // HeadlampState + ACK-wait bookkeeping
```

`fsm::step` is the L2 **orchestrator** after L4 zone merge: operational FSM transitions and domain actions. L1 headlamp behaviour runs in **`HeadlampActor`** (or locally in pure tests via `zone_turn`).

> **Headlamp (step-7b):** `HeadlampContext` still embeds in `VehicleContext` (phase A). The brain refreshes that embed from **`HeadlampZoneReply`** after tell-back — it does not call `on_receiving_message` in parallel with the child. Other zones remain in-process until actorified; see the milestone handoff doc.

### Brain ↔ headlamp assembly (enumerated mailboxes)

Cross-boundary traffic uses typed enums only:

| Direction | Types |
| --------- | ----- |
| Brain → headlamp | `HeadlampActorMsg::Apply(HeadlampActorVocabulary { message, turn_id, tell_attempt, … })` |
| Headlamp → brain (correlated) | `DigitalTwinCarVocabulary::HeadlampZoneReady { turn_id, tell_attempt, reply }` |
| Headlamp → brain (zone deadline) | `HeadlampZoneSpontaneous` (ACK timer → `FrontHeadlampActuationIncomplete` commit) |
| Brain internal (unresponsive twinlet) | `TellBackTimeout` → retry tell → synthetic embed |

Tell is fire-and-forget; the brain mailbox stays free until tell-back (or timeout). Details: [`docs/milestone-actor-headlamp-scope.md`](docs/milestone-actor-headlamp-scope.md).

### Actor commit contract (step-7b)

One **FSM ingress** → one **quiescent commit** (0+ internal hops after zone merge):

1. Brain receives `DigitalTwinCarVocabulary::Fsm(…)` (or processes tell-back / spontaneous zone message).
2. If demux routes headlamp: **tell** `HeadlampActor` → wait for **`HeadlampZoneReady`** (with tell-back deadline + retries).
3. **`commit_resolved_turn`** → `run_to_quiescence` → `zone_turn` merge + `fsm::step` (+ detector-synthesized internal hops).
4. **`apply_committed_quiescence`**: one ledger row per hop → single `apply_step` on final cut → execute merged actuation.

**`RequestFrontHeadlampOn` does not flip the lamp in the action phase.** Lux crossing the threshold moves lighting to `OnRequested` and emits the command; `On` arrives only when a later ACK event runs through the headlamp path. ACK/NACK on CAN becomes a **new** FSM ingress.

Each ledger record lists **intended** `DomainAction`s from that hop. Runtime-only hints (e.g. actor mode) are filtered out of the stored record. See `crates/common/src/twin_runtime/twin_turn.rs` and `virtual_car_actor.rs`.

---

## Observability: a fact ledger, not just logs

Two channels with **distinct, deliberate jobs**:

|          | `transition_tx` — fact ledger       | `diagnostic_tx` — telemetry/presentation bus |
| -------- | ----------------------------------- | -------------------------------------------- |
| Delivery | bounded, lossless-or-error          | unbounded, best-effort                       |
| Ordering | total, by `record_seq`              | none guaranteed                              |
| Cadence  | exactly one record per FSM event    | many sources (init, ticks, failures, meta)   |
| Audience | machines (replay, invariant checks) | humans / logs                                |

The twin doesn't write to those channels directly; it emits through two **sink traits** —
`TransitionRecordSink` (carrying `PublishedTransitionRecord`) and `DiagnosticSink` (carrying
`DiagnosticMessage`). The default implementations (`TokioMpscTransitionRecordSink` /
`TokioMpscDiagnosticSink`) wrap the `transition_tx` / `diagnostic_tx` senders, and
`spawn_stdout_diagnostic_observer` drains the diagnostic side to stdout. Keeping the twin behind
sink traits is what lets a future observer/telemetry actor subscribe without the twin knowing
who is listening. (These are the sinks shown in the architecture diagram.)

Each `RawTransitionRecord` now also carries the **intended `DomainAction`s** the pure step emitted (intents, *not* outcomes — ACK/timeout/failure stay separate facts). A serializable **`published`** mirror projects every `Instant` to a wall-clock `Duration` so a "dumb writer → file → offline verifier" pipeline is possible: monotonic `Instant` *measures* time inside the core; `Duration` *places* records for the outside world.

`GetStatus` replies with a stamped `CarSnapshot { car, as_of_seq }`: a snapshot is never "wrong", only *as-of* sequence `N`, and the stamp makes staleness legible and reconcilable against the ledger.

**Later:** state-transition lines like "Transitioned to Driving…" should eventually be **derived from the ledger** by an observer, not emitted separately alongside it.

### Live-run hardening (console back-pressure)

A live run with `Ctrl-S` / `Ctrl-Q` (XOFF/XON) exposed two gaps, both fixed in this repo:

| Gap | Fix | Where |
| --- | --- | --- |
| Clean headlamp ACK was silent on the diagnostic stream (ledger had the context diff) | Compare `headlamp.state` before/after `step()`; emit `diag_front_headlamp_confirmed` on `*Requested → On/Off` | [`crates/common/src/twin_runtime/controller/virtual_car_actor.rs`](crates/common/src/twin_runtime/controller/virtual_car_actor.rs) |
| Synchronous logging on the CAN ingress / actuator loop could stall protocol handling | Bounded log queue, non-blocking `try_send`, drop-on-full; gateway submits ACK/NACK to the twin **before** logging | `crates/gateway/src/gateway_runtime.rs`, `crates/front_headlamp_actuator/src/main.rs` |

---

## Correctness model: enforce → announce → detect

* Invariants play three **separate** roles:
1. **Enforce** in the FSM transition — clamp/reject so the twin stays in a steady state (e.g. speed is frozen to 0 while `Off`).
2. **Announce** would-be / clamped violations via the **diagnostic** sink (this is why
   `LogWarning` now routes to diagnostics, not the actuation no-op).
3. **Detect** post-hoc with a **pure, public** `verify_state_laws(&FsmState, &VehicleContext)` over a named `STATE_LAWS` catalog — an **oracle** for tests / CI / offline replay, **never** a synchronous hot-path gate.
* DigitalTwinCar` is **correct-by-construction**: private fields, a checked `new(...)` (rejects a blank identity), and a single `apply_step(...)` mutator. "A twin with a blank identity" and "a twin mutated outside the FSM step" are now *unrepresentable*, not runtime-checked.

> Scope honesty: this prototype demonstrates that *the FSM never lets the vehicle reach a dangerous state*; it does **not** claim production-grade safety-clamp coverage (that needs deep automotive/physics domain knowledge).

---

## Vehicle states

The car's **operational mode** is a single primary FSM (`FsmState`):

| State                         | Meaning                                                                                                                                               |
| ----------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| **`Off`**                     | Ignition off. Derived speed forced to **0**; lighting context cleared to **Off**.                                                                     |
| **`Idle`**                    | Powered on (healthy), engine at rest (RPM ≤ 1000).                                                                                                    |
| **`Driving`**                 | Powered on, **RPM > 1000**, derived speed non-zero.                                                                                                   |
| **`ExtremeOperationWarning`** | Stress band: derived **speed > 160 km/h** and/or **speed > 160 with RPM > 5500**. Buzzer on; recovers after a **5 s** cooldown once thresholds clear. |

```text
Off ──PowerOn──► Idle ◄──speed=0── Driving
                  ▲                    │
                  │                    │ operational_warning_active
                  └──── speed=0 ───────┤
                                       ▼
                          ExtremeOperationWarning
                                       │
                          (cooldown + thresholds clear)
                                       ▼
                                 Driving or Idle
```

Front-headlamp progress is tracked separately in **`HeadlampState`** (`Off` → `OnRequested` → `On` → `OffRequested` → …) inside the headlamp zone — not as extra top-level FSM states. So at any instant the twin holds 

* **one primary mode** plus,

* **one lighting sub-state** (e.g. `Driving` + `On`).

### Anti-flap: hysteresis & cooldown

The twin has two boundaries where a noisy signal could otherwise cause rapid on/off (or in/out) **chatter** (a.k.a. flapping): **(1)** the ambient-lux level that switches the **headlamp** on/off, and **(2)** the speed/RPM level that moves the car in/out of
**`ExtremeOperationWarning`**. Both are debounced **inside the twin** (the emulator only emits raw telemetry) — but with two different mechanisms, because the two signals differ:

| Boundary                              | Mechanism                                                     | Enter / Exit                                                                                                                |
| ------------------------------------- | ------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| **Lux → headlamp**                    | **value deadband** (lux is a measured, jittery signal)        | request ON at `lux ≤ 840`; request OFF at `lux ≥ 860`; **hold** in the `(840, 860)` band                                    |
| **Speed → `ExtremeOperationWarning`** | **temporal latch** (speed is derived; this is a safety state) | enter at `speed > 160`; leave only after **≥ 5 s dwell AND** the hazard has cleared (`speed ≤ 160`), checked on `TimerTick` |

The cooldown is an **anti-flap floor, not an auto-clear timer**: the 5 s sets the *earliest* time recovery is allowed, but the hazard clearing is what actually triggers it. The timer is stamped once at entry and is **not** re-armed while latched, so once 5 s has passed the twin recovers immediately on the tick after the hazard clears. Consequently, **a hazard the world never resolves keeps the twin latched indefinitely — by design** (e.g. if RPM stays high enough that derived speed never drops to ≤ 160, the warning never clears and the buzzer stays on).

---

## Intentional shortcomings (carried forward / consciously kept)

| Area                 | Current choice                                                                  | Why it matters                                                                  |
| -------------------- | ------------------------------------------------------------------------------- | ------------------------------------------------------------------------------- |
| **VSS**              | Local `VssSignal` enum, not COVESA catalog / databroker                         | Fast iteration; real VSS mapping is future work.                                |
| **Kinematics**       | `WheelRpm` is 4 fields but broadcast (`uniform`); single RPM → speed multiplier | No four-wheel model, slip, gear, or observed-speed fusion yet.                  |
| **Powertrain mode**  | `PowertrainMode` derived but **not consumed** by the FSM                        | Groundwork only; behaviour unchanged.                                           |
| **Speed on CAN**     | `0x101` decoded, **not** consumed                                               | Deliberate separation until an observed-speed ECU path exists.                  |
| **Dashboard**        | stdout only                                                                     | No Zenoh, MQTT, or HMI; `PublishStateSync` is a log stub.                       |
| **Interface**        | `vcan0` hardcoded in three binaries                                             | No CLI/env yet; fine for Ubuntu + SocketCAN labs.                               |
| **Assembly fields**  | still `pub`                                                                     | Compile-compat shim; full encapsulation comes with the actor split.             |
| **Offline verifier** | designed & unblocked, **not built**                                             | The file-writer + folding tool are a deferred consumer of the published ledger. |

These are documented **non-goals for the milestone**, not oversights.

---

## Software map

| Crate                                                            | Responsibility                                                                                                                                                                                                                                                                               |
| ---------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [**`common`**](crates/common/)                                   | L0–L5 pyramid in one crate (see module docs + [`docs/design-notes-pyramid-layers.md`](docs/design-notes-pyramid-layers.md)): VSS, projection, FSM + `step`, `vehicle_state/*`, `digital_twin`, `twin_runtime` / `VirtualCarActor`, `published`, facade. |
| [**`vehicle_device_bus`**](crates/vehicle_device_bus/)           | Front-headlamp CAN codec, wire kinds, ingress policy.                                                                                                                                                                                                                                        |
| [**`emulator`**](crates/emulator/)                               | World models (RPM target tracking, lux jitter/tunnels) → telemetry frames.                                                                                                                                                                                                                   |
| [**`gateway`**](crates/gateway/)                                 | `main` + `gateway_runtime`: install twin, CAN loop, timer tick, CMD TX, bounded log side-channel.                                                                                                                                                                                            |
| [**`front_headlamp_actuator`**](crates/front_headlamp_actuator/) | Blocking actuator loop on CMD with configurable drop/NACK probabilities.                                                                                                                                                                                                                     |

Key files:

| What             | Path                                                                                                                 |
| ---------------- | -------------------------------------------------------------------------------------------------------------------- |
| FSM table        | [`crates/common/src/fsm/transition_map.rs`](crates/common/src/fsm/transition_map.rs)   |
| Assemblies       | [`crates/common/src/vehicle_state/`](crates/common/src/vehicle_state/)                                                 |
| Published mirror | [`crates/common/src/published.rs`](crates/common/src/published.rs)                                                   |
| State laws       | [`crates/common/src/digital_twin/car_behaviour_checker.rs`](crates/common/src/digital_twin/car_behaviour_checker.rs) |

---

## Wire protocol (reference)

**Telemetry** — 11-bit standard IDs, 2-byte big-endian:

| Signal        | ID      | Notes                        |
| ------------- | ------- | ---------------------------- |
| Vehicle speed | `0x101` | Decoded; **not** fed to twin |
| Engine RPM    | `0x102` | Ingress → `UpdateRpm`        |
| Ambient lux   | `0x103` | Ingress → `UpdateAmbientLux` |

* `0x101` is a reserved slot for a future observed-speed ECU (the twin derives speed from RPM today): the emulator never emits it, the gateway CAN reader decodes-then-drops it, and the projector also rejects it with an error. The double handling is deliberate — the reader skip covers the CAN path, while the projector rejection is defense-in-depth for any non-CAN ingress (tests, or code building `PhysicalCarVocabulary` directly) and is pinned by a contract test.

* **Front headlamp** — ID `0x204`, kinds in `vehicle_device_bus` (CMD / ACK / NACK for ON and OFF paths).

---

## Tests

```bash
cargo test -p common
cargo test -p gateway --lib
cargo test -p vehicle_device_bus
cargo test -p common --features proptest   # optional
```

Bus integration tests need `vcan0` up:
`cargo test -p vehicle_device_bus --test front_headlamp_bus_e2e`,
`cargo test -p gateway --test front_headlamp_e2e`.

---

## How to run (Linux)

**Requirements:** Linux with SocketCAN, Rust (workspace edition 2024).

```bash
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan 2>/dev/null || true
sudo ip link set up vcan0
```

Three terminals:

```bash
cargo run -p emulator
cargo run -p front_headlamp_actuator
cargo run -p gateway
```

Optional gateway flags (combine as needed):

```bash
cargo run -p gateway -- --print-timer-tick          # TimerTick heartbeat on stdout
cargo run -p gateway -- --print-transitions         # FSM transition lines
cargo run -p gateway -- --trace-actuation-ingress   # ignored headlamp ingress (wire trace); off by default
```

**Actuator with demo env** (values in `0.0`..=`1.0`):

```bash
FRONT_HEADLAMP_ACTUATOR_DROP_RESPONSE_PROB=0.15 \
FRONT_HEADLAMP_ACTUATOR_ACK_NACK_RESPONSE_PROB=0.5 \
cargo run -p front_headlamp_actuator
```

| Variable                                         | Example | Effect                                                                      |
| ------------------------------------------------ | ------- | --------------------------------------------------------------------------- |
| `FRONT_HEADLAMP_ACTUATOR_DROP_RESPONSE_PROB`     | `0.15`  | ~15% of CMDs get **no** ACK/NACK on CAN (gateway may log `⏱️` timeout).     |
| `FRONT_HEADLAMP_ACTUATOR_ACK_NACK_RESPONSE_PROB` | `0.5`   | When the actuator **does** respond, P(ACK)=0.5 (default if unset: **0.7**). |

**Emulator with tunnel-frequency env** (controls how often low-lux tunnels drive the headlamp ON; value in `0.0`..=`1.0`):

```bash
EMULATOR_TUNNEL_PROB=0.002 cargo run -p emulator
```

| Variable               | Example | Effect                                                                                                                                          |
| ---------------------- | ------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| `EMULATOR_TUNNEL_PROB` | `0.002` | Per-100 ms-tick probability of entering a tunnel. Default (unset) `0.01` ≈ a tunnel every ~10 s; `0.002` ≈ every ~50 s; `0.001` ≈ every ~100 s. |

**Teardown:** `Ctrl+C` each process; `sudo ip link del vcan0`.

Change `DEFAULT_CAN_INTERFACE` in emulator, actuator, and `gateway_runtime` if not using `vcan0`.

---

## Dependencies (summary)

`socketcan`, `tokio` (gateway), [`ractor`](https://crates.io/crates/ractor/0.15.12) (actor), `anyhow`, `rand` (emulator models), `serde` (published mirror).

---

## Roadmap (`sdv_simulation_3`)

**`main`:** pyramid milestone **done** (tag `pyramid-m2-complete`). Further layer work on `main`
is optional (`sdv_core` split, gateway e2e diagnostics polish).

**Actor track (branches `milestone/actor-*`; blog Iteration 3):**

- **Done on `milestone/actor-headlamp`:** headlamp child actor, typed brain↔assembly mailboxes, tell-back race, quiescence, actor-owned ACK timer (template for zone #2)
- ADR-6 power barrier; further zone twinlets
- Single-writer ledger actor, correlation IDs, diagnostics-as-projection (WI-8–10)
- Actuation child + resilience (WI-11, WI-13); `Clock` seam (WI-3)
- Offline ledger file writer + folding verifier

**Later (any iteration):** observed-speed ECU / fusion, COVESA VSS, transport scale-up, structured egress, richer emulation. See also [Iteration 1 roadmap](https://github.com/nsengupta/sdv_simulation_1#roadmap).

---

*Blog posts are drafted after repo milestones land. This README stays the operational truth — update when behaviour or layout changes.*
