# TODO — Simulation 5 (carry-forward from simulation-4)

Items not completed during simulation-4.  Each is a standalone unit of work.
Detailed design notes for each item live in `brain_fsm_redesign_impl_Phase_10.md`.

---

## 1. CAN emulation for `PowerOn` / `PowerOff`

**Status:** Not started.  
**Reference:** `brain_fsm_redesign_impl_Phase_10.md` Item C; `analysis_4_response.md` Stage 1.

`PowerOn` and `PowerOff` events are currently injected programmatically.
Map real CAN frames to FSM events:

| CAN ID | Payload                     | FSM event          |
|--------|-----------------------------|--------------------|
| `0x100` | `01 00 00 00 00 00 00 00` | `FsmEvent::PowerOn`  |
| `0x100` | `00 00 00 00 00 00 00 00` | `FsmEvent::PowerOff` |

Implement in a new `can_emulator` module (function first, actor if gateway integration requires it).

---

## 2. Non-blocking actuation

**Status:** Not started.  
**Reference:** `brain_fsm_redesign_impl_Phase_10.md` Item D; `findings/single-thread-guarantee.md` Category 2.

`actuation_manager.execute()` is currently `.await`-ed directly inside `virtual_car_actor.rs`,
holding the actor's execution thread during CAN transmission.

Move actuation into the assembly actor's own thread (send a ractor message to `HeadlampActor` /
`WiperActor`; they call `actuation_manager.execute()` on their own threads).
`VirtualCarActor` does not block.

---

## 3. Code commenting pass

**Status:** Not started.  
**Reference:** `brain_fsm_redesign_impl_Phase_10.md` Item E; `findings/Code-commenting-plan.md`.

Systematic doc-comment pass over the core call tree.
Priority order:

1. `zone_message_for_event`
2. `begin_fsm_turn`
3. `try_drain_barrier_queue`
4. `apply_committed_quiescence`
5. `step` / `transition` / `output`
6. `TurnBarrier` struct and constructors

Each doc comment must state: what the function does, its invariants, and its relationship
to its caller and callee.

---

## 4. HeadlampActor isolation tests and missing `ActuationIncomplete(Off)` coverage

**Status:** Not started.

### 4a — Missing L1 zone state transitions in `headlamp_lifecycle_contract.rs`

`headlamp_lifecycle_contract.rs` tests `HeadlampContext::on_receiving_message()` in isolation
(no Brain, no actor).  The `ActuationIncomplete(On) → Ready` path is covered but the OFF
direction is entirely absent:

| Missing test | Scenario |
|---|---|
| `actuation_incomplete_off_recovers_to_ready` | `OffRequested` + `ActuationIncomplete(Off)` → expected state? |
| `nack_for_off_while_on_requested` | `OnRequested` + NACK (off direction) → expected state? |
| `off_cmd_ack_normal_path` | `OffRequested` → `AckOff` → `Ready` (the happy path for OFF) |

### 4b — `HeadlampActor` (ractor actor) in isolation

No test spawns just `HeadlampActor` and exercises it directly without `VirtualCarActor`.
All actor-level headlamp tests (`headlamp_ack_timer_contract.rs`) use the full
`VehicleController` stack.

Required: a test harness that spawns only `HeadlampActor`, sends messages to it, and
inspects the replies and state transitions independently of the Brain.

### 4c — The observed hang scenario (ON → OFF NACK → retry → drop → quiet)

The exact sequence observed in the running system:
1. Headlamp is `On` (confirmed ACK for ON).
2. OFF CMD sent → actuator responds NACK → headlamp → `ActuationIncomplete(Off)`.
3. HeadlampActor retries → actuator drops response → max retries exhausted →
   synthetic reply committed.
4. `LightingUnsafe` detector does NOT re-fire (lux is now high, headlamp is
   `ActuationIncomplete(Off)` — neither `Off` nor `Ready`).
5. No new `RequestFrontHeadlampOff` emitted until the next lux-triggered FSM transition.

**Test required:** Assert that after steps 1–4 the system is in a well-defined
"quiet but consistent" state and that the first subsequent lux drop correctly
re-issues an ON command (not an OFF retry).

---

## 5. Actor-level fuzz / steady-state tests

**Status:** Partial — FSM-level `proptest` exists (`fsm_properties.rs`); actor-level missing.  
**Reference:** `brain_fsm_redesign_impl_Phase_10.md` Item F; `analysis_4_response.md` Stage 6.

Required tests (spawn `VirtualCarActor`, exercise the full message loop):

1. **Ignition cycle invariant** — `PowerOn` → `AssemblyZoneReady` × N → random events →
   `PowerOff` → `AssemblyZoneReady` × N → assert final state is `Off`.
2. **Queue drain invariant** — inject events while a barrier is pending; assert
   `barrier_queue` drains to empty once all `AssemblyZoneReady` events arrive.
3. **No-panic property** — any sequence of valid ractor messages must not crash the actor.
