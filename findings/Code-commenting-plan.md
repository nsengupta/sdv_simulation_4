**Action Plan**

1. Read file `crates/common/src/twin_runtime/controller/virtual_car_actor.rs` lines 215–240 to see the full context around line 224 and 232.
2. Identify the exact code at line 224 that pushes `evt_arrived` (likely onto a queue/channel/vec).
3. Identify the exact code at line 232 that pops `evt_arrived` (likely from the same queue/channel/vec).
4. Determine if the push at line 224 and pop at line 232 reference the **same underlying data structure** (same field name, same channel, same vec).
5. Check if there is any conditional logic or thread boundary between the push and pop that could cause them to operate on different instances.
6. Check if the pop operation at line 232 is **unconditional** (i.e., always pops) or if it first checks whether a matching event exists (e.g., comparing IDs).
7. Verify whether other code paths could push an `evt_arrived` event between lines 224 and 232.
8. Conclude whether line 232 specifically pops the event pushed at line 224, or if it could pop a different/older event.
9. Document findings with line numbers and reasoning, to this file itself as an appendix.
10. Output final answer.

## Appendix — Findings

### Context
The file has been modified since this plan was written (comments were added), so line numbers have shifted. The original "line 224 push" and "line 232 pop" correspond to the following in the current file:

**Push** (current line 224):
```rust
runtime_state.fsm_backlog.push_back((evt_arrived, now));
```
This is inside the `Fsm(evt_arrived)` match arm, guarded by `if runtime_state.pending_turn.is_some()` — i.e., it only fires when a turn is already in-flight awaiting tell-back.

**Pop** (current line 617, function `pump_fsm_backlog`):
```rust
let Some((evt, now)) = runtime_state.fsm_backlog.pop_front()
```
Called at four points (current lines 232, 247, 254, 262), always immediately after a handler that might have consumed a pending turn.

### 4. Same data structure?
**Yes.** Both reference `runtime_state.fsm_backlog`, a `VecDeque<(FsmEvent, Instant)>` field on the `VirtualCarRuntimeState`. The actor holds `&mut VirtualCarRuntimeState` throughout, so there is no aliasing.

### 5. Conditional logic / thread boundary?
**No thread boundary.** Both push and pop execute synchronously (`.await` points are *between* push and the next pop, but the actor's `handle` future is single-threaded — ractor dispatches one message at a time).

There *is* conditional logic: push only happens when `pending_turn.is_some()`. Pop (`pump_fsm_backlog`) only happens when `pending_turn.is_none()`. These conditions are complementary across the `.await`:

1. Push occurs → `pending_turn` is `Some` → `begin_fsm_turn` is **not** called for this event.
2. Later, some handler consumes the pending turn → `pending_turn` becomes `None`.
3. `pump_fsm_backlog` runs in a `while` loop: pops the oldest event from the queue, calls `begin_fsm_turn` on it. If `begin_fsm_turn` sets `pending_turn` back to `Some`, the loop exits (leaving remaining events in the backlog).
4. The next handler call (after the backlog drain returns) will re-enter `pump_fsm_backlog`.

### 6. Unconditional or conditional pop?
The pop is **conditional in a loop**:
```rust
while runtime_state.pending_turn.is_none() {
    let Some((evt, now)) = runtime_state.fsm_backlog.pop_front() else {
        break;  // queue empty — exit loop
    };
    Self::begin_fsm_turn(brain, runtime_state, evt, now).await?;
}
```
It pops one event at a time, calling `begin_fsm_turn` for each. If `begin_fsm_turn` sets `pending_turn` (i.e., starts a headlamp wait), the loop condition fails and the rest of the backlog remains buffered. It does **not** compare IDs — it processes events strictly in FIFO order (oldest first).

### 7. Other push paths?
**No.** The only `fsm_backlog.push_back` in the entire file is the one at the current line 224. No other handler pushes to the backlog.

### 8. Does line 232 pop the event pushed at line 224?
**Not necessarily the exact same event, but same queue.** Consider this sequence:

| Step | Event | `pending_turn` | Backlog |
|------|-------|----------------|---------|
| 1 | `Fsm(A)` arrives | `None` | `[]` |
| 2 | `begin_fsm_turn(A)` → headlamp wait | `Some` | `[]` |
| 3 | `Fsm(B)` arrives, push to backlog | `Some` | `[B]` |
| 4 | `Fsm(C)` arrives, push to backlog | `Some` | `[B, C]` |
| 5 | `HeadlampZoneReady` arrives for A | `None` (consumed) | `[B, C]` |
| 6 | `pump_fsm_backlog` pops **B**, calls `begin_fsm_turn(B)` | `Some` (B may wait) | `[C]` |

So `pump_fsm_backlog` on line 232 pops *the oldest backlogged event* (`B`), calls `begin_fsm_turn(B)`. If `begin_fsm_turn(B)` sets `pending_turn` (starts a headlamp wait), the loop exits and `C` remains in the queue for a later drain cycle. The push at line 224 and the next pop at line 232 operate on the **same FIFO queue** but are temporally decoupled: multiple pushes may accumulate before the next pop. `B` is always popped before `C` — strict FIFO ordering is preserved.

### Conclusion
The push at (original) line 224 and pop at (original) line 232 reference the **same `VecDeque` field** with no thread boundary. The pop is FIFO and conditional (stops when `pending_turn` is `Some`). There is only one push site. The backlog correctly serialises FSM events: while a turn is pending, new events are queued; when the pending turn is resolved, the backlog is drained one-at-a-time, with each event potentially starting a new wait cycle.
