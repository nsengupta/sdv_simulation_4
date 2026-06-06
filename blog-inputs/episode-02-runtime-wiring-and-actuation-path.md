# Episode 2: From Deterministic Core to Runtime Wiring and Actuation Path

## Preface

Episode 1 established a deterministic domain core.  
This episode shows how that core is wired into runtime loops, gateway ingress, and actuation routing without violating domain boundaries.

## Motivation

A clean domain model is necessary but not sufficient. We still need:

- continuous ingress from CAN and timer sources,
- controlled action execution with backpressure-aware channels,
- a wiring model that keeps `main` thin and testable.

## Scope

This episode covers:

- gateway runtime orchestration,
- CAN reader thread + async dispatch loop split,
- timer tick injection path,
- actuation command routing and actuator process (today: separate `front_headlamp_actuator` binary on CAN).

It does not yet deep-dive into reliability hardening logic (ACK/NACK/no-response policy detail), which is the focus of Episode 3.

## Approach

The runtime keeps boundaries explicit:

1. Gateway reads frames from SocketCAN in a dedicated blocking thread.
2. Ingress events are mapped to canonical physical vocabulary.
3. `VehicleController` projects and forwards messages to actor/domain execution.
4. Domain actions are routed as actuation commands to the actuator (CAN CMD egress + ACK/NACK ingress in the current repo).

This preserves the domain contract from Episode 1 while enabling end-to-end operation.

## Arch Diagram

Primary diagram for this episode:

- `blog-inputs/diagrams/03-runtime-container-wiring.mmd`

Quick render option:

```bash
mmdc -i blog-inputs/diagrams/03-runtime-container-wiring.mmd -o blog-inputs/diagrams/03-runtime-container-wiring.svg
```

## Key Design Aspects

### 1) Keep `main` thin with runtime module orchestration

Use `gateway_runtime::run(...)` as the high-level entrypoint so setup logic remains centralized and test-friendly.

Suggested snippet source:

- `crates/gateway/src/gateway_runtime.rs` (`run`, launch config, channel setup)

### 2) Blocking CAN reads live on a dedicated OS thread

`read_frame()` is blocking and long-lived, so it stays outside Tokio worker paths.  
That design prevents accidental starvation and keeps async tasks focused on orchestration.

Suggested snippet source:

- `crates/gateway/src/gateway_runtime.rs` (`spawn_can_reader_thread`)

### 3) Ingress mapping remains canonical

`VehicleEvent -> PhysicalCarVocabulary` mapping is explicit and tested.  
This prevents ad-hoc transport payloads from bleeding into controller/domain APIs.

Suggested snippet source:

- `crates/gateway/src/ingress/mapping.rs`

### 4) TimerTick is a first-class event path

A periodic loop emits timer tick events into the same ingress/controller path used by telemetry, ensuring uniform state progression semantics.

Suggested snippet source:

- `crates/gateway/src/gateway_runtime.rs` (`spawn_timer_tick_loop`)

### 5) Actuation commands are routed through policy + actuator path

The gateway updates policy state, encodes CMD on CAN, and correlates ACK/NACK from the standalone `front_headlamp_actuator` process — a seam for reliability hardening and test injection.

Suggested snippet source:

- `crates/gateway/src/gateway_runtime.rs` (`spawn_front_headlamp_command_publisher`)
- `crates/common/src/twin_runtime/controller/actuation_manager.rs` (command emission)

### 6) Observability sinks are injected at gateway init — the twin does not route stdout

Episode 1 left replay and diagnostics as natural outputs of the deterministic core. Episode 2
wires them through **two separate channels** with different jobs (fact ledger vs presentation).
The important runtime rule:

**CLI flags → gateway `run()` → `VehicleControllerRuntimeOptions` → twin sinks (or absence thereof).**

The twin emits through trait objects (`TransitionRecordSink`, `DiagnosticSink`). It never
reads `--print-transitions-only` or chooses stdout vs discard. The gateway decides, once, when
it constructs the runtime:

| Launch | Diagnostic sink on twin | Transition sink on twin |
| --- | --- | --- |
| default | yes → stdout observer task | no |
| `--print-transitions-only` | **no** | yes → coloured ledger task |

There is no third “mixed” mode (ledger + diagnostics on stdout together).

When the gateway does **not** inject a sink, `VirtualCarActor` holds `None` and guards each
emission with `if let Some(sink)`. That guard is **not** routing policy — it is the mechanical
consequence of “runtime did not inject a sink.” Skipping the guard path also skips building
`DiagnosticMessage` / projecting `PublishedTransitionRecord`, so “observability off” is cheap.

The runtime owns the **picker** on the RX side (stdout printer today; file or GUI later). The
twin is only a **dropper** into whatever was wired. Do not confuse ledger-only silence on the
console with a stalled actuator: CAN actuation (`actuation_command_tx`) remains wired in every
mode; only human-facing streams are toggled at init.

Suggested snippet sources:

- `crates/gateway/src/gateway_runtime.rs` (`diagnostic_tx` / `transition_tx` wiring in `run`)
- `crates/common/src/twin_runtime/controller/virtual_car_actor.rs` (`pre_start` sink wrap)
- `crates/common/src/diagnostic/mod.rs` (module comment: twin unconcerned with RX consumer)

## Output Screenshots / Clips

Recommended artifacts for this episode:

- gateway startup log showing wiring initialization,
- first actuation command dispatch log path,
- optional short clip of telemetry + actuation loop in action.

## Link to the Next Episode

Episode 3 focuses on robustness at the transport boundary:

- correlation-safe ACK/NACK handling,
- no-response windows and timeout recovery,
- bus-level integration tests for front-headlamp flows.

Next draft file: `blog-inputs/episode-03-can-boundary-hardening.md`
