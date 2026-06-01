<!--
DRAFT README for Iteration 2 — generated for the author to edit.
Operational sections (run / tests / wire protocol / crates) are carried over and
verified against the current tree. Framing/architecture/decisions are rewritten to
reflect what actually changed since Iteration 1. Remove this comment before publishing.
-->

# SDV simulation (Iteration 2) — repository overview

---

↩️This repo is the **Iteration 2** of a software-defined-vehicle (#SDV) prototype. It builds directly on **Iteration 1** (processes, CAN bus, the actor+FSM twin):
**[Iteration 1 — repository & README](https://github.com/nsengupta/sdv_simulation_1#readme)**.

This README is the **current truth** for running and reading the code.

---

A Rust workspace that prototypes a **software-defined vehicle control path**: telemetry and actuation on a **shared CAN bus**, a **gateway** that translates wire traffic into domain
events, and a **digital twin** that maintains vehicle state, decides when to actuate, and
closes the loop when the body ECU acknowledges (or fails).

This is an educational / demonstrator codebase, not a product stack.

---

## What this iteration is about (and what it deliberately is *not*)

If you ran Iteration 1 and Iteration 2 side by side, **you could not tell them apart**: the
processes, the CAN wire protocol, the FSM behaviour, and the on-screen output are the same; well, not exactly, but very much the same.

That is intentional. **Iteration 1 made the control loop *work*. Iteration 2 makes it
*decomposable, observable, and provable*** — restructuring the internals and tightening the contracts so that the *next* iteration can split the monolithic twin into concurrent
per-zone child actors ("actorification") without a (major) rewrite.

So this is a **refactor-and-contracts milestone**. The interesting work is underneath the
surface. Three threads run through it:

1. **Decompose the twin's state to mirror the car's physical zones** (powertrain, body lighting, health, visibility) — *before* making them concurrent.
2. **Turn observation into a first-class, portable contract** 
   1. a serializable and **potentially replayable** transition ledger you can replay and audit 
      offline. 
   2. a separate, best-effort, human diagnostic bus.
3. **Make illegal states hard to represent and easy to detect** — enforce in the FSM,
   announce via diagnostics, detect offline with a pure law catalog.

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

Future plan: Everything tagged *actorification* in the design log is **deliberately deferred to 
the next iteration** (a fresh cloned project): single-writer ledger actor, end-to-end correlation IDs, diagnostics-as-projection, an actuation child actor, and actuation resilience (retry/backoff/dedup/circuit-breaker).

---

## Runtime shape (unchanged from Iteration 1)

**Three processes** share Linux **SocketCAN** (`vcan0` by default):

| Process                     | Role                                                                                                                                    |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| **emulator**                | Stand-in for powertrain + ambient-light sensing: publishes **engine RPM** and **ambient lux** on CAN (~10 Hz).                          |
| **gateway**                 | Owns ingress (read CAN), **projection** into twin vocabulary, the **digital twin** runtime, and egress (write headlamp **CMD** frames). |
| **front_headlamp_actuator** | Stand-in body ECU: receives CMD, replies with **ACK** or **NACK** on the same bus (~150 ms later).                                      |

Inside the gateway, the twin is a **`VirtualCarActor`** ([`ractor`](https://crates.io/crates/ractor/0.15.12) 
mailbox, single-threaded handling) driving a pure **FSM** plus an orthogonal **lighting**
sub-state. Outcomes are visible on **stdout** — a deliberate stand-in for a dashboard or cloud stream.

Signals are modeled in a **VSS-inspired** Rust enum (`EngineRpm`, `AmbientLux`, and a
decoded-but-unused `VehicleSpeed` slot for a future observed-speed ECU). Payload layout and headlamp wire kinds live in **`vehicle_device_bus`** so the gateway stays thin and the actuator binary stays independent.

### Demo screenshots

Still frames from a live three-process run on `vcan0`. Both show the same gateway **stdout** surface — FSM transitions, RPM/lux telemetry, and (when lighting rules fire) headlamp actuation — under different ambient-light conditions.

**Daylight band (headlamp off)** — ambient lux stays above the OFF threshold (`LUX_OFF` 860).

The twin keeps `LightingState::Off`; the gateway log has no headlamp CMD or ACK/NACK lines.

![Gateway run in daylight — headlamp off, no actuation traffic](assets/runtime-screenshot-no-headlamp.png)

**Tunnel / dim band (headlamp on)** — lux falls through the ON threshold (`LUX_ON` 840), the twin requests ON, the gateway sends `📤🔆` CMD on CAN, and the actuator reply appears as `✅💡` (or `❌🔆` / `⏱️` on failure paths).

![Gateway run in low lux — headlamp ON requested and ACK'd](assets/runtime-screenshot-with-headlamp.png)

---

## Architecture

Telemetry and commands meet on one virtual CAN interface; the gateway is the only component that speaks both "wire" and "twin."

![SDV prototype — emulator, gateway (digital twin), front-headlamp actuator, and vcan0](assets/SDV-Blog-Inputs-4.jpg)

**Ingress path:** CAN frame → `VssSignal` / headlamp payload → `PhysicalCarVocabulary` →`PhysicalToDigitalProjector` → `DigitalTwinCarVocabulary` → actor → `fsm::step`.

**Egress path:** `DomainAction` (e.g. request headlamp ON) → `DefaultActuationManager` →`ActuationCommand` → gateway encodes CMD → CAN → actuator → ACK/NACK → same reader thread →policy correlation → twin ACK/NACK/incomplete events.

### The twin's state is now organised by assembly (zone)

`VehicleContext` is no longer a flat bag of fields; it is an **aggregate of self-sufficient
assemblies**, each owning its own data *and* the rules over that data:

```text
VehicleContext
├── powertrain : PowertrainContext   // WheelRpm (4 wheels) + derived speed_kph + PowertrainMode
├── health     : VehicleHealthContext// fuel / oil / tyre
├── visibility : VisibilityContext   // ambient lux
└── headlamp   : HeadlampContext     // LightingState + ACK-wait bookkeeping
```

`fsm::step` has become a thin **orchestrator**: it dispatches each event to the owning assembly (`apply_rpm`, `apply_lux`, `apply_on_ack`, …), triggers derivations (`refresh_speed`), and runs the operational FSM — but the subsystem *behaviour* lives on the assembly types.

> This is in preparation for the zone-actor plan, where each assembly's `impl` block becomes a child actor's local state + flat FSM. The aggregate's fields stay public **only** as a compile-compatibility shim during the transition.

### Actor turn contract

One mailbox message → one authoritative cut. The actor loop is fixed-order:

1. `fsm::step(...)` — pure decision (context update, operational FSM, intended actions).
2. `twin_car.apply_step(...)` — persist.
3. Emit transition record — **after** persist, **before** actuation (captures intent, not outcome).
4. Execute actions — actuation manager takes an **immutable** twin ref; no `step()` re-entry in the same turn.

**`RequestFrontHeadlampOn` does not flip the lamp in the action phase.** Inside `step()`, lux crossing the threshold moves lighting to `OnRequested` and emits the command; `On` arrives only when a later ACK event runs through `step()`. ACK/NACK on CAN becomes a **new** mailbox event.

Each ledger record lists **intended** `DomainAction`s from that step. Runtime-only hints (e.g. actor mode) are filtered out of the stored record; ACK, NACK, timeout, and failure remain separate facts. See `crates/common/src/fsm/step.rs` and `crates/common/src/twin_runtime/controller/virtual_car_actor.rs`.

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

Front-headlamp progress is tracked separately in **`LightingState`** (`Off` → `OnRequested` →`On` → `OffRequested` → …) inside the headlamp assembly — not as extra top-level FSM states. So at any instant the twin holds 

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
| [**`common`**](crates/common/)                                   | VSS encode/decode, vocabularies, projection, FSM + `step`, **per-assembly contexts** (`vehicle_state/*`), **`published`** serializable mirror, `VirtualCarActor`, `VehicleController`, actuation manager, state-law catalog, `vehicle_physics`, `front_headlamp_log`. |
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

## Roadmap

Major milestones ahead (not in priority order). Carries forward open items from
[Iteration 1](https://github.com/nsengupta/sdv_simulation_1#roadmap) plus this iteration's actorification track.

- **Actorification** — parent FSM actor + per-zone child actors (assemblies → actors); unified diagnostic fan-in
- **Observability & audit** — single-writer ledger actor, correlation IDs end-to-end, diagnostics-as-projection of the ledger; offline file writer + folding verifier
- **Actuation resilience** — when the actuator is down or dropping responses, lux-driven reconcile currently re-requests every telemetry tick and emits a timeout warning every tick (unbounded command spam, no recovery). Next iteration: bounded retry/backoff, dedup of pending requests, explicit degraded/`Unknown` lighting state, rate-limited "peer persistently unavailable" diagnostic. Design input: **WI-13** in [`docs/design-notes-runtime-observation.md`](docs/design-notes-runtime-observation.md) (anchors: `crates/common/src/vehicle_state/front_headlamp.rs`).
- **Observed-speed ECU & fusion** — wire path for `0x101` vs RPM-derived kinematic speed
- **Standards alignment** — official COVESA VSS / databroker; DBC-driven CAN IDs
- **Transport & scale-up** — CLI/env CAN interface; additional `vehicle_device_bus` devices and zones
- **Structured egress** — beyond stdout (Zenoh, uProtocol, HMI/dashboard)
- **Richer emulation** — ECU profiles and world models
- **`Clock` seam** — injectable time at the actor boundary (pairs with the future timer/ticker child)

---

*Keep the blog as the narrative arc; keep this README as the current truth. Update this file
when user-visible behaviour or repo layout changes.*
