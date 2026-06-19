# Brain FSM Redesign — Phase 2 Implementation Notes

**Status:** Complete, tested (122/122 pass), zero warnings.  
**Git tag:** `phase-2-headlamp-zone-alphabet` (to be applied on milestone commit)

---

## What Phase 2 delivers

Phase 2 introduces the full lifecycle vocabulary for the Headlamp zone assembly.  
No brain-to-headlamp wiring changes yet (Phase 5); this phase is purely **L1 zone alphabet** work.

### New types

| Item | Location | Description |
|---|---|---|
| `HeadlampState::Ready` | `vehicle_state/front_headlamp.rs` | Assembly active, physical lamp dark. |
| `HeadlampMessage::BecomeOn` | `vehicle_state/front_headlamp.rs` | Brain activates assembly: `Off → Ready`. |
| `HeadlampMessage::BecomeOff` | `vehicle_state/front_headlamp.rs` | Brain deactivates assembly: any → `Off`. |
| `ZoneId` | `fsm/machineries.rs` | `pub enum ZoneId { Headlamp }` — opaque zone identity token for future generalization. |
| `PublishedHeadlampState::Ready` | `published.rs` | Ledger-serializable mirror of the new state. |

---

## HeadlampState FSM (Brain's FSM as template)

Uses the same pattern as `FsmState`: pure state enum + behavior in `HeadlampContext::on_receiving_message`.

### States

| State | Invariant |
|---|---|
| `Off` | Assembly not started. Ignores all lux events. Default at cold start. |
| `Ready` | Assembly started (`BecomeOn` received). Physical lamp is dark. Responds to lux. |
| `OnRequested` | ON command in flight (`RequestOn` emitted). Waiting for hardware `AckOn`. |
| `On` | Physical lamp confirmed ON. Responds to high-lux events for turn-off. |
| `OffRequested` | OFF command in flight (`RequestOff` emitted). Waiting for hardware `AckOff`. |

### Full state transition table

| From | Event/Message | To | Zone Outcome (L4 egress) |
|---|---|---|---|
| `Off` | `BecomeOn` | `Ready` | — |
| `Off` | `BecomeOff` | `Off` | — |
| `Off` | `AmbientLux(_)` | `Off` | — (ignored; not started) |
| `Off` | `ResetForIgnitionOff` | `Off` | — |
| `Ready` | `BecomeOff` | `Off` | — |
| `Ready` | `AmbientLux(lux ≤ LUX_ON_THRESHOLD)` | `OnRequested` | `RequestOn` |
| `Ready` | `AmbientLux(lux > LUX_ON_THRESHOLD)` | `Ready` | — |
| `Ready` | `TimerTick` | `Ready` | — (no pending ACK) |
| `Ready` | `ResetForIgnitionOff` | `Off` | — |
| `OnRequested` | `AckOn` | `On` | — |
| `OnRequested` | `ActuationIncomplete(On, _)` | `Ready` | `LogWarning` |
| `OnRequested` | `TimerTick` (at ON deadline) | `Ready` | `LogWarning` via timeout |
| `On` | `BecomeOff` | `Off` | — (forced shutdown) |
| `On` | `AmbientLux(lux ≥ LUX_OFF_THRESHOLD)` | `OffRequested` | `RequestOff` |
| `On` | `AmbientLux(lux < LUX_OFF_THRESHOLD)` | `On` | — |
| `On` | `ResetForIgnitionOff` | `Off` | — |
| `OffRequested` | `AckOff` | `Ready` | — (assembly stays active) |
| `OffRequested` | `ActuationIncomplete(Off, _)` | `On` | `LogWarning` (rollback) |
| `OffRequested` | `TimerTick` (at OFF deadline) | `On` | `LogWarning` via timeout |

**Key semantic difference from pre-Phase-2:**  
Previously `AckOff` and `ActuationIncomplete(On)` both landed in `Off`.  
Now both land in **`Ready`** — the assembly is still active, only the physical lamp is dark.  
`Off` is now strictly "assembly not started" (before `BecomeOn`, or after `BecomeOff`).

---

## Affected production files

| File | Change |
|---|---|
| `vehicle_state/front_headlamp.rs` | `HeadlampState::Ready`; `HeadlampMessage::BecomeOn/BecomeOff`; updated `apply_off_ack`, `evaluate_lux`, `recover_incomplete`; added `apply_become_on/off` |
| `fsm/machineries.rs` | `ZoneId { Headlamp }` |
| `fsm/mod.rs` | Re-export `ZoneId` |
| `published.rs` | `PublishedHeadlampState::Ready`; updated `From<&HeadlampState>` |
| `twin_runtime/detectors/lighting_unsafe.rs` | Detector guard: fire on `Off` **or** `Ready` |
| `twin_runtime/controller/virtual_car_actor.rs` | `front_headlamp_confirmed_direction`: `OffRequested→Ready` (was `→Off`) |
| `twin_runtime/controller/vehicle_controller.rs` | `initial_headlamp_ctx: Option<HeadlampContext>` added to `VehicleControllerRuntimeOptions` |

## Affected test files

| File | Change summary |
|---|---|
| `test/headlamp_lifecycle_contract.rs` | **NEW** — 9 RED→GREEN tests for BecomeOn, BecomeOff, Ready lux, AckOff→Ready, etc. |
| `test/lighting_step_contract.rs` | 7 tests updated: lux-trigger tests now use `Ready` starting state; `AckOff`/timeout/incomplete results now assert `Ready` |
| `test/operational_policy_contract.rs` | `ctx_driving_dangerous_after_failed_on()` helper uses `Ready`; one timeout assertion updated |
| `test/headlamp_ack_timer_contract.rs` | Both tests: `initial_headlamp_ctx: Ready`; final `wait_headlamp_state` updated to `Ready` |
| `test/headlamp_reply_contract.rs` | Test uses `initial_headlamp_ctx: Ready`, drains `AssembliesReady` record, `expected_headlamp_after_on_ack_journey` starts from `Ready` |
| `test/actor_contract.rs` | 3 tests: inline setup with `initial_headlamp_ctx: Ready`; NACK test final state updated to `Ready` |
| `test/quiescence_actor_contract.rs` | `initial_headlamp_ctx: Ready`; final headlamp assertion updated to `Ready` |
| `test/mod.rs` | `install_with_actuation` marked `#[allow(dead_code)]` with guidance on `initial_headlamp_ctx` |
| `twin_runtime/detectors/lighting_unsafe.rs` (inline) | 2 new positive tests for `Ready` state |

## Test protocol bridge (`initial_headlamp_ctx`)

The Brain-to-Headlamp `BecomeOn` message is wired end-to-end in **Phase 5**.  
Until then, tests that need the headlamp in `Ready` state set  
`VehicleControllerRuntimeOptions::initial_headlamp_ctx = Some(HeadlampContext { state: Ready, .. })`.  
This is the Phase 2 test-time shim — no production path is bypassed; the headlamp actor starts  
already in the `Ready` state as if `BecomeOn` had already been processed.

---

## See also

- State transition diagram: `diagrams/headlamp_assembly_state_transition.md`
- Phase 1 notes: `brain_fsm_redesign_plan.md` § Phase 1
- Next phase: Phase 3 (turn barrier — zone reply tracking for `AssembliesReady`)
