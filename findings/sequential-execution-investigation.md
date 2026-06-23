# Sequential Execution Guarantee — Investigation Results

## Question

Does any existing test validate that the FSM events are processed sequentially
(w.r.t. the actor's `pending_turn` / `fsm_backlog` serialisation) when events
are **delivered concurrently** from multiple tokio tasks?

## Finding

**No.** All existing tests drive the actor sequentially — each call to
`send_message`, `submit_fsm_event`, `submit_physical_car_event` is followed by
an `.await` on the send itself (or on a subsequent `get_snapshot` / `rx.recv()`)
before the next event is submitted. This means there is **no test** that:

1. Spawns multiple tokio tasks that send events to the same actor simultaneously.
2. Checks that the backlog (`fsm_backlog`) correctly buffers events when a
   `pending_turn` is active.
3. Verifies that events from different producers are drained in FIFO order.
4. Stress-tests the actor under concurrent event delivery to confirm the
   `pending_turn` guard never leaks a parallel turn.

## What we do have

### Scenario tests (`scenarios_smoke.rs`)
- Send multiple events in rapid succession (e.g. `UpdateRpm(2000)` then
  `UpdateRpm(7500)`), but these are sequential calls from a single task.
- `send_message` is synchronous (atomic queue push), so both messages are
  enqueued before the actor processes the first one. However, the test never
  asserts **which order** they were processed — it just checks the final state.

### Headlamp reply / ACK timer contracts
- Verify tell-back timeout, ACK/NACK paths, and quiescence hop synthesis.
- All events arrive one-at-a-time from a single tokio task.

### E2E tests (`front_headlamp_e2e.rs`, `front_headlamp_bus_e2e.rs`)
- Also single-task, sequential event submission.

### Transition ledger tests (`quiescence_actor_contract.rs`)
- Read back individual ledger rows via `mpsc::Receiver` to verify per-hop
  sequencing (e.g., `record_seq` monotonicity).
- But events are still submitted one-at-a-time, awaiting each handler to
  complete before the next `submit_physical_car_event` call.

## What's missing

A **concurrent event delivery stress test** that:

1. Spawns N tokio tasks.
2. Each task sends a specific FSM event to the same actor simultaneously.
3. The test waits for all events to be processed (e.g., via transition ledger
   or final state assertion).
4. Asserts that:
   - The `record_seq` values are contiguous and monotonic.
   - The final FSM state matches a known deterministic outcome given the
     sequence of events (i.e., no events were dropped or reordered).
   - (Optional) Inspect the backlog size or `pending_turn` transitions via
     a diagnostic channel.

## Why it matters

The `pending_turn` / `fsm_backlog` serialisation is the **key invariant** of
the actor's FSM processing. If it were broken (e.g., two events bypassing the
backlog guard due to a race), the FSM could enter an inconsistent state because
two turns would be in-flight simultaneously. A concurrent stress test would
make this invariant regression-proof.

## Proposed test plan

### Test name
`scenario_concurrent_event_delivery_maintains_ordering`

### Setup
- `VehicleController` with `transition_tx` (ledger channel) and
  `actuation_command_tx` (to drain actuation commands and prevent backpressure).

### Events to send (concurrently)
Four events that each trigger an FSM turn, none of which require headlamp
tell-back (so `pending_turn` stays `None` and backlog is exercised trivially):

| Event | Expected intermediate state |
|-------|---------------------------|
| `PowerOn` | `Idle` |
| `UpdateAmbientLux(20)` (dark) | — (drives toward headlamp-on, but RPM needed) |
| `UpdateRpm(2500)` (driving) | `Driving` |
| `UpdateAmbientLux(50000)` (daylight) | `Idle` |

### Concurrent delivery
```rust
let events = vec![
    FsmEvent::PowerOn,
    FsmEvent::UpdateAmbientLux(20),
    FsmEvent::UpdateRpm(2500),
    FsmEvent::UpdateAmbientLux(50000),
];
let handles: Vec<_> = events.into_iter().map(|evt| {
    let controller = controller.clone();
    tokio::spawn(async move {
        controller.submit_fsm_event(evt).await
    })
}).collect();
for h in handles {
    h.await.expect("task panicked")?;
}
```

### Assertions
- Read all ledger rows from `rx`. Verify `record_seq` is 1, 2, 3, 4 (contiguous).
- Verify the final snapshot state matches what the **sequential** execution of
  those events would produce (e.g., `Idle`).
- Verify no duplicate or missing `record_seq` values.

### Variant with pending_turn exercised
A second test that sends an event requiring headlamp tell-back (e.g.,
`UpdateAmbientLux(20)` while `UpdateRpm(2500)` is active) concurrently with
other events, to exercise the `fsm_backlog` push path.
