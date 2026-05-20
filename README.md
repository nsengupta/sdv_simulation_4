# SDV simulation (draft)

Rust workspace that simulates a small **vehicle data path** inspired by **VSS (Vehicle Signal Specification)** ideas: telemetry is modeled as named signals, encoded on **SocketCAN**, and consumed by a **gateway** that hosts a **digital twin** (FSM + actor) and orchestrates **front-headlamp** actuation over the bus.

This is a hands-on learning / demo project, not production software.

## Requirements

- **Linux** with SocketCAN (typical for `vcan` or real CAN hardware).
- **Rust** toolchain compatible with the workspace (edition 2024).

## How to run (Linux quick start)

`vcan0` is the default interface; all three binaries use it in code.

**Setup** (once per boot, requires `sudo`):

```bash
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan 2>/dev/null || true
sudo ip link set up vcan0
```

**Start** (three shells, no `sudo`):

```bash
cargo run -p emulator
```

```bash
cargo run -p front_headlamp_actuator
```

```bash
cargo run -p gateway
```

Optional gateway flag for heartbeat logs:

```bash
cargo run -p gateway -- --print-timer-tick
```

**Teardown** after `Ctrl+C` in each shell:

```bash
sudo ip link del vcan0
```

To use another interface, change `DEFAULT_CAN_INTERFACE` in `crates/emulator/src/main.rs`, `crates/front_headlamp_actuator/src/main.rs`, and `crates/gateway/src/gateway_runtime.rs`.

## What you should see

| Process | Role |
| -------- | ----- |
| **emulator** | Publishes **engine RPM** and **ambient lux** on CAN (~10 Hz). Prints derived speed in debug only (not on the bus). |
| **front_headlamp_actuator** | Listens for headlamp **CMD** frames; responds with **ACK/NACK** after ~150 ms (configurable drop/NACK via env). |
| **gateway** | Runs the digital twin, ingests telemetry + headlamp responses, emits **CMD** on CAN when the twin requests lighting. |

**Gateway logs (representative):**

- State transitions: `[NASHIK-VC-001]: Transitioned to …`
- Cloud sync: `📡 Publishing to Cloud: …`
- Headlamp **commands**: `📤🔆 Requesting front headlamp ON.` / `📤🌑 … OFF.`
- Headlamp **ingress**: `[actuation-can-ingress …]: ✅💡 … confirmed` / `❌🔆 … rejected (NACK)`
- **Alerts** (timeout): `[ALERT …]: ⏱️💡 … no actuator response (timed out).`
- **Stress**: `🔊 BUZZER ON` / speed or extreme-operation alert lines when thresholds are exceeded

**Actuator** (when it receives CMDs): `received ON CMD` / `OFF CMD`, then ACK or NACK on the wire.

**Emulator debug line (example):**

```text
DEBUG: Time=33s | CompositeRPM=6388 (Target=6500) | DerivedSpeedKph=728.23 | AmbientLux=842
```

TimerTick lines on the gateway are **off** unless `--print-timer-tick` is set.

### Demo actuation (optional env)

On the actuator process:

- `FRONT_HEADLAMP_PLANT_DROP_RESPONSE_PROB` — probability of no frame after CMD (simulate silent bus).
- `FRONT_HEADLAMP_PLANT_ACK_NACK_RESPONSE_PROB` — when responding, probability of ACK vs NACK (default `0.7` ACK).

## Workspace layout

| Crate | Role |
| ----- | ---- |
| `common` | VSS signals, physical/digital vocabularies, projection, FSM (`step`, `transition_map`), `VirtualCarActor`, actuation manager, vehicle constants/kinematics, `front_headlamp_log` icons |
| `vehicle_device_bus` | Shared CAN payload codec, wire kinds, and **front_headlamp** policy (correlation, pending command) |
| `emulator` | Virtual ECU: RPM + lux models → CAN telemetry |
| `gateway` | Thin `main`; `gateway_runtime` — CAN reader thread, ingress, twin controller, CMD publisher |
| `front_headlamp_actuator` | Standalone body ECU binary |

Block diagrams (Mermaid): `blog-inputs/diagrams/05-sdv-architecture-block.mmd`, `06-digital-twin-fsm-inset.mmd`.

## End-to-end data path

```text
emulator ──RPM, lux──► vcan0 ◄──CMD / ACK|NACK── front_headlamp_actuator
                          │
                          ▼
                    gateway (CAN reader)
                          │
          PhysicalCarVocabulary → PhysicalToDigitalProjector
                          │
                          ▼
                 VirtualCarActor (mailbox)
                          │
                    fsm::step → transition_map
                          │
              DomainAction → ActuationManager → CMD on vcan0
```

**Ingress today:** `EngineRpm`, `AmbientLux`, `TimerTick`, `SystemReset`, front-headlamp **Confirmed/Rejected** (from CAN ACK/NACK). **Observed speed** on CAN (`VehicleSpeed`, ID `0x101`) is decoded but **rejected** at projection; the twin **derives** `VehicleContext::speed` from RPM in `step()` via `vehicle_kinematics`.

## Digital twin (inside gateway)

- **`VehicleController`** — installs `VirtualCarActor`, runs `PhysicalToDigitalProjector`, submits events.
- **`VirtualCarActor`** — holds `DigitalTwinCar` (`FsmState` + `VehicleContext`), calls `fsm::step`, executes `DomainAction` via `DefaultActuationManager`.
- **FSM helpers** (called from `step`):
  - `engine::op_strategy::transition_map` — `transition` / `output`
  - `vehicle_kinematics` — `calculate_speed_from_rpm`, `refresh_context_speed`
  - `vehicle_constants` — RPM/lux/speed thresholds, ACK wait durations
- **Lighting** — orthogonal `LightingState` in context; lux hysteresis; ACK wait / timeout / NACK recovery in `step`.

## Primary FSM (`Off` | `Idle` | `Driving` | `ExtremeOperationWarning`)

Canonical rules: `crates/common/src/engine/op_strategy/transition_map.rs`.

| Transition | Condition |
| ---------- | ----------- |
| `Off` → `Idle` | `PowerOn` and healthy context |
| `Idle` → `Driving` | `UpdateRpm` > 1000 |
| `Driving` → `Idle` | Derived `speed == 0` (after kinematic refresh in `step`) |
| `Driving` → `ExtremeOperationWarning` | **Operational warning** active (see below) |
| `ExtremeOperationWarning` → `Driving` / `Idle` | `TimerTick`, 5 s cooldown elapsed, warning cleared; `Idle` if `speed == 0` |

### Operational warning (speed + RPM)

Constants in `vehicle_constants.rs`:

| Constant | Value | Meaning |
| -------- | ----- | -------- |
| `SPEED_EXTREME_OPERATION_THRESHOLD_KPH` | 160 | Derived speed above this contributes to warning |
| `RPM_EXTREME_OPERATION_THRESHOLD` | 5500 | RPM above this (with speed) = extreme-operation pair |

**Enter `ExtremeOperationWarning`** when **either**:

1. **Speed alone** — derived `speed` > 160 km/h → `SpeedThresholdExceeded` alert (+ buzzer), or  
2. **Extreme operation** — `speed` > 160 **and** `rpm` > 5500 → `ExtremeOperationWarning` alert (+ buzzer); both alerts if both apply.

**Recovery:** after `RPM_STRESS_DURATION_THRESHOLD_SECS` (5 s), leave warning when **neither** condition holds (`operational_warning_active` is false).

**Kinematics:** `speed` (km/h, `u16`) is computed from wheel/composite RPM (`rpm × 0.114`, tire model in `vehicle_kinematics`); not clamped to 255. Ignition `Off` forces `speed = 0` in `step`.

## Front headlamp + ambient lux

### Thresholds (demo-calibrated)

| Constant | Value | Effect |
| -------- | ----- | ------ |
| `LUX_ON_THRESHOLD` | 840 | `Off` + lux ≤ 840 → request ON |
| `LUX_OFF_THRESHOLD` | 860 | `On` + lux ≥ 860 → request OFF |
| Deadband | 841–859 | Hold current lighting state |

Emulator profile (`daytime_tunnel_profile`): baseline ~850 lux, jitter ±35 (~815–885), occasional tunnel dips — intended to cross ON/OFF often in one run. This is **demo tuning**, not literal night lux.

### Lighting sub-states

`Off` → `OnRequested` → `On` → `OffRequested` → `Off` (ACK-driven). Pending states suppress duplicate CMD intents. **Timeout** (2 s, `FRONT_HEADLAMP_*_ACK_WAIT`) and **NACK** map to `FrontHeadlampActuationIncomplete` with recovery in `step`.

### Vocabulary layers

| Layer | ON success | OFF success | Failure |
| ----- | ---------- | ----------- | ------- |
| CAN / physical | Confirmed | Confirmed | Rejected |
| FSM event | `FrontHeadlampOnAck` | `FrontHeadlampOffAck` | `FrontHeadlampActuationIncomplete` |

Policy and correlation: `vehicle_device_bus::devices::front_headlamp::policy`.

### Log icons (`common::front_headlamp_log`)

| Situation | Icon | Message constant |
| --------- | ---- | ---------------- |
| Command ON | `📤🔆` | `MSG_REQUEST_ON` |
| Command OFF | `📤🌑` | `MSG_REQUEST_OFF` |
| ACK ON | `✅💡` | `MSG_ACK_ON` |
| ACK OFF | `✅🌑` | `MSG_ACK_OFF` |
| NACK ON | `❌🔆` | `MSG_NACK_ON` |
| NACK OFF | `❌🌑` | `MSG_NACK_OFF` |
| Timeout ON | `⏱️💡` | `MSG_TIMEOUT_ON` |
| Timeout OFF | `⏱️🌑` | `MSG_TIMEOUT_OFF` |

## CAN mapping (telemetry + headlamp)

Standard 11-bit IDs, 2-byte big-endian payloads for telemetry:

| Signal | ID | Payload |
| ------ | --- | -------- |
| Vehicle speed (future ECU) | `0x101` | km/h × 100 — **not** used by twin today |
| Engine RPM | `0x102` | `u16` RPM |
| Ambient lux | `0x103` | `u16` lux |

Front headlamp actuation (`vehicle_device_bus`, ID **`0x204`**): CMD / ACK / NACK kinds in `devices/front_headlamp/codec`; gateway egress encodes CMD; reader thread decodes ACK/NACK into `PhysicalCarVocabulary`.

Unknown IDs are ignored by the telemetry decoder until extended.

## Tests

```bash
# Unit / contract tests (no vcan required)
cargo test -p common
cargo test -p gateway --lib
cargo test -p vehicle_device_bus

# Optional property tests
cargo test -p common --features proptest

# Bus e2e (requires vcan0)
cargo test -p vehicle_device_bus --test front_headlamp_bus_e2e
cargo test -p gateway --test front_headlamp_e2e
```

## Dependencies (not exhaustive)

- **`socketcan`** — CAN on Linux  
- **`tokio`** (gateway) — async runtime, channels, timers  
- **`ractor`** (common) — `VirtualCarActor` mailbox  
- **`anyhow`** — binaries  
- **`rand`** (emulator) — world-model jitter  

## Future work (ideas)

- CLI/env for CAN interface on all binaries  
- Observed-speed ECU path wired through projection (separate from kinematic expectation)  
- Configurable emulator profiles (day / tunnel / night) at startup  
- DBC- or ARXML-driven signal catalog  
- Additional devices under `vehicle_device_bus::devices`  
- Structured logging/metrics instead of println demo lines  
- Transport adapters (Zenoh / uProtocol) reusing the same twin and policy core  

---

*This README is a **draft**; update it when user-visible behavior or milestones change.*
