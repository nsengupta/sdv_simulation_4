# Finding: Async/.await inside `VirtualCarActor::handle` — Does it break the single-threaded actor guarantee?

## The actor model guarantee (ractor)

`ractor` processes messages sequentially: one message at a time per actor, on a single async task.
The `handle(&self, myself, message, state)` method is `async fn`. The framework **awaits** the
returned future to completion before dispatching the next message. As long as the future does
not yield to the tokio runtime in a way that lets **another message from the same actor** interleave,
the guarantee holds.

## All .await points inside `handle` and their nature

Every `.await` inside `VirtualCarActor::handle` and its callees falls into one of three categories:

### Category 1 — Intra-actor synchronous (safe)

- `begin_fsm_turn`, `pump_fsm_backlog`, `on_headlamp_zone_ready`, `on_tell_back_timeout`,
  `on_headlamp_zone_spontaneous`, `commit_resolved_turn`, `apply_committed_quiescence`:
  these are all `async fn` but they are **structurally synchronous** — they call `.await`
  only on other async functions within the same actor, with no `.await` on external I/O or
  tokio channels. The await chain is:

  ```
  handle → begin_fsm_turn → begin_headlamp_wait (no .await inside, sets pending_turn)
         → pump_fsm_backlog → begin_fsm_turn (same as above)
         → on_headlamp_zone_ready → commit_resolved_turn → apply_committed_quiescence
              → actuation_manager.execute(...).await    ← SEE CATEGORY 2
              → try_emit (sync, no .await)
         → on_tell_back_timeout → tell_headlamp_zone (sync, no .await)
                                → arm_tell_back_timer (sync, no .await)
  ```

  None of these contain a `.await` that could yield to the runtime while another message
  from the **same actor's** mailbox is processed, because ractor delivers one message at a
  time and waits for the future to complete.

### Category 2 — actuation_manager.execute(...).await (potential concern)

`DefaultActuationManager::execute` performs `tx.send(...).await` on a tokio `mpsc::Sender`.

This **does yield** to the tokio runtime. If the channel is full (receiver is slow), the
`.await` will suspend the actor's task until capacity is available. While suspended,
**no other message from this actor's mailbox is processed** — ractor waits for the
`handle` future to complete — so the single-threaded message order is preserved.

However, there is a **subtle correctness risk**: if the channel is *always* full (deadlock or
backpressure), the actor is stuck indefinitely. The existing `TODO` comments in
`actuation_manager.rs` acknowledge this:
```
// TODO(actuation-child-actor): offload connector I/O to a child actor
// and keep parent actor loop non-blocking under slow transports.
```

The current design tolerates this because:
- `RequestFrontHeadlampOn` / `RequestFrontHeadlampOff` are the only actions that hit `.await`
- The CAN egress channel has capacity 64  
- The CAN writer is a separate tokio task (non-blocking)

**Verdict:** Does NOT break single-threaded message ordering, but DOES introduce
a potential actor-stall under backpressure. The TODO to offload to a child actor
is the correct fix.

### Category 3 — `send_after` for tell-back timers (safe, external)

`arm_tell_back_timer` uses `brain.send_after(...)` which schedules a timer message
delivery via ractor's own timer wheel. This is a ractor primitive, not a raw tokio
`spawn`, and delivers the `TellBackTimeout` message through the actor's normal mailbox.
No concurrency concern.

## What about the gateway runtime?

The gateway (`gateway_runtime.rs`) runs the actor on an async tokio task alongside
several `tokio::spawn` loops:

| Spawned task | Async? | Interacts with actor via |
|---|---|---|
| Timer tick loop | `tokio::sleep + controller.submit_physical_car_event().await` | `VehicleController::submit_physical_car_event` → `actor.cast(...)` |
| CAN ingress dispatch loop | `rx.recv().await + controller.submit_physical_car_event().await` | Same as above |
| Front-headlamp CMD publisher | `rx.recv().await + blocking CAN write` | Reads from mpsc channel (actor writes to it via actuation_manager) |
| Ingress log printer | `rx.recv().await + println!` | Reads from mpsc channel (actor-independent) |
| Transition log task | `rx.recv().await` | Reads from mpsc channel (actor writes via try_emit) |
| CAN reader thread | blocking `read_frame()` loop | `UnboundedSender.send(...)` into mpsc channel |

All actor interactions from these spawned tasks go through:
- `actor.cast(...)` (fire-and-forget → mailbox) ← **safe, ractor enqueues in mailbox**
- `actor.call(...)` (RPC → mailbox) ← **safe, ractor enqueues in mailbox**
- `mpsc::Sender::send(...)` → actor reads via `try_send` in `apply_committed_quiescence`
  ← **safe, non-blocking on actor side**

The spawned tasks do NOT share `&mut VirtualCarRuntimeState` or run code inside the
actor's `handle`. They communicate through channels. The actor model guarantee holds.

## Summary

| Async point | Breaks guarantee? | Risk |
|---|---|---|
| `begin_fsm_turn` / `pump_fsm_backlog` / `on_*` call chain | No | None — all structurally synchronous |
| `actuation_manager.execute(...).await` | **No** (ordering preserved) | Actor-stall under channel backpressure; child-actor offload planned |
| `try_emit` on diagnostic/transition sinks | No | `try_send` is synchronous, returns immediately |
| Timer via `send_after` | No | Ractor primitive, goes through mailbox |
| Gateway spawned tasks | No | All communicate via channels / mailbox |
| CAN reader thread (OS thread) | No | Thread-safe channel, no shared state |

**Conclusion:** The async `.await` usage does NOT break the single-threaded
message-handling guarantee of the actor model. Every `.await` inside `handle`
either resolves internally (no yield to runtime) or, when it does yield
(actuation_manager), ractor preserves message ordering by awaiting the full
future before dispatching the next message. The only correctness concern is
actor-stall under actuation channel backpressure, which is acknowledged with
a TODO for child-actor offloading.

## More insights

The key insight is in the distinction between **ractor's message ordering guarantee** and **tokio's work-stealing scheduler**.

## Why tokio work-stealing can break the guarantee

The `ractor` crate's `handle` method is `async fn`. When the actor framework calls it, it `poll`s the returned future. If that future yields (returns `Poll::Pending`), control returns to the tokio runtime — not to ractor's dispatch loop. Here's the critical path:

### The scenario

Consider `actuation_manager.execute(...).await`:

```rust
// DefaultActuationManager::execute
let _ = tx.send(ActuationCommand::SwitchFrontHeadlampOn { correlation_id }).await;
```

This calls `mpsc::Sender::send()` which is an **async** method that yields when the channel is full (backpressure). When it yields:

1. The actor's `handle` future returns `Poll::Pending` to tokio.
2. Tokio's **work-stealing scheduler** sees this task is blocked and may **steal** the worker thread to run other tasks from the global queue.
3. Concurrently, another tokio task (e.g., the CAN ingress dispatch loop, timer tick loop, or even a **second actor**) is running on a different worker thread.
4. That other task calls `controller.submit_physical_car_event(physical).await` which does `actor.cast(...)` — a mailbox enqueue.

### The race

`actor.cast(...)` in ractor is an atomic queue push. The message is enqueued into the actor's mailbox from **another thread** (the other tokio task's worker). When the actor's `handle` future is later re-polled (after the `mpsc::Sender` acquires capacity), **ractor does not check the mailbox during the `poll` of the ongoing future** — it only checks the mailbox when dispatching the *next* message.

So the sequence is:

1. Actor is processing message **A** (e.g., `Fsm(UpdateAmbientLux)`).
2. Inside `handle`, it calls `actuation_manager.execute().await` → yields.
3. **Message B** arrives via `actor.cast()` from another tokio task (work-stealing thread).
4. Message B is enqueued in the mailbox. **But the actor is still logically processing message A** — the `handle` future for A has yielded but not returned.
5. When the `send` acquires capacity, the `handle` future for A resumes, completes, and returns to ractor.
6. Ractor then polls the mailbox and dispatches message B.

**This is still ordered correctly** — B is processed after A. So where's the break?

### The real break: concurrent modification of **shared state outside the mailbox**

The tell-back timer uses `brain.send_after(...)` which schedules a future via ractor's timer wheel. That timer fires from a **tokio timer task** (spawned internally by ractor). When it fires, it does an atomic enqueue just like `cast`. Fine.

But consider what happens **when the actor's `handle` yields during `actuation_manager.execute().await`**:

The actor has `&mut VirtualCarRuntimeState`. While the future is yielded, this `&mut` reference is **suspended on the stack** (in the generated state machine of the async fn). No other code can access it — Rust's borrow checker prevents that. So **within the same tokio task**, safety is guaranteed.

**However**, the `mpsc::Sender::send().await` call is on a channel. If the receiver of that channel is on **another tokio task** that itself calls back into the actor... But it doesn't — the CAN command publisher is a separate task that writes to a CAN socket and never calls the actor.

### The real real break: the diagnostic sink and transition sink are `try_send` (sync)

These don't yield. So the only yielding `.await` is the actuation channel. The question is whether the **can egress channel receiver** (in `spawn_front_headlamp_command_publisher`) could, under work-stealing, delay the `send` completion long enough that the actor remains yielded while many messages accumulate in the mailbox.

When the actor's `handle` finally resumes, it processes them all sequentially. **Ordering is preserved** because each message was either processed before the yield (and completed) or queued in the mailbox during the yield and processed after.

### The actual break condition

The break occurs not with **ractor message ordering** but with **timeliness and liveness**:

1. **Work-stealing can delay re-polling.** When the actor yields, tokio may move the task to another thread's queue. If that thread is busy, the actor stays yielded longer than necessary. During this time, the mailbox accumulates messages. The actor's logical view of the world is frozen at the moment it yielded.

2. **Tell-back timers are ractor-internal.** The `send_after` creates a tokio delay future. If the actor is yielded when the timer fires, the `TellBackTimeout` message is enqueued in the mailbox but **not processed** until the actor's `handle` future completes and returns. If the actor is stuck on `actuation_manager.execute().await` for longer than `ZONE_TELL_BACK_WAIT`, the timeout fires, enqueues the message, but the actor doesn't process it until the channel send completes. This means the **tell-back timeout is measured in wall-clock time including the time the actor spent yielded** — not the time it spent actively processing. For a 100ms timeout, if the channel is blocked for 50ms, the actor only has 50ms of effective processing time before the timeout fires.

3. **The backlog can grow unboundedly.** While the actor is yielded on `actuation_manager.execute().await`, new FSM events and `HeadlampZoneReady` replies from the twinlet can arrive and pile up in `fsm_backlog`. Since `pending_turn` is likely `Some` (the yield happened during commit resolution after a headlamp wait resolved), these backlog entries are pushed but not drained. On resume, `pump_fsm_backlog` drains them all — but the wall-clock delay is observable.

### Summary

| Risk | Cause | Severity |
|---|---|---|
| Message reordering | **Impossible** — ractor dispatches one-at-a-time, `&mut` reference on stack prevents concurrent access | None |
| Mailbox pile-up during yield | Actor yields on `actuation_manager.execute().await`; work-stealing delays re-poll | Medium — causes observable latency spikes |
| Tell-back timeout measured in wall time | Timer fires while actor is yielded; timeout includes yield duration | Medium — may cause spurious retries if yield is long |
| Backlog growth | New events arrive while actor yielded; backlog drains on resume | Low — bounded by memory, but latency spikes |
| **Liveness** | If CAN egress channel is *permanently* full (receiver deadlocked), actor is stuck forever | **High** — total system stall |

The **fundamental break** is not message ordering — that's preserved by Rust's ownership system + ractor's dispatch model. The break is **liveness**: a yielding `.await` inside `handle` makes the actor's responsiveness depend on an external channel's capacity, and tokio work-stealing can amplify the wall-clock delay unpredictably. This is exactly what the TODO comment calls out as needing a child-actor offload.
