# Brain FSM Redesign — Phase 10 Work Items
## Documentation Consolidation and Remaining simulation-4 Objectives

**Status: PENDING (next iteration).**  
**Depends on:** Phases 1–9 complete and committed.  
**Nature:** Not a single coherent change but a collection of independent deferred items —
each can be a standalone PR on its own branch.

---

## Item A — `brain_fsm_redesign_impl_Phase_8.md` documentation cleanup

**Effort:** Small (1–2 hours).  
**Branch:** `docs/phase-8-accuracy`

The Phase 8 doc was written as a plan before implementation.  Its header was updated to flag
the superseded status, but the internal code snippets (D2, D4, Steps 3–5) still show the
intermediate `&'static [AssemblyId]` design as if it were final.

**Tasks:**
- In D2: add a note below the code block: *"This struct-variant syntax is the Phase 8
  intermediate design; Phase 9 changed it to `PreparingToStart(BTreeSet<AssemblyId>)`."*
- In D4 (step.rs snippet): add a note that the `remaining_assemblies` mutation block was
  deleted in Phase 9.
- In the Deletion checklist: mark `pending_assemblies` row as "renamed then deleted (Phase 9)".
- The Revisit section and Discussion checkpoint are already correct.

---

## Item B — README and blog compilation

**Effort:** Medium (half-day).  
**Branch:** `docs/readme-blog`

**README (`README.md`):**
- Architecture overview: Digital Twin → VirtualCarActor → HeadlampActor + WiperActor
- FSM state diagram: `Off → PreparingToStart → Idle ↔ Driving → PreparingToStop → Off`
- Assembly topology: `ALL_ASSEMBLIES` as single source; adding a new assembly = one-line edit
- `TurnBarrier` drain loop: brief explanation of the reorder-buffer guarantee
- Test count and coverage summary

**Blog (`blog/draft.md`):**  
Use the staged redesign as a narrative:
1. The problem: `begin_fsm_turn` complexity and sprinkled assembly knowledge
2. Phase 1–4: vocabulary, generic zone envelope, `TurnBarrier`
3. Phase 5–6: wiring real barriers, deleting speculative execution
4. Phase 7: second assembly (Wiper) as architecture validation
5. Phase 8–9: FSM as single source of topology; BTreeSet countdown
6. What remains (Phase 10)

Source: `brain_fsm_redesign_plan.md`, `brain_fsm_redesign_impl_Phase_{2..9}.md`,
`findings/`, `diagrams/`.

---

## Item C — CAN emulation for `PowerOn` / `PowerOff`

**Effort:** Small–Medium.  
**Branch:** `feat/can-emulation`

**Problem:** `PowerOn` / `PowerOff` events are currently injected programmatically.
The original plan (`simulation_4.ralph.plan.md`) required them to come from a CAN frame.

**CAN frame spec:**
- ID: `0x100`
- Payload `01 00 00 00 00 00 00 00` → `FsmEvent::PowerOn`
- Payload `00 00 00 00 00 00 00 00` → `FsmEvent::PowerOff`

**Design options:**
1. Simple function: `fn emulate_ignition_switch(payload: [u8; 8]) -> Option<FsmEvent>`
   in a new `crates/common/src/can_emulator.rs` module.  Wire into the gateway layer.
2. Actor: a `CanBusEmulatorActor` that receives raw frames and forwards `FsmEvent` messages
   to `VirtualCarActor` — cleaner separation, easier to test.

**Recommendation:** Start with option 1 (function) to establish the mapping; wrap in an actor
if the gateway integration demands it.

**Tests:** Unit test the payload→event mapping; integration test via the existing e2e harness.

---

## Item D — Non-blocking actuation

**Effort:** Medium.  
**Branch:** `refactor/nonblocking-actuation`

**Problem:** `actuation_manager.execute()` is `.await`-ed directly inside
`virtual_car_actor.rs`, holding the actor's execution thread during CAN transmission.

```rust
// Current — blocks VirtualCarActor thread
runtime_state.actuation_manager
    .execute(&other_action, &runtime_state.twin_car)
    .await
```

**Proposed fix:** Send the actuation command as a ractor message to the relevant assembly actor
(e.g. `HeadlampActor`), which runs `actuation_manager.execute()` on its own thread.
`VirtualCarActor` does not `.await` the result — it is fire-and-forget from the coordinator's
perspective (the physical CAN bus confirmation loop is handled by the `TellBackWait` mechanism
already in place).

**Files to change:**
- `virtual_car_actor.rs` — replace `.await` on `actuation_manager` with a ractor `tell`
- `headlamp_actor.rs` (and `wiper_actor.rs`) — add a new message variant for actuation

**Reference:** `findings/single-thread-guarantee.md` Category 2.

---

## Item E — Code commenting pass

**Effort:** Small (can be done incrementally).  
**Branch:** `docs/code-comments`

Every `pub` and `pub(crate)` function in the core call tree must have a doc comment.
Priority order (highest impact first):

1. `zone_message_for_event` — routing decision point
2. `begin_fsm_turn` — three-path dispatch
3. `try_drain_barrier_queue` — reorder-buffer drain loop
4. `apply_committed_quiescence` — action execution
5. `step` / `transition` / `output` — FSM pipeline functions
6. `TurnBarrier` struct and all its constructors

**Standard:** Each doc comment must cover:
- What the function does (one sentence)
- Its preconditions / invariants
- Its relationship to the caller and callee (which function calls it and why)

**Reference:** `findings/Code-commenting-plan.md`.

---

## Item F — Actor-level fuzz / steady-state tests

**Effort:** Medium.  
**Branch:** `test/actor-level-fuzz`

**Problem:** `fsm_properties.rs` uses `proptest` at the FSM function level (`transition`,
`twin_turn`).  There are no tests that exercise the full actor message loop.

**Required tests:**
1. **Ignition cycle invariant:** Spawn `VirtualCarActor`; send `PowerOn` + enough
   `AssemblyZoneReady` events to reach `Idle`; send random `FsmEvent`s; send `PowerOff` +
   enough `AssemblyZoneReady` events; assert final state is `Off`.
2. **Queue drain invariant:** Inject multiple `FsmEvent`s while a barrier is pending;
   assert `barrier_queue` drains to empty within a bounded number of events after all
   `AssemblyZoneReady` events arrive.
3. **No-panic property:** Send any sequence of valid ractor messages; assert the actor
   does not crash.

**Tooling:** Standard `tokio::test` with a spawned actor; `proptest` for the random sequence.

---

## Item G — Embedded readiness: `ArrayVec` migration *(optional)*

**Effort:** Trivial.  
**Branch:** `refactor/arrayvec-countdown` (only if embedding target confirmed)

**Trigger:** Only required if the FSM is compiled for a bare-metal ECU with no allocator.

**Change:** In `machineries.rs`, replace `BTreeSet<AssemblyId>` with
`arrayvec::ArrayVec<AssemblyId, MAX_ASSEMBLIES>` in `FsmState::PreparingToStart` and
`PreparingToStop`.  Add `arrayvec` to `Cargo.toml`.

No logic changes anywhere else — `ArrayVec` implements `Iterator`, `PartialEq` (on live
prefix), and `is_empty()` with identical semantics.

**Reference:** `brain_fsm_redesign_impl_Phase_8.md` "Revisit" section.

---

## Dependency order

```
Item A (doc cleanup)     — independent, can start now
Item B (README/blog)     — depends on Item A being clean
Item C (CAN emulation)   — independent
Item D (non-blocking)    — independent; Item C useful first (end-to-end test path)
Item E (comments)        — independent, can be done incrementally
Item F (fuzz tests)      — depends on Item D (stable actor thread model)
Item G (ArrayVec)        — only if embedded target confirmed; independent of all
```
