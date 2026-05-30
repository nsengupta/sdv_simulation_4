# Design notes: runtime observation channels & the road to actorification

Scope: the three optional channels injected via `VehicleControllerRuntimeOptions`
(`transition_tx`, `diagnostic_tx`, `actuation_command_tx`), the snapshot RPC, the
transition ledger, time handling, and how all of this should be shaped given the
upcoming split of the monolithic `VirtualCarActor` into a parent FSM actor + child
actuation/observability actors.

Anchors in the code at time of writing:
- `crates/common/src/engine/controller/vehicle_controller.rs` — `VehicleControllerRuntimeOptions`.
- `crates/common/src/engine/controller/virtual_car_actor.rs` — the actor loop (persist → emit record → run actions → emit diagnostics).
- `crates/common/src/engine/controller/actuation_manager.rs` — `DefaultActuationManager`, the no-op TODOs.
- `crates/common/src/fsm/step.rs` — pure `step(state, ctx, event, now) -> StepResult`, `TransitionRecord`.
- `crates/common/src/engine/op_strategy/transition_map.rs` — `transition` / `output` (where actions are born).
- `crates/common/src/transition_sink.rs`, `crates/common/src/diagnostic/mod.rs` — the two sinks.
- `crates/common/src/digital_twin/{mod.rs,car_behaviour_checker.rs}` — `verify_all_invariants`, the laws.
- `crates/common/src/engine/controller/actuation_contract.rs` — `ActuationCommand` / `ActuationFeedback` / `CorrelationId`.

---

## Q1 — Do `transition_tx` and `diagnostic_tx` really need to be separate?

**Verdict: keep them separate, but they are not peers.** One is a *fact ledger*, the
other is a *best-effort operational log*. The state-transition diagnostic is indeed
derived and is currently emitted twice — that duplication should be removed.

Why they differ structurally today:

| | `transition_tx` (`RawTransitionRecord`) | `diagnostic_tx` (`DiagnosticMessage`) |
|---|---|---|
| Channel | bounded `mpsc::channel(N)` | **unbounded** |
| Delivery | lossless-or-error (Full/Closed surfaced) | best-effort, fire-and-forget |
| Ordering | `sequence_no`, total order | none guaranteed |
| Audience | machines: replay, invariant checks | humans/logs |
| Sources | parent FSM only (1 per step) | many: init, timer tick, actuation failure, sink-overflow meta |

A diagnostic is *partly* derivable from transitions — `diag_state_transition` is
literally `(identity, next_state)` taken straight from a transition. But the diagnostic
stream also carries events that are **not** transitions: the init message, `TimerTick`
heartbeats, actuation failures, and "transition sink full/closed" meta-diagnostics. So
you cannot reconstruct the diagnostic stream from the transition stream alone.

The clean mental model:
- `transition_tx` = the **primitive, authoritative FSM fact ledger** (one record per event).
- `diagnostic_tx` = a **cross-cutting, multi-source presentation/telemetry bus**.

Actorification angle: after the split, child actors (actuation, headlamp connector)
will need to emit diagnostics but they do **not** produce FSM transitions. So the
diagnostic bus must remain a shared, separate channel — this *strengthens* the case for
two channels. The cleanup is the other direction: stop the parent from directly emitting
the *state-transition* diagnostic. Let a future observer/telemetry actor subscribe to the
transition ledger and **project** those diagnostics. The parent then emits to
`diagnostic_tx` only for things that are not transitions (lifecycle, actuation outcome,
sink overflow).

---

## Q2 — Can we test `actuation_command_tx` by injecting the harness's own tx/rx?

**Verdict: yes — that is already the intended seam, and it's the idiom the repo uses.**

`actuation_command_tx: Option<mpsc::Sender<ActuationCommand>>` is injected through
`VehicleControllerRuntimeOptions`. A test creates `(tx, rx) = mpsc::channel(N)`, passes
`tx` in, drives events, then asserts on `rx.recv()` (e.g.
`ActuationCommand::SwitchFrontHeadlampOn { correlation_id }`). This mirrors exactly how
`actor_contract.rs::scenario_raw_transition_records_are_emitted_in_order` already tests
`transition_tx`.

**The harness plays the role of the future child actor.** Post-actorification the
actuation child owns the rx side, performs connector I/O, and feeds `ActuationFeedback`
back. In tests the harness substitutes for that child: it owns rx (asserts the outbound
command) and can inject the ack/nack back as events
(`submit_physical_car_event` / `submit_fsm_event`) to close the round trip. The
front-headlamp e2e tests already drive the feedback side via `PhysicalCarVocabulary`.

Caveats:
- `CorrelationId.session_id` is derived from `SystemTime::now()` → **non-deterministic**.
  Assert on structure and on `sequence_no` monotonicity, never on the exact `session_id`.
- Use a channel capacity large enough that the actor never hits backpressure mid-scenario,
  or drain promptly, to keep ordering assertions deterministic.

Suggested ergonomics: a test helper returning `(controller, actuation_rx)` plus a
one-liner to inject the matching ack — so a test reads "send command → observe command →
inject ack → observe resulting transition."

---

## Q3 — `GetStatus` / `RefreshStatus`: the RESP may be stale. How to live with it.

**Verdict: keep `GetStatus`, keep it pure/read-only, and make staleness *explicit*
with a sequence stamp. Do not add a mutating `RefreshStatus`.**

`GetStatus` is a `ractor` `call` (RPC reply port). It is processed in mailbox order, so
the reply reflects the actor's state *at the moment it processes that message*. By the
time the caller reads the value, newer events may already be applied. The snapshot is
never *wrong* — it is *as-of a point in the event order*. This is intrinsic to async
actors; you cannot remove it, only make it legible.

How to deal with the inevitability:
1. **Stamp the snapshot with a logical version** = the last applied transition
   `sequence_no` (the actor already maintains `next_sequence_no`). Add e.g.
   `as_of_seq: u64` to `DigitalTwinCar` (or to the reply). A consumer then knows "this
   snapshot reflects events ≤ N" and can reconcile it against `transition_tx` records
   with `sequence_no > N`. Today `DigitalTwinCar` carries no version → staleness is
   invisible.
2. **Prefer the transition ledger over polling for verification.** Polling `GetStatus`
   races with in-flight events; the ledger is exact. Use `GetStatus` for *settled*
   assertions (after draining) and for live UIs.
3. **Keep it read-only.** A `RefreshStatus` that *forces recomputation* would break the
   documented contract that `GetStatus` "does not call `transition`." Recomputation
   belongs to events, not to a query. If "refresh" means "give me the newest", the
   version stamp + draining the ledger already answers that.

Actorification angle: with child actors, a parent snapshot can only ever summarize the
parent's view; child state (in-flight actuation) is reflected later as feedback events.
The `as_of_seq` stamp is what lets a consumer say "snapshot is at N, but commands up to M
are still outstanding."

---

## Q4 — `transition_tx` should include the actions taken (names + params).

**Verdict: agree, and it's a small, high-value change. Record the *intended* actions
(deterministic, from the pure step), not execution outcomes.**

Today `TransitionRecord` = `{ at, event, old_state, next_state, old_ctx, current_ctx }`.
`StepResult` carries `actions: Vec<DomainAction>` *separately*, and the actor consumes
them in the loop **without** recording them. So a `transition_tx` consumer cannot see
what the FSM decided to do.

Proposed change: add the emitted actions to the record, e.g.
`actions: Vec<DomainAction>` (or a slim `Vec<ActionSummary { name, params }>`).
`DomainAction` already derives `Debug/Clone/PartialEq`, so it's cheap.

Important distinction this preserves:
- The record is produced **before** actions run, by a **pure** step. So `actions` in the
  record = **intended/emitted** actions (deterministic), *not* "succeeded/failed."
- Execution **outcomes** (ack/timeout/nack/failure) are separate facts. Today failures go
  to `diagnostic_tx`; successes come back as feedback events that generate their *own*
  transition records. The loop closes naturally.

Cleanups to fold in:
- Filter out `DomainAction::EnterMode(_)` from the recorded list (it's a runtime control
  hint, not a domain action) — or record it in a separate field.
- Consider embedding the `CorrelationId` of any actuation-producing action so the record
  becomes provenance: transition N → command(corr) → feedback event → transition M.
  (Best done *with* actorification; see Q9.)

**Naming hazard once both numbers live in one record (decided):** the record already
carries `RawTransitionRecord.sequence_no` (the **ledger** counter, Counter A — see Q7),
and the embedded action's `CorrelationId.sequence_no` is a **different** counter (Counter
B, the command counter). The moment both coexist in one struct, two unrelated 1-based
`u64` series share the field name `sequence_no`. **Disambiguate before they coexist:**
rename the ledger field to `record_seq` (or `ledger_seq`), and keep `CorrelationId`'s as
the command axis (e.g. read as `correlation_id.command_seq`). A reader of the
transition/actuation log must never be able to confuse "ledger position" with "command
position." Naming is the contract here.

This single change also resolves Q5 and feeds Q6/Q9.

---

## Q5 — Why aren't `StartBuzzer`/`StopBuzzer`/`PublishStateSync`/`LogWarning` already in `transition_tx`?

**Verdict: because the record carries no actions at all yet (Q4). Do Q4 and these become
observable for free — which is exactly the right home for the still-no-op ones.**

These four are `DomainAction`s minted in `step`/`output` and routed to the
`ActuationManager`, which currently **no-ops** them (the `TODO(actuation-*)` arms). They
have no connector wired, so their *only* meaningful observable today **is their presence
in the action list**. Recording the emitted actions (Q4) means the harness/diagnostics
can already assert e.g. "Driving → ExtremeOperationWarning emitted `StartBuzzer` +
`LogWarning(...)`" without the actuation side doing anything. The no-op `execute()` is
then fine: the *fact* is captured at the ledger, and the *effect* is filled in later
(actuation child actor / egress connector) without changing the ledger contract.

Flagged smell: **`LogWarning` is really observability, not actuation.** It is generated in
two places (`step` for `RejectedPowerOff`, `output` for threshold/extreme messages),
routed as a `DomainAction` the manager no-ops, while a parallel diagnostic stream already
exists. After Q4 it will show up in the record; separately it would be cleaner to route
`LogWarning` **directly to the diagnostic sink** rather than through the actuation path.
Candidate reclassification (small).

---

## Q6 — Run `verify_all_invariants()` over every cut in the journey; tests call checkers as library functions.

**Verdict: correct goal. Expose a *pure, public* state-law entry point that takes
`(&FsmState, &VehicleContext)`; keep `verify_all_invariants()` as the snapshot-level
wrapper. Then tests fold the laws over each captured cut.**

Current shape:
- `DigitalTwinCar::verify_all_invariants()` needs a full twin (identity + state + ctx).
- The individual laws in `car_behaviour_checker.rs` are `pub(super)` — invisible outside
  the `digital_twin` module — and already pure on `(&FsmState, &VehicleContext)`.

Every cut is reconstructable from a `RawTransitionRecord`: it carries
`old_state/next_state` and `old_ctx/current_ctx`. So a journey check iterates records and
evaluates the laws at each `(state, ctx)` cut — non-invasively, with no actor access.

Two frictions to remove:
1. The laws are not reachable from tests → **bump visibility** and add a free function,
   e.g. `pub fn verify_state_laws(state, ctx) -> Result<(), Vec<LawViolation>>`. Keep it
   as a **catalog of named laws** so the harness can report *which* law failed at *which*
   `sequence_no`.
2. `verify_all_invariants()` should become a thin wrapper: identity/health checks +
   `verify_state_laws(...)`. The state-law subset is what runs over the journey (identity
   is constant; health is part of ctx).

Note: `ExtremeOperationWarning(Instant)` carries a non-deterministic instant, but the laws
only read ctx, so cut-checking is unaffected. (See Q7 for equality.)

This is an easy, do-it-now change (mostly a visibility bump + one pure entry point + a
test fold helper).

---

## Q7 — Non-deterministic payloads: `TransitionRecord.at = Instant::now()` and `ExtremeOperationWarning(Instant)`.

**Verdict: agree — keep the `Instant` as elemental data (durations are derived, don't
pre-fold). But separate "elemental capture" from "ordering" and from "equality":**

- **Ordering: use `sequence_no`, not `at`.** `RawTransitionRecord.sequence_no` is a
  monotonic, total, clock-independent order. Within one actor, mailbox order ==
  `sequence_no` order, which is *stronger* than timestamp order (timestamps can tie). I'd
  gently amend the framing "order is determined by the message timestamp": prefer
  `sequence_no` for ordering; use `at`/durations only for *temporal* properties (cooldown
  elapsed, command latency).
- **Capture: keep `at` and keep `began_at` in `ExtremeOperationWarning`.** They are the
  raw material for duration/aberration analysis (e.g. the 5 s cooldown is literally
  `now - began_at`). Don't discard.
- **Determinism: inject the clock (Q8).** The non-determinism enters only at the call
  sites `fsm::step(..., Instant::now())` and `at: now` *inside the actor*. `step` itself
  is already pure w.r.t. time (it takes `now`). A clock seam makes the records and state
  instants reproducible in tests.
- **Equality in tests: compare on the discriminant / projected `VehicleState`, not the
  raw `Instant`.** Tests already do `matches!(s, ExtremeOperationWarning(_))` and avoid
  asserting on `at`. Formalize that: a helper that compares states ignoring the instant,
  or compare via `VehicleState::from(&state)` which drops the instant.

### Q7 addendum — there are TWO `sequence_no` counters; `seq` is per-source, not global

Inspection (whole tree) found two **physically independent** counters that happen to
share the name `sequence_no` and the same 1-based `u64` value space:

| | Counter A — ledger | Counter B — correlation |
|---|---|---|
| Field | `RawTransitionRecord.sequence_no` | `CorrelationId.sequence_no` |
| Stored in | `VirtualCarRuntimeState.next_sequence_no: u64` | `DefaultActuationManager.next_sequence_no: AtomicU64` |
| Bumped | `try_emit_transition_record` (`saturating_add(1)`) | `next_correlation_id` (`fetch_add(1, Relaxed)`) |
| Cadence | **every FSM event** (one per `step`) | **only when a correlated actuation command is emitted** |
| Scope | `car_identity` | `(source_id, session_id)` — and `source_id == car_identity` |
| Leaves process? | no | yes — packed onto CAN, **narrowed to `u32`** in `vehicle_device_bus` codec/can |

(Table uses *current* type names. Per recommendation 7 these rename to:
`TransitionRecord` → `RawTransitionRecord`, current `RawTransitionRecord` →
`PublishedTransitionRecord`, and its `sequence_no` → `record_seq`.)

Findings:
- **No shared state, no aliasing, no cross-assignment.** Bumping one never affects the
  other. There is no bug today: nothing compares them.
- They overlap only in **value range** and **field name**. They count different things on
  different axes and drift apart immediately (e.g. ledger `3` ↔ correlation `1` once the
  first headlamp command is emitted). They must never be cross-referenced.
- This is the concrete evidence for the earlier point: **`seq` is meaningful only
  per-source.** Counter B is *already* namespaced by `(source_id, session_id, seq)` on
  purpose. Counter A is namespaced only by `car_identity` today and has no session.

**Decisions (agreed):**
1. **Naming is the contract.** Rename Counter A's field to `record_seq` / `ledger_seq`;
   keep Counter B as the command axis (`correlation_id.command_seq`). Do this *before* Q4
   puts both in one record. A reader must never confuse ledger position with command
   position.
2. **More actuators → more child actors tomorrow.** Each child is its own source. For the
   **ledger** keep a single writer so `record_seq` stays a true total order — favour a
   dedicated journal/ledger actor that owns Counter A; children *message* it rather than
   minting ledger numbers. For **correlation**, the opposite is correct: each
   actuator/child owns its own Counter B, namespaced by its own `source_id`
   (+`session_id`), so command/feedback uniqueness holds without a central counter.
3. Counter A should gain a session/epoch concept too (mirroring B) so restarts don't
   reuse `record_seq` ambiguously — ties to the `as_of_seq` snapshot stamp (Q3).

### Q7 addendum — timestamp coverage (confirmed)

**Every `transition_tx` record carries the `Instant`.** `TransitionRecord` is constructed
in exactly one place — `fsm::step` (`step.rs:120`) — and always sets `at: now`
unconditionally. The actor always calls `step(.., Instant::now())` and always wraps
`result.transition_record` into the emitted `RawTransitionRecord`. There is no
constructor path that omits `at`. So the instant is a guaranteed field on every ledger
record (subject to Q8: replace the hardcoded `Instant::now()` with the injectable clock so
that guaranteed instant is also deterministic in tests).

---

## Q8 — Time-based behavior (ACK timeout, 5 s cooldown): pure-step layer vs clock seam. ("Not clear.")

**Clarification: the pure step is already time-as-input (good — keep it). The only gap is
that the *actor* hardcodes `Instant::now()`. Add a small `Clock` seam at the actor so that
single call site becomes injectable. Do both, layered.**

What "time-based behavior" means here:
- **5 s cooldown**: `operational_warning_recovery_ready(began_at, now, ctx)` in
  `transition_map.rs` compares `now - began_at` against `RPM_STRESS_DURATION_THRESHOLD_SECS`.
- **ACK timeout**: the headlamp assembly marks `TimedOut` on `TimerTick` using a deadline
  vs `now`.

Both ultimately depend on the `now` that the **actor** supplies via
`fsm::step(&state, &ctx, &evt, std::time::Instant::now())`. The *pure* layer never reads
a clock — it receives `now`. So:

- **"Keep at the pure-step layer"** = never read the clock inside pure code; the caller
  passes `now`. Already true for `step`. This is the functional core; preserve it.
- **"Add a clock seam"** = introduce `trait Clock { fn now(&self) -> Instant; }`, inject
  it via `VehicleControllerRuntimeOptions` (default = real monotonic clock; tests = a
  manually-advanceable fake). The actor calls `self.clock.now()` instead of
  `Instant::now()` — feeding both `step`'s `now` and the record's `at`.

Recommendation: do both. Keep the pure core as-is; add the seam at the imperative shell.
Payoff: the 5 s cooldown and ACK timeout become testable by *advancing a fake clock +
sending `TimerTick`*, instead of `tokio::time::sleep(FRONT_HEADLAMP_ON_ACK_WAIT + ...)` as
the current timeout e2e does. It also makes `at` deterministic (ties to Q7).

Actorification angle: when actuation/timeout move to child actors driven by timers, a
shared injectable clock keeps timeout logic deterministic across the refactor. Do this
seam *now*; it pays off immediately and survives the split.

---

## Q9 — "Once per FSM event (after state persist, before actions run)": actions must not change current state. Are we ready for message-to-self? Should the journey capture it?

**Verdict: the current ordering is correct and deliberate — the pure `step` is the *only*
place state changes; actions are effects, not mutators. You are actually *more* ready for
message-to-self than the question implies: a self-sent event is just another mailbox
message that produces its own cut. What's missing is *causality*, not the cut itself.**

Actor loop ordering today (`virtual_car_actor.rs::handle`):
1. `step()` (pure) →
2. **persist** `current_state` / `context` →
3. **emit transition record** →
4. **run actions** (`actuation_manager.execute`, async) →
5. emit diagnostics.

Two things follow:
- Because the record is emitted at step 3 reflecting the persisted state, an action at
  step 4 must **not** mutate `current_state` in place — that would desync from the
  already-emitted record. The design *structurally prevents* this: `execute()` takes
  `&DigitalTwinCar` (immutable); only the `EnterMode` hint is handled inline and it merely
  sets a local `mode` (currently `let _ = mode;`). Good — keep this invariant.
- **Message-to-self is compatible with the model.** A self-sent (or feedback) event is a
  *new* mailbox message → a *new* `handle()` call → a *new* `step` → a *new* record with a
  *new* `sequence_no`. The natural FSM progression via actuation feedback
  (`FrontHeadlampOnAck`, etc.) already works this way today. What you are correctly *not*
  doing is **synchronous in-handler re-entrancy** (an action recursively re-running the
  FSM for the same event). That distinction is the real content of "we are not ready for
  message-to-self": you are ready for *asynchronous* self-messages; you are (rightly)
  avoiding *synchronous* re-entrancy.

Does the journey capture it? **The *consequence* yes, the *causality* no.** Each resulting
event already gets its own cut. What's not captured is the link "transition N's action
emitted command C, whose feedback caused transition M." To capture that, thread a
`CorrelationId` from the recorded action (Q4) through the `ActuationCommand` and back via
the feedback event, and record it on both ends. Then the journey becomes a **causal DAG**
rather than a flat list — and that's exactly the property you want once actuation lives in
a separate, concurrently-running child actor (feedback may interleave;
`sequence_no` + `correlation_id` keep it sortable and causally linkable).

Rule to enshrine for actorification: **child actors never mutate parent state; they only
feed results back as new events.** The parent's pure `step` stays the sole state mutator.

---

## Work-item ledger (traceability)

Stable IDs so any future change can be mapped back to the decision that drove it. One
work-item = one focused commit, subject prefixed with the ID (e.g.
`WI-7a: rename TransitionRecord -> RawTransitionRecord ...`). Keep unrelated edits
(`.gitignore`, `README.md`, plan files) out of work-item commits.

| ID | Title | Questions | Status | Depends on |
|----|-------|-----------|--------|------------|
| WI-1 | Record emitted actions in `RawTransitionRecord` | Q4, Q5, Q9 | **DONE** | WI-7a |
| WI-2 | Public pure state-law checker + journey fold | Q6 | **DONE** | WI-1 |
| WI-3 | `Clock` seam in runtime options | Q7, Q8 | **deferred → actorification** (co-design with ticker actor; pure core already time-as-input) | — |
| WI-4 | `as_of_seq` snapshot stamp + Counter-A session/epoch | Q3, Q7 | session/epoch **DONE** (via WI-12 `SessionEpoch`); `as_of_seq` snapshot stamp still pending | WI-7b |
| WI-12 | Serializable published record (`Instant` inside → `Duration`-since-`UNIX_EPOCH` for the world) | Q7 | **DONE** | WI-7b |
| WI-5 | Reclassify `LogWarning` toward diagnostic sink | Q5 | **DONE** | — |
| WI-6 | Test helper `(controller, actuation_rx)` + ack-injection | Q2 | **DONE** | — |
| WI-7a | Type renames `TransitionRecord`→`RawTransitionRecord`, old `RawTransitionRecord`→`PublishedTransitionRecord` | Q4, Q7 | **DONE (uncommitted)** | — |
| WI-7b | Field rename `PublishedTransitionRecord.sequence_no`→`record_seq` (a1: leave `CorrelationId` as-is) | Q4, Q7 | **DONE** | WI-7a |
| WI-8 | Single-writer ledger actor owns `record_seq` | Q7 | actorification | WI-7b |
| WI-9 | Correlation IDs end-to-end (action→command→feedback→record) | Q4, Q9 | actorification | WI-1, WI-7b |
| WI-10 | State-transition diagnostics as a projection of the ledger | Q1 | actorification | — |
| WI-11 | Move buzzer/egress I/O into actuation child actor | Q5 | actorification | WI-1 |

### WI-1 scope (DONE, Option A — owned, filtered clone)

Record the **intended** domain actions emitted by the pure step into the transition
record, so the ledger shows what the FSM decided to do (resolves Q4; makes the no-op
`StartBuzzer`/`StopBuzzer`/`PublishStateSync`/`LogWarning` observable — Q5; feeds Q6/Q9).

Decision trail (why Option A, not a reference or `Arc`):
- A **reference** (`RawTransitionRecord` borrowing `StepResult.actions`) is rejected: it
  would be a self-referential struct (sibling fields of one `StepResult`), and the
  published record is sent through an async mpsc channel so it must **own** its data
  (effectively `'static`) — a per-step borrow cannot cross that boundary. The record's
  effective lifetime is *longer* than the step, not shorter.
- **`Arc<[DomainAction]>`** (shared ownership, refcount-bump instead of deep copy) is
  possible but over-engineering here: each step emits **0–3** actions, so an owned
  `Vec<DomainAction>` clone is a tiny alloc + a few enum copies — negligible.
- **Option A**: the record owns a `Vec<DomainAction>` that is an owned, **filtered**
  clone of `StepResult.actions`.

Content rules:
- `StepResult.actions` stays the **unfiltered execution feed** (the actor needs
  `EnterMode(_)` to set its mode).
- `RawTransitionRecord.actions` is the **ledger projection**: full `DomainAction`
  (lossless; it already derives `Clone/Debug/PartialEq`) **minus `EnterMode(_)`** (a
  runtime control hint, not a domain intent).
- Semantics: these are **intended/emitted** actions from the pure step (deterministic),
  **not** execution outcomes. Ack/timeout/failure remain separate facts (diagnostics +
  future feedback events).

Files:
- `fsm/step.rs` — add `pub actions: Vec<DomainAction>` to `RawTransitionRecord`; build the
  filtered clone at `StepResult` construction. Add a clear comment block stating the
  decision (owned filtered clone; reference/`Arc` rejected; EnterMode excluded; intended
  not outcome) and pointing here.
- tests — extend `test/actor_contract.rs` (and/or a step-level test) to assert the
  recorded actions for a known transition; fix any literal `RawTransitionRecord` /
  `StepResult` constructors or exhaustive destructures broken by the new field.
- `README.md` — short note in the observability/known-behaviors area: the transition
  record carries the intended domain actions (EnterMode excluded; intent, not outcome).

Acceptance: workspace builds; `cargo test -p common` green; new assertion proves actions
are recorded and `EnterMode` is absent.

### WI-2 scope (DONE — final shape)

Expose the state laws as a **pure, public primitive** and keep `verify_all_invariants` a thin
wrapper (resolves Q6). **No journey-fold helper ships in the library** — that consumer-side
concern lives *outside* the twin (external verifier / offline tool / future observer actor),
folding the primitive itself. See ADR-1 (superseded) and ADR-2/-3 for the reasoning.

Decision trail (the "why"):
- **Catalog over ad-hoc calls.** The two laws were `pub(super)` free functions invoked
  inline by `verify_all_invariants`. They are now gathered into a named catalog
  `STATE_LAWS: &[StateLaw { name, check }]` so a failure can be reported as *which* named
  law failed — not just a string. `verify_state_laws(&FsmState, &VehicleContext)` runs the
  whole catalog and **collects all** violations (`Result<(), Vec<LawViolation>>`), rather
  than short-circuiting on the first, so a single cut can surface every breach at once.
- **`verify_all_invariants` is now a thin wrapper:** identity + health (snapshot-only
  concerns) then delegate to `verify_state_laws`. The state-law subset is exactly what an
  external verifier folds over a journey (identity is constant; health rides in ctx).
- **No library journey checker (reversed mid-design).** An earlier iteration shipped a
  first-class `verify_journey_state_laws` (over `(record_seq, &RawTransitionRecord)`, tagging
  `JourneyCut::{Entry,Exit}`, checking `s0` + every exit). It was **dropped**: the verifier is
  a *consumer* of the published stream, expected to be written by a non-core dev / offline CLI
  reading a captured file, using the twin's public types + `verify_state_laws`. Baking the
  fold into the library was over-engineering (ADR-1 superseded). The pure laws are an
  **oracle** (tests / CI / offline / async-sampled), never a PROD hot-path gate.
- **Cut semantics (retained as guidance for the external verifier, ADR-2).** A *cut* is one
  `(state, ctx)` snapshot; each record spans an **entry** `(old_state, old_ctx)` and **exit**
  `(next_state, current_ctx)`. A verifier folds `verify_state_laws` over each cut and should
  **verify** the starting cut `s0` rather than assume it (totality over partial / replayed /
  windowed streams). The laws are **node** invariants, not **edge**/transition-legality.
- **Intent assertions ride along (the WI-1 payoff).** Because records carry
  `transition.actions`, a verifier asserts on emitted intents (e.g. "the redline cut emitted
  `StartBuzzer`") alongside the state-law fold. `ExtremeOperationWarning(Instant)`
  non-determinism does not affect cut-checking (laws read only ctx).

Files (final):
- `digital_twin/car_behaviour_checker.rs` — `StateLaw`, `STATE_LAWS`, `LawViolation`,
  `verify_state_laws`. Laws kept private; reached only through the catalog.
- `digital_twin/mod.rs` — `verify_all_invariants` thin wrapper; re-exports the state-law API.
- `lib.rs` — crate-level re-exports of `verify_state_laws` + catalog types.
- `test/fsm_step_contract.rs` — two tests showing the *external-verifier pattern*: fold
  `verify_state_laws` over a legal journey's cuts (+ assert recorded `StartBuzzer` intent),
  and flag an illegal `Driving`-sub-stall-RPM cut by law name.

Acceptance met: workspace builds; `cargo test -p common` green (74 tests); `verify_state_laws`
+ catalog are the public primitives, no journey helper in the lib.

### WI-5 scope (DONE — least-ripple routing change)

Reclassify `DomainAction::LogWarning` as observability: the actor routes it to the
**diagnostic sink** instead of the actuation manager's no-op (resolves Q5; the runtime
embodiment of ADR-3's "enforce in transition, **announce via diagnostics**").

Decision trail (the "why"):
- **Re-route at the actor, don't restructure the pure layer.** `LogWarning` stays a
  `DomainAction`, still emitted by the pure step (`step` for `RejectedPowerOff`, `output` for
  speed/extreme messages, **and** the front-headlamp assembly for in-flight request warnings —
  more producers than Q5 first noted). It stays in `StepResult.actions` *and* in the
  `RawTransitionRecord.actions` ledger (per Q5: the record entry is expected and useful for
  replay). Only the **routing** changes — the single chokepoint is the actor's action loop.
  This is why WI-5 is the lowest-ripple do-now item.
- **Rejected (bigger ripple):** removing `LogWarning` from `DomainAction` and surfacing
  warnings through a separate `StepResult` field / diagnostic-intent type. That would touch the
  pure step shape, the transition record, and ~20 test assertions across
  `fsm_step_contract`/`lighting_step_contract`/`fsm_engine_contract`/`op_strategy_contract`.
  Deferred indefinitely; not needed to achieve the reclassification.
- **Actuation manager keeps a no-op `LogWarning` arm** purely for `DomainAction` match
  exhaustiveness; it is now unreachable in practice (commented as such).

Files:
- `diagnostic/mod.rs` — new `diag_warning(identity, message)` helper (Warning level).
- `lib.rs` — export `diag_warning`.
- `engine/controller/virtual_car_actor.rs` — new `DomainAction::LogWarning(message)` arm in the
  action loop → `diagnostic_sink.try_emit(diag_warning(..))`; no longer falls through to
  `actuation_manager.execute`.
- `engine/controller/actuation_manager.rs` — `LogWarning` arm re-commented as dead/no-op.
- `test/actor_contract.rs` — `scenario_log_warning_is_routed_to_diagnostic_sink`: redline
  transition surfaces the speed-threshold warning as a Warning-level diagnostic.

Acceptance met: workspace builds; `cargo test --workspace` green (`common` 76); the
speed/extreme/headlamp warnings now appear on the diagnostic stream, not the actuation path.

### WI-6 scope (DONE — actuation round-trip test ergonomics)

Test-only helpers that make the full actuation loop a one-liner:
**send command → observe command → inject ack/nack → observe resulting transition** (resolves Q2).
The harness plays the role of the *future* actuation child actor — it owns the `rx` end of the
injected `actuation_command_tx`, asserts the outbound command, then feeds the matching ack/nack
back through the **real physical-ingress path**.

Decision trail (the "why"):
- **Location: `#[cfg(test)]` in `common/src/test/mod.rs`.** Zero production / public-API
  ripple — the helpers live beside `ActorGuard` and are compiled only under `cfg(test)`.
- **No new types; reuse `ActorGuard`.** `install_with_actuation` returns a
  `(VehicleController, mpsc::Receiver<ActuationCommand>, ActorGuard<…>)` tuple. An earlier
  `ActuationRig` struct was **rejected as noise** — the tuple + existing guard carry everything.
- **Ack path = real physical ingress.** Injection goes through
  `submit_physical_car_event(FrontHeadlampCommandConfirmed/Rejected)`, mapping
  `On → on_command:true`, `Off → false`. This exercises the projection seam and mirrors the
  existing gateway e2e rather than poking the FSM directly.
- **Determinism.** Helpers return the command verbatim; tests assert on the **variant** and
  `sequence_no` monotonicity, never on `session_id` (wall-clock-derived). Ack injection needs no
  `correlation_id`.
- **Tidy-up folded in:** the blanket `#[allow(dead_code)]` + stale "not wired yet" comment on
  `ActorGuard` were removed; the doc comment now explains the RAII contract, and the unread
  `handle` field carries a **scoped** `#[allow(dead_code)]` with an accurate "held for ownership"
  rationale.

Files:
- `test/mod.rs` — `install_with_actuation`, `expect_actuation_command` (panics on
  timeout/close), `inject_matching_ack`, `inject_matching_nack`; `ActorGuard` doc/lint tidy-up.
- `test/actor_contract.rs` — `scenario_actuation_ack_round_trip_via_helper` (observe
  `SwitchFrontHeadlampOn` → inject ack → `headlamp.state == On`) and
  `scenario_actuation_nack_round_trip_via_helper` (… → inject nack → `state == Off`).

Acceptance met: `cargo test -p common --lib` green (78); no new clippy warnings.

### WI-12 scope (DONE — `Instant` inside, `Duration` for the world)

The published transition record is now **serializable, portable, and offline-foldable**, while the
functional core keeps measuring time with `std::time::Instant`. Resolves the blocker that
`std::time::Instant` has no serde impl and no defined zero, so a "dumb writer → file → offline
verifier" pipeline was impossible with the old record shape.

Decision trail (the "why"):
- **Two clocks for two jobs.** Monotonic `Instant` *measures* elapsed time (timeout/cooldown
  decisions — safe against wall-clock jumps); wall-clock `Duration`-since-`UNIX_EPOCH` *places*
  records for serialization/folding. The bridge is a one-shot `(t0_instant, t0_unix)` correlation
  captured per run.
- **Project at the published boundary, never in the core.** The pure `FsmState`,
  `VehicleContext`, and `RawTransitionRecord` stay `Instant`-bearing and serde-free. A new
  `published` module owns the **full lossless mirror** (every type, time fields → `Duration`) plus
  all serde. This reuses the WI-7a raw/published split: `PublishedTransitionRecord` *is* the
  projected form.
- **Three `Instant` sites projected:** `RawTransitionRecord.at`,
  `FsmState::ExtremeOperationWarning(Instant)` (in `old_state`/`next_state`), and
  `HeadlampContext.ack_pending_since` (in `old_ctx`/`current_ctx`). `DomainAction`/`FsmEvent` carry
  no `Instant` but are mirrored anyway (decision: full decoupling of the wire contract; pure core
  stays serde-free). `EnterMode` is dropped from `PublishedDomainAction` — it is a runtime control
  hint, not a domain intent (consistent with WI-1).
- **Foldability triple, now complete:** `record_seq` (clock-independent order) + `at_unix`
  (`Duration` since `UNIX_EPOCH`, for elapsed-between-transitions) + `session_epoch_unix_nanos`
  (which run). An external tool can derive a flat `u128`-nanos timeline from `Duration` itself, so
  `Duration` is serialized with default serde (no premature flattening).
- **Session epoch unified.** `SessionEpoch::capture()` reads the wall clock **once** per run and
  now seeds *both* the published stamps and the actuation `session_id` (previously a separate
  `SystemTime::now()` read in `pre_start`). Lives in `VirtualCarRuntimeState`, never in the twin
  (the twin stays a clean correct-by-construction value).
- **WI-3 (clock seam) explicitly deferred.** The pure core is already time-as-input, so timeout
  logic is already deterministically testable at the pure layer. The clock seam's marginal value is
  actor-level test determinism; it is identical now vs post-actorification and is best co-designed
  with the future ticker/timer child actor. Revisit during actorification.

Files:
- `published.rs` (new) — `SessionEpoch` + the full serializable mirror (`PublishedFsmState`,
  `PublishedVehicleContext` and sub-contexts, `PublishedFsmEvent`, `PublishedDomainAction`, …) and
  `PublishedTransitionRecord::project(raw, identity, seq, epoch)`.
- `transition_sink.rs` — `PublishedTransitionRecord` now re-exported from `published` (was the
  raw-wrapping struct); sink trait/impls unchanged.
- `engine/controller/virtual_car_actor.rs` — `VirtualCarRuntimeState` holds `SessionEpoch`;
  `try_emit_transition_record` projects before emit; `session_id` derives from the same epoch.
- `lib.rs` — declare + export the `published` module.
- `engine/controller/vehicle_controller.rs` — `transition_tx` carries the projected record (via the
  re-export; no change at the use site).
- `test/actor_contract.rs` — assertions move to published types (`PublishedFsmEvent`/`State`) and
  add session-epoch + `at_unix` monotonicity checks.
- `gateway/src/gateway_runtime.rs` — transition print uses the flat published fields.

Acceptance met: `cargo test --workspace` green (78 common + gateway/bus e2e); `published.rs`
clippy-clean. Not in this slice (deliberately): the file writer, the offline reader/verifier, and
the `as_of_seq` snapshot stamp (the rest of WI-4).

### WI-7b scope (DONE, agreed a1)

Renamed **only** the ledger field; `CorrelationId` and all `vehicle_device_bus`
wire `sequence_no` (the command/wire axis) left intact.
- `transition_sink.rs` — `PublishedTransitionRecord.sequence_no` → `record_seq`.
- `virtual_car_actor.rs` — generator `next_sequence_no` → `next_record_seq`; struct init uses `record_seq`.
- `gateway_runtime.rs` — `record.sequence_no` → `record.record_seq` (left `payload.sequence_no`, that's CAN wire).
- `test/actor_contract.rs` — `.sequence_no` → `.record_seq`.
- Acceptance met: workspace builds; all 71 `cargo test -p common` green.

Deferred to WI-4 (per (b)): Counter-A session/epoch.

## Consolidated recommendation, ordered by "do now" vs "do with actorification"

Cheap & high-value **now** (independent of the actor split, and they make the split safer):

1. **Record emitted actions in `TransitionRecord`** (Q4) — unlocks Q5, feeds Q6/Q9.
   Filter/relocate `EnterMode`.
2. **Expose pure, public state-law checker** + journey-fold helper (Q6). Visibility bump +
   one entry point.
3. **Add a `Clock` seam** to `VehicleControllerRuntimeOptions`; replace the lone
   `Instant::now()` in the actor (Q7, Q8). Default real clock; fake for tests.
4. **Stamp snapshots (and keep `sequence_no` on records) with `as_of_seq`** (Q3, Q7) so
   staleness and ordering are explicit and reconcilable.
5. **Reclassify `DomainAction::LogWarning` toward the diagnostic sink** (Q5) — small.
6. Add a **test helper** `(controller, actuation_rx)` + ack-injection (Q2).
7. **Rename for clarity** — must land *before* actions+correlation share one record
   (Q4, Q7).
   - [DONE] Type: `TransitionRecord` (step.rs, the pure step output) →
     **`RawTransitionRecord`**.
   - [DONE] Type: old `RawTransitionRecord` (transition_sink.rs, the identity+seq wrapper
     emitted to the sink) → **`PublishedTransitionRecord`**. The channel/options types
     followed (`transition_tx: Sender<PublishedTransitionRecord>`); the sink trait keeps
     its name `TransitionRecordSink` but now takes `PublishedTransitionRecord`. Workspace
     builds; all 71 `common` tests green.

   Resulting scheme (now in code): `RawTransitionRecord` = raw pure-step fact (`at`,
   event, states, ctx); `PublishedTransitionRecord` = `{ car_identity, sequence_no,
   transition: RawTransitionRecord }` published to the sink.

   **Still to finalize (TODO 7):**
   - [x] Field: `PublishedTransitionRecord.sequence_no` → **`record_seq`** (`ledger_seq`),
     leaving `CorrelationId` as the command axis (`command_seq`). **DONE in WI-7b.**
   - [ ] Give Counter A a **session/epoch** (mirrors `CorrelationId.session_id`) so
     restarts don't reuse `record_seq` ambiguously (ties to `as_of_seq`, Q3).
     **Deferred to WI-4 (pending).**

Better done **with actorification** (shape becomes clear once children exist):

8. **Single-writer ledger actor** owning Counter A (`record_seq`); child actors *message*
   it rather than minting ledger numbers, preserving a true total order as more actuators
   appear (Q7). Correlation counters stay per-source on each child.
9. **Correlation IDs end-to-end**: recorded action → `ActuationCommand` → feedback event →
   resulting record (Q4, Q9). Turns the journey into a causal DAG across actor boundaries.
10. **Make state-transition diagnostics a projection** of the transition ledger via an
    observer/telemetry actor, instead of the parent emitting them directly (Q1). Parent
    keeps emitting only non-transition diagnostics; child actors share the diagnostic bus.
11. **Move connector I/O for buzzer/egress into the actuation child actor** (the existing
    `TODO(actuation-child-actor)` / `TODO(actuation-egress)` arms), now backed by the
    recorded-action ledger so behavior stays observable through the transition.

---

## Knowledge capture — status + rationale digest (source for README & blog)

Compact, single-glance digest pulled from the Q&A and work-item scopes above. This is the
**authoritative status + "why"** to draw from when writing (a) a tight README section and
(b) the technical blog. Keep it current as work-items land.

### Status board

| WI | What | Status | One-line "why it matters" |
|----|------|--------|---------------------------|
| WI-7a | `TransitionRecord`→`RawTransitionRecord`, old `RawTransitionRecord`→`PublishedTransitionRecord` | **DONE** | Names must distinguish the raw pure-step fact from the published-to-sink wrapper *before* actions + correlation share a record. |
| WI-7b | Ledger field `sequence_no`→`record_seq` | **DONE** | Disambiguate the **ledger** counter (Counter A) from the **command** counter (`CorrelationId`, Counter B) — naming is the contract. |
| WI-1 | Record emitted actions in `RawTransitionRecord` | **DONE** | The ledger now shows *what the FSM decided to do* (intended intents), making no-op actions observable and feeding journey/causality work. |
| WI-2 | Public pure state-law primitive (`verify_state_laws` + catalog) | **DONE** | Invariants are a pure, named-law *oracle*; an external/offline verifier folds it over a captured stream. No journey helper in the lib (dropped as over-engineering). |
| WI-3 | `Clock` seam in runtime options | **deferred → actorification** | Pure core is already time-as-input (deterministic at the pure layer). Seam's marginal value is actor-level test determinism; identical now vs post-split, so co-design it with the future ticker actor. |
| WI-4 | `as_of_seq` snapshot stamp + Counter-A session/epoch | session/epoch **DONE** (WI-12); `as_of_seq` pending | `SessionEpoch` now anchors every run (one wall-clock read, shared by published stamps + actuation `session_id`); `as_of_seq` snapshot legibility still to come. |
| WI-12 | Serializable published record (`Instant`→`Duration`-since-`UNIX_EPOCH`) | **DONE** | Monotonic `Instant` *measures* inside; wall-clock `Duration` *places* for the world. New `published` module owns the full lossless mirror + serde; core stays `Instant`-bearing & serde-free. Unblocks file-dump + offline folding. |
| WI-5 | Reclassify `LogWarning` toward the diagnostic sink | **DONE** | `LogWarning` is observability, not actuation — the actor now routes it to the diagnostic bus instead of the actuation no-op. |
| WI-6 | Test helper `(controller, actuation_rx)` + ack-injection | **DONE** | `#[cfg(test)]` helpers in `test/mod.rs` reusing `ActorGuard`; ack/nack injected via the real `submit_physical_car_event` ingress path. |
| WI-8..11 | Ledger actor / correlation DAG / diagnostics-as-projection / actuation child I/O | actorification | Shapes that only crystallize once the monolithic actor splits into parent + child actors. |

### Decisions worth keeping (the "why", distilled)

1. **Two channels, not peers.** `transition_tx` = authoritative *fact ledger* (lossless,
   totally ordered by `record_seq`, one record per event). `diagnostic_tx` = best-effort
   *multi-source telemetry bus* (unbounded, unordered, many producers). You cannot
   reconstruct one from the other; actorification *strengthens* the two-channel split.
2. **`GetStatus` is as-of, never wrong.** A snapshot reflects events ≤ its sequence; the
   fix is to make staleness *legible* (`as_of_seq`, WI-4), not to add a mutating refresh.
   Prefer the ledger over polling for verification.
3. **Record intended actions, not outcomes.** The pure step is deterministic, so the record
   captures *emitted intents*; ACK/timeout/nack/failure are *separate* facts (diagnostics +
   feedback events that generate their own records). `EnterMode` is excluded from the record
   (runtime control hint, not a domain intent).
4. **Two `sequence_no` counters exist and must never be confused.** Counter A (ledger,
   `record_seq`, per `car_identity`, every event) vs Counter B (`CorrelationId`, per
   `(source_id, session_id)`, only on a correlated command, leaves the process narrowed to
   `u32` on CAN). No aliasing today; naming keeps them apart.
5. **Pure core, injected edges.** `step(state, ctx, event, now)` never reads a clock — time
   is an input. The only non-determinism is the actor's `Instant::now()`; a `Clock` seam
   (WI-3) makes it deterministic without touching the functional core.
6. **A journey is a trajectory of cuts; verification lives outside the twin.** Laws hold at
   every `(state, ctx)` cut. The library ships only the pure primitive `verify_state_laws`
   (named-law catalog, collects all violations); an *external/offline verifier* folds it over
   a captured `PublishedTransitionRecord` stream, verifying `s0` rather than assuming it.
   Laws are **node** invariants, not edge/transition-legality. Invariants are *enforced* in
   the FSM transition and *announced* via diagnostics — the oracle never gates the hot path.
7. **Child actors never mutate parent state.** The parent's pure `step` stays the sole state
   mutator; children only feed results back as new events (async self-messages are fine;
   synchronous in-handler re-entrancy is not). This rule survives actorification.

### Open TODOs (carried forward)

- **WI-3:** add `trait Clock { fn now(&self) -> Instant; }`, inject via
  `VehicleControllerRuntimeOptions` (default = real monotonic; tests = advanceable fake);
  replace the actor's `Instant::now()`. Unblocks deterministic timeout/cooldown tests.
- **WI-4:** add `as_of_seq: u64` to the snapshot reply; give Counter A a session/epoch so
  restarts don't reuse `record_seq`.
- **WI-5:** **DONE** — the actor routes `DomainAction::LogWarning` to the diagnostic sink
  (see WI-5 scope).
- **WI-6:** **DONE** — `#[cfg(test)]` helpers return `(controller, actuation_rx, guard)` plus
  one-liner ack/nack injectors that round-trip through the real physical-ingress path
  (see WI-6 scope).
- **Actorification (WI-8..11):** single-writer ledger actor owns `record_seq`; correlation
  IDs threaded action→command→feedback→record (causal DAG); state-transition diagnostics
  become a projection of the ledger; buzzer/egress I/O moves into the actuation child actor.

---

## ADR log (in-flight technical decisions)

Conscious choices captured as they are debated, so the README/blog can cite the *why*, not
just the *what*. Status: `ACCEPTED` (decided) / `OPEN` (recommendation pending confirmation).

### ADR-1 — Journey checker input type & placement (Q1/Q6) — `SUPERSEDED`

**Superseded by ADR-3 (verifier lives outside the lib).** The whole "where does the journey
checker live / what record type does it take" question dissolved once we decided the verifier
is a *consumer* of the published stream, written outside the twin. No `ledger_audit` module;
no library journey-fold; `verify_journey_state_laws`/`JourneyCut`/`JourneyViolation` were
dropped. The analysis below is retained for the blog ("a path we explored and rejected").

<details, retained for history>


**Context.** `verify_journey_state_laws` currently takes `(record_seq, &RawTransitionRecord)`
pairs and lives in `digital_twin` next to the pure law catalog. The runner of a journey
check, however, is whoever sits at the **receiving end of `transition_tx`**: today a test
harness draining `rx`, later a dedicated observer/telemetry/journal actor (WI-10). Both hold
`PublishedTransitionRecord`, which **already carries `record_seq`**.

**Forces.**
- `record_seq` is *intrinsically a published-ledger property* — a bare `RawTransitionRecord`
  (pure-step fact) has no sequence number; it is assigned at publication. So "verify a
  journey *and report ledger positions*" conceptually operates on `PublishedTransitionRecord`.
- The tuple signature forces every caller to write `.map(|p| (p.record_seq, &p.transition))`
  and to *re-supply* a seq the published record already owns.
- Layering: `digital_twin` is the pure safety-law layer (`verify_state_laws` over
  `FsmState`+`VehicleContext`). Making it import `transition_sink` (the observability
  transport type) is a layering smell — though **not** a cycle (`transition_sink → fsm`
  only; `transition_sink` does not import `digital_twin`).

**Recommendation (pending placement nod).**
- Keep `verify_state_laws(&FsmState, &VehicleContext)` — the pure catalog — in `digital_twin`.
- Move `verify_journey_state_laws` to take `&PublishedTransitionRecord` and live at the
  **ledger-consumer layer** (inside `transition_sink`, or a small new `ledger_audit`/`journey`
  module that may depend on both). It calls *into* `digital_twin::verify_state_laws`.
- Resulting dependency arrow: **ledger-consumer → law-catalog** (correct direction); drops the
  tuple gymnastics; uses the existing `record_seq`. Function stays first-class/PROD-usable.
- Rejected alternative: keep the `Raw`+tuple API for "full decoupling." Its only benefit is
  letting pure step-level tests build records without publishing — but those tests already
  build `PublishedTransitionRecord` and strip it to a tuple, so the benefit is illusory.
- **Open:** placement — `transition_sink` vs a new `ledger_audit` module.

**Evidence from the tree (for the placement sub-decision).**
- The live consumer already exists and holds `PublishedTransitionRecord`: the gateway's
  `transition_tx` end is a `tokio::spawn` drain loop (`while let Some(record) = rx.recv()`)
  that reads `record.record_seq` directly. This is the prototype of the WI-10
  observer/telemetry actor and confirms the `Published` signature is the ergonomic one.
- Precedent: the `diagnostic` module bundles *streamed type + sink + consumer*
  (`DiagnosticMessage` + sink + `spawn_stdout_diagnostic_observer`). So "consumer-side helper
  next to the streamed type" is an accepted pattern — *but* `diagnostic`'s observer needs
  nothing external, whereas the journey auditor needs `digital_twin::verify_state_laws`.

**Cycle check (both legal).** `digital_twin` deliberately does **not** import `transition_sink`
(no back-edge), so:
- Option A (auditor in `transition_sink`) ⇒ `transition_sink → digital_twin → fsm` + the
  existing `transition_sink → fsm`. Acyclic, but the pure transport/sink module would start
  depending on the safety-law model (against its "ship it, don't inspect it" identity).
- Option B (new `ledger_audit`) ⇒ `ledger_audit → {transition_sink, digital_twin} → fsm`.
  Acyclic; both leaves stay lean; the bridge concern (audit a shipped ledger against the laws)
  gets its own home — exactly where the WI-10 observer/auditor and a future **edge-law**
  checker (see ADR-2) will accrete.

**Sharpened recommendation: Option B (new `ledger_audit` module), `Published` signature.**
The auditor is a cross-cutting *consumer* concern bridging two leaf modules and is expected to
grow (observer actor, alert-on-breach, edge-laws). The `diagnostic` precedent favors
streaming-helpers-next-to-type, but auditing-against-laws is a *bridge*, not streaming, so it
should not burden the transport module. Counter-argument (acknowledged): today it is a single
function, so a dedicated module is slightly ahead of need — acceptable only because WI-10 /
edge-laws are already anticipated. Name: `ledger_audit` preferred over `journey` (which
collides with the fold vocabulary).
</details>

### ADR-2 — The external verifier audits *nodes* (cuts), and *verifies* `s0` rather than *assumes* it (Q2/Q6) — `ACCEPTED` (re-scoped to the external/offline verifier)

**Scope note:** this is guidance for the **external/offline verifier** (the library no longer
ships a journey-fold; see ADR-1 superseded). It applies to whatever folds `verify_state_laws`
over a captured stream.

**Distinction.** The inductive *correctness proof* of an FSM is "base case `s0` legal + every
transition preserves legality ⇒ all states legal." The verifier is **not** proving the
machine correct — it is **auditing observed data**. A pure function handed a `Vec` of records
cannot know the provenance (cold boot vs replay vs windowed slice vs fuzz vs buggy build), so
it **verifies** the base case rather than **assuming** it. `s0` is present in
`records[0].old_state/old_ctx`; checking it directly is one extra evaluation and is strictly
stronger than trusting it. An invariant guardian that trusts its input is not guarding.

**Node-laws, not edge-laws.** The current laws are predicates over a *single* `(state, ctx)`
cut ("is this state internally consistent", e.g. not Off-while-moving). They say nothing about
whether `s0 --event--> s1` was a *permitted transition*. Edge/transition-legality would be a
separate checker over `(old_state, event, next_state)` — noted as a **future law category**,
not done. Hence there is no "reach `s0` first, then validate the edge" ordering here; every
cut is validated independently as a node.

**Cost/benefit of checking `s0`.** For a genuine complete-from-boot journey, `s0 = (Off,
default ctx)` is legal by construction, so the entry check never fires → cheap redundancy,
**zero false positives**. It earns its keep only when `s0` is *not* the cold-boot state:
- a **windowed/partial ledger** — a live observer may never see record 1 (bounded channel,
  late subscription), so its first-seen `s0` is a mid-trajectory state (direct tie to ADR-1);
- a **replayed/persisted/networked** log, or a hand-built/fuzzed record;
- a real **`step` bug** emitting an illegal state.

**Guidance for the verifier.** Fold over the **entry cut of the first record (`s0`) + the exit
cut of every record** (`s1..sn`), each distinct trajectory state once; report failures with the
`record_seq` and which endpoint. Entry cuts of non-first records need not be re-checked (they
equal the prior exit on a contiguous stream). A synthetic `(Off, speed=42)` starting record —
not something the live FSM emits — demonstrates this is **total/self-contained** over arbitrary
record sets, *not* an FSM bug.

**Alternative (not recommended):** "audit forward progress only, trust the start" (check exits
only). It silently breaks for windowed/replayed streams (a live observer may never see record
1), so a verifier should prefer checking `s0`.

### ADR-3 — Invariants are *enforced* in the transition, *announced* via diagnostics, *detected* offline — never an inline PROD gate (Q3) — `ACCEPTED`

**Decision.** Three distinct roles, never collapsed into "call a big checker on every state
change":
1. **Enforce (primary guardian)** in the FSM transition — clamp or reject so the twin stays in
   a steady, predictable state even when an input would violate a property. The transition is
   *strict*; illegal states are minimized by construction. Already exemplified by
   `freeze_standstill()` (speed clamped to 0 while `Off`).
2. **Announce** a would-be / clamped violation as a **diagnostic** (not actuation). This is
   exactly **WI-5** (reclassify `LogWarning` toward the diagnostic sink): Point 3 and WI-5 are
   the same insight. The pure `step` cannot emit diagnostics directly, so it emits a
   diagnostic-*intent* the actor routes to `diagnostic_tx`.
3. **Detect (oracle)** post-hoc with the pure `verify_state_laws`, folded by the external
   verifier over the recorded stream — tests / CI / offline / async-sampled. **Never** on the
   PROD hot path, **never** a synchronous gate.

**Why not an inline checker (the cost reasoning).**
- *PROD compute:* the oracle is off the hot path → zero PROD cost regardless of how large the
  law catalog grows.
- *Testing:* the oracle is pure and actor-free → replay a captured/synthetic
  `Vec<PublishedTransitionRecord>` and fold; cheap, parallel, decoupled from runtime timing.
  An inline gate would inflate every integration test and couple correctness to scheduling.
- *Actorification:* "are all parameters correct *right now*" stops being answerable by any one
  actor (state is spread across actors + in-flight messages). A synchronous global check would
  need a **distributed barrier** (freeze all actors, snapshot) — expensive and it kills the
  concurrency actorification buys. So: **local invariants** stay enforced per-actor in
  transitions; **global/cross-actor invariants** are reconstructed *offline* from the merged
  ledger via `record_seq` + `correlation_id`. This is why `as_of_seq` (Q3/WI-4) exists.

**Prototype scope (explicit, agreed).**
- The set of *safety clamps* (which dangerous states to forbid, how to clamp) depends on deep
  Automotive/Physics domain knowledge. This prototype does **not** aim for PROD-grade clamp
  coverage; it aims to **demonstrate that the FSM never lets the vehicle reach a dangerous
  state**, with violations surfaced via diagnostics.
- For capturing the transition stream cheaply, prefer a **memory-mapped file** sink (no
  network cost/latency) that an offline verifier reads — over a networked collector. (Candidate
  for the "dumb writer on `rx`" consumer; not yet built.)

**Open for actorification (carry forward).** *Where and how the global view of the
`DigitalTwinCar` is held* once state is split across a parent FSM actor + child actors is an
unresolved design question. Options to weigh later: a single-writer ledger/journal actor as the
authoritative merge point (ties to WI-8), a periodically-materialized parent snapshot stamped
with `as_of_seq`, or purely offline reconstruction from the ledger. Decide with the actor split,
not before.

### ADR-4 — `DigitalTwinCar` is correct-by-construction; FSM step is the sole mutator (enforce, don't validate) — `ACCEPTED`

**Context.** `verify_all_invariants` opened with a runtime check `if self.identity.is_empty()`.
That guarded a hole the *type* allowed: `identity: String` (public) could be empty, and any code
could set `current_state`/`context` to arbitrary values. The goal: make "a `DigitalTwinCar`
without valid constituents" *unrepresentable* rather than runtime-checked (ADR-3 #1 in the
small: enforce by construction).

**Decisions (chosen interactively).**
- **Mechanism:** private fields + a checked constructor (not a `CarIdentity` newtype). Keeps
  `identity: String` but routes all construction through `DigitalTwinCar::new`.
- **Constructor:** `new(identity, current_state, context) -> Result<Self, DigitalTwinCarError>`
  validates the only structurally-invalid constituent — a **blank identity** (empty or
  **whitespace-only**, rejected; stored **trimmed**). `current_state`/`context` are
  caller-supplied (a freshly-born twin passes `Off` + `VehicleContext::default()`).
- **Boundary:** validation happens where the twin is born — the actor's `pre_start` — and a
  blank identity **fails actor startup** (`DigitalTwinCarError` → `ActorProcessingErr` via `?`).
  Public `install_and_start*` signatures are unchanged.
- **Mutation:** fields are private, so the actor's old `twin.current_state = …; twin.context =
  …` becomes a single `apply_step(next_state, context)` method. This **structurally enforces
  Q9**: external code cannot mutate the twin except by recording a pure-step result, so the FSM
  `step` is provably the sole state mutator. Reads go through `identity()` / `current_state()` /
  `context()` accessors.
- **`verify_all_invariants`:** the identity check is **deleted** (now dead — non-blank is
  guaranteed by the type). `is_healthy()` stays a runtime check because health is *time-varying*
  (a sensor property), not a construction invariant.

**Scope (explicitly bounded).**
- Only the identity hole is closed structurally. **`VehicleContext` internal coherence**
  (e.g. speed derived from rpm) is **deferred** — a separate newtype effort, out of scope here.
- **State restore** (rehydrating a twin at a non-`Off` state after replay/restart) is **not**
  added now; a validated `from_snapshot`/`rehydrate` constructor comes later (ties to WI-4).
- *Safety clamps* (which dangerous states to forbid + how to clamp) need deep Automotive/Physics
  domain knowledge; this prototype only **demonstrates** the FSM never reaches a dangerous
  state, not PROD-grade clamp coverage (carried from ADR-3 prototype scope).

**Result.** "Twin with a blank identity" and "twin mutated outside the FSM step" are now
unrepresentable. `cargo test --workspace` green; the change rippled only into accessor/`apply_step`
call sites (actor + tests), no public controller-signature change.
