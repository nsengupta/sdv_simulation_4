# Iteration 1 → Iteration 2: architecture & design diff notes (blog input)

> Working notes for the next blog post in the *Prototyping a Software Defined Vehicle*
> series. Source material: both repos' `README.md`, `blog-inputs/episode-0{1,2,3}-*.md`,
> `docs/design-notes-runtime-observation.md` (iteration-2 only), the live source trees,
> and the published Stage I / Stage II posts.
>
> Audience reminder for the post: the whole series is about *moving from a minimal
> starting point toward cleaner, more realistic prototypes over 3–4 iterations* — showing
> the **thinking**: which limitations I chose to live with, which I solved, and how I
> re-arranged the architecture to solve them.

---

## 0. The one-line story of this iteration

**Iteration 1 made the control loop *work*. Iteration 2 made it *decomposable, observable,
and provable* — without changing what the user sees.**

The processes, the CAN wire protocol, the FSM behaviour, and even the three episode
write-ups are essentially the same. What changed is **the shape of the code and the
contracts inside it**, deliberately laid down as *groundwork for the next iteration*
(splitting the monolithic twin into per-zone child actors — "actorification").

This is an important narrative beat: **a refactor-heavy milestone where almost nothing
changes on screen.** That is worth calling out honestly in the blog — it is the iteration
where I pay down conceptual debt before I can grow.

---

## 1. What is the SAME across both iterations (set the baseline, then move past it)

So readers don't go hunting for differences that aren't there:

- **Topology / runtime shape.** Three processes on one Linux SocketCAN `vcan0`:
  `emulator` (RPM + ambient lux ~10 Hz), `gateway` (CAN ingress, projection, the digital
  twin, CMD egress), `front_headlamp_actuator` (body ECU, ACK/NACK ~150 ms).
- **Wire protocol.** Telemetry IDs `0x101` speed (decoded, not consumed), `0x102` RPM,
  `0x103` lux; headlamp `0x204` (CMD / ACK / NACK).
- **Digital twin = `ractor` actor + pure FSM.** `VirtualCarActor` mailbox, single-threaded
  handling, calling a pure `step(state, ctx, event, now) -> StepResult` boundary.
- **Four primary FSM states** (`Off`, `Idle`, `Driving`, `ExtremeOperationWarning`) plus an
  orthogonal `LightingState` sub-state; lux hysteresis (`LUX_ON` 840 / `LUX_OFF` 860);
  speed derived from RPM (`rpm × 0.114`); `ExtremeOperationWarning` on speed>160 / RPM>5500
  with a 5 s cooldown.
- **Key principle from day one:** *domain logic emits intent (`DomainAction`), never performs
  I/O.* The gateway/controller projects raw CAN into twin vocabulary; the actor never parses
  a CAN ID.
- **Intentional shortcomings** (local `VssSignal` enum instead of COVESA VSS; single
  RPM→speed multiplier; stdout dashboard; `vcan0` hardcoded; one actuator device) are
  carried forward unchanged.

> Blog framing: "If you ran both prototypes side by side, you couldn't tell them apart.
> The interesting work in this iteration is *underneath* — and it is all in service of the
> iteration after this one."

---

## 2. THE headline change — flat context → per-assembly ("zone") decomposition

This is the single biggest structural difference and the spine of the post.

### Iteration 1 (flat)
`VehicleContext` is one flat struct in `fsm/machineries.rs`:

```rust
pub struct VehicleContext {
    pub rpm: u16,
    pub speed: u16,
    pub fuel_level: u8,
    pub oil_pressure: u8,
    pub tyre_pressure_ok: bool,
    pub ambient_lux: u16,
    pub lighting_state: LightingState,
    pub lighting_ack_pending_since: Option<Instant>,
}
```

All the *behaviour* over that data (lux→request rules, ACK-timeout recovery, speed
derivation, ignition-off reset) lives inline inside a **238-line `step.rs` monolith** as
free functions (`try_front_headlamp_ack_timeout`, `try_recover_front_headlamp_incomplete`,
direct `modified_ctx.lighting_state = …` mutations, etc.).

### Iteration 2 (assemblies)
`VehicleContext` becomes an **aggregate of per-assembly contexts**, each owning *its own
data AND the behaviour over that data* (`crates/common/src/fsm/assembly/`):

```rust
pub struct VehicleContext {
    pub powertrain: PowertrainContext,   // wheel RPM + derived speed
    pub health:     VehicleHealthContext,// fuel / oil / tyre
    pub visibility: VisibilityContext,   // ambient lux
    pub headlamp:   HeadlampContext,     // lighting state + ACK-wait bookkeeping
}
```

`step.rs` shrinks from 238 → ~158 lines and becomes a **pure orchestrator**: it decides
*when* to invoke each assembly, but the *how* lives on the assembly (`apply_rpm`,
`refresh_speed`, `evaluate_lux`, `on_timer_tick`, `on_incomplete`, `reset_for_ignition_off`).

```rust
// step.rs now reads like a score, not an implementation:
FsmEvent::UpdateRpm(rpm)        => modified_ctx.powertrain.apply_rpm(*rpm),
FsmEvent::UpdateAmbientLux(lux) => modified_ctx.visibility.apply_lux(*lux),
FsmEvent::FrontHeadlampOnAck    => modified_ctx.headlamp.apply_on_ack(),
// …
modified_ctx.powertrain.refresh_speed();
modified_ctx.headlamp.evaluate_lux(prev, *lux, now, &mut actions);
```

### Why (the limitation it attacks)
The flat struct + monolithic `step` is fine for a demo but does **not** map onto how a real
SDV is built: distinct subsystems (powertrain, body/lighting, health) are *separate ECUs /
zones* that own their own state and run concurrently. The flat model has no seam to split
along.

### What it sets up (the explicit payoff, in the code comments)
Every assembly file says it out loud: *"In Step 2 this becomes `PowertrainActor`'s local
state + flat FSM"*, *"each field below migrates into its own child actor."* This is
**Step 1 groundwork for the zone-actor plan (referred to as ADR 0001 in code)**. The
aggregate keeps public fields and a thin `is_healthy()` delegate **purely so existing call
sites keep compiling during the transition** — an honest, incremental-refactor detail worth
showing.

> Blog framing: "I reorganised the twin's state to mirror the physical car's zones —
> *before* I make them concurrent. The decomposition is the boring half of the actor split;
> doing it first, behind a stable contract, means the exciting half won't be a rewrite."

---

## 3. Toward realism: single RPM → per-wheel `WheelRpm`

A small but telling change inside the powertrain assembly:

```rust
pub struct WheelRpm { pub front_left: u16, pub front_right: u16,
                      pub rear_left: u16, pub rear_right: u16 }

// today, one bus reading is broadcast to all four wheels:
pub fn apply_rpm(&mut self, rpm: u16) { self.wheel_rpm = WheelRpm::uniform(rpm); }
pub fn primary_rpm(&self) -> u16 { self.wheel_rpm.front_left } // representative for control
```

- Iteration 1 had a single `rpm: u16`.
- Iteration 2 models **four wheels**, but consciously **lives with the single-input
  assumption** (`uniform(rpm)`), using `front_left` as the representative for control
  decisions.
- A new `PowertrainMode { Stalled, Rolling, Redline }` is derived **but not yet consumed by
  the operational FSM** — "exposed for observability/tests only (no behaviour change); Step 2
  this becomes the actor's stored flat-FSM state."

> Blog framing: this is a clean micro-example of the series' thesis — *introduce the realistic
> shape (4 wheels, a local mode) now, keep the simple behaviour (one value, broadcast) for
> now, and leave a labelled seam for later.* Recall Stage II already flagged "a real 4-W car
> has 4 RPM sensors; we use one" — iteration 2 starts honouring that in the type system
> without paying the full kinematics cost yet.

---

## 4. Observability becomes a first-class, portable contract

Iteration 1 had a `TransitionRecord` and a `transition_tx` channel, but it was an
in-process, `Instant`-bearing, non-serializable audit object. Iteration 2 turns the
observation story into a designed subsystem. Almost all of this is documented in
`docs/design-notes-runtime-observation.md` (the iteration-2-only design log — a goldmine of
ADRs and Q&A, with stable `WI-*` work-item IDs).

### 4a. Records now carry *intended actions* (WI-1)
`RawTransitionRecord` gains `actions: Vec<DomainAction>` — an owned, **filtered** clone of
what the pure step emitted (minus `EnterMode`, a runtime hint). So the ledger now shows
*what the FSM decided to do*, not just the state delta. Crucially these are **intended
intents, not outcomes** — ACK/timeout/failure remain *separate* facts. (Design note records
*why* a clone, not a borrow or `Arc`: 0–3 actions per step, and the record must outlive the
step to cross an async channel.)

### 4b. Serializable "published" mirror (WI-12) — two clocks for two jobs
New `published.rs` module holds a **full lossless serde mirror** of the record where every
`Instant` is projected to a wall-clock `Duration`-since-`UNIX_EPOCH`:

- monotonic `Instant` *measures* elapsed time inside the core (timeout/cooldown — safe
  against wall-clock jumps);
- `Duration` *places* records for serialization/offline folding.

The pure core (`FsmState`, `VehicleContext`, `RawTransitionRecord`) stays `Instant`-bearing
and **serde-free**; projection happens only at the published boundary. This unlocks a
"dumb writer → file → offline verifier" pipeline that was *impossible* in iteration 1.

### 4c. Naming discipline: two `sequence_no` counters, never confused (WI-7a/b)
The design log found two physically-independent counters that happened to share the name
`sequence_no`:

| | Counter A — ledger | Counter B — correlation |
|---|---|---|
| Field | `RawTransitionRecord` → `record_seq` | `CorrelationId` (command axis) |
| Cadence | every FSM event | only on a correlated actuation command |
| Scope | per `car_identity` | per `(source_id, session_id)`; goes on the wire (`u32`) |

Iteration 2 **renamed** the types/fields (`TransitionRecord`→`RawTransitionRecord`, old
`RawTransitionRecord`→`PublishedTransitionRecord`, ledger `sequence_no`→`record_seq`)
*before* the two ever coexist in one struct. "Naming is the contract."

### 4d. Snapshot staleness made legible (WI-4)
`GetStatus` now returns `CarSnapshot { car, as_of_seq }` instead of a bare twin. The
snapshot is "never wrong, only *as-of* sequence N"; the stamp lets a consumer reconcile it
against the transition stream. Counter A advances **once per applied FSM event regardless of
whether a sink is wired**, so `as_of_seq` is always meaningful (0 = nothing applied yet).

> Blog framing: "In iteration 1 I could *watch* the twin. In iteration 2 I can *replay and
> audit* it offline, deterministically, with every record telling me what the FSM intended,
> in a stable order, with explicit staleness." Great place to talk about the
> *fact-ledger vs operational-log* distinction (§5).

---

## 5. Two channels with sharpened, separate jobs

Both iterations have `transition_tx` and `diagnostic_tx`. Iteration 2 *articulates and
enforces* the difference (design note Q1, decision #1):

- `transition_tx` = **authoritative fact ledger** — bounded, lossless-or-error, totally
  ordered by `record_seq`, **one record per event**, machine audience (replay, invariants).
- `diagnostic_tx` = **best-effort, multi-source telemetry/presentation bus** — unbounded,
  unordered, many producers, human audience.

You cannot reconstruct one from the other. The intended cleanup (deferred to actorification):
stop the parent emitting the *state-transition* diagnostic directly; let a future observer
actor **project** it from the ledger.

---

## 6. Correctness model: enforce / announce / detect (ADR-3, ADR-4, WI-2)

Iteration 1 had two `pub(super)` law functions invoked inline by `verify_all_invariants`.
Iteration 2 promotes invariants into a designed, three-role model:

1. **Enforce** in the FSM transition (clamp/reject) — e.g. `freeze_standstill()` zeroes speed
   while `Off`. Illegal states minimized by construction.
2. **Announce** would-be/clamped violations via the **diagnostic** sink (this is why
   `LogWarning` was reclassified — §7).
3. **Detect** post-hoc with a **pure, public** `verify_state_laws(&FsmState, &VehicleContext)`
   over a named **`STATE_LAWS` catalog** (collects *all* violations, tags *which law* failed).
   It's an **oracle** for tests/CI/offline — **never** a PROD hot-path gate.

Plus **correct-by-construction twin (ADR-4):** `DigitalTwinCar` got private fields + a checked
constructor (`new(...) -> Result<…>` rejecting blank identity) + a single `apply_step(...)`
mutator. Result: *"a twin with a blank identity"* and *"a twin mutated outside the FSM step"*
are now **unrepresentable**. This also structurally enforces "the pure `step` is the sole
state mutator" — a rule that must survive the actor split.

> Blog framing: a nice "why I don't just call a big `check_everything()` on every state
> change" digression — an inline global checker becomes a *distributed barrier* once state is
> split across actors, killing the concurrency you refactored to get. So: local invariants
> enforced per-actor in transitions; global invariants reconstructed offline from the merged
> ledger. (Why `as_of_seq` exists.)

---

## 7. Smaller-but-real runtime hardening (lessons from a live run)

These came out of an actual live gateway+actuator run (the design note's "Surfaced this
iteration → live-run observation" section) and are good, concrete blog colour:

- **Silent-success defect fixed.** A clean headlamp ACK used to emit *no* `DomainAction`, so
  success was invisible on the always-on diagnostic stream (only NACK/timeout showed). The
  actor now classifies the `*Requested → On/Off` settle and emits a confirmation diagnostic —
  pure FSM untouched.
- **Hot-path logging fix.** Both the actuator loop and the gateway CAN-ingress loop used to
  log **synchronously on the protocol-critical path** — so console back-pressure (literally a
  `Ctrl-S`/XOFF tty freeze triggered it) became a *protocol stall*. Iteration 2 hands logging
  to a **bounded side-channel** drained by a separate thread/task, with non-blocking
  `try_send` and **drop-on-full**; the ACK reaches the twin *before* logging. (`sim_2`'s
  `gateway_runtime.rs` has `ingress_log_tx.try_send(line)` "off the hot path"; `sim_1` logs
  inline.)
- **`LogWarning` reclassified (WI-5).** It's *observability, not actuation* — the actor now
  routes it to the diagnostic sink instead of the actuation manager's no-op.
- **Diagnostics enriched at the producer, payload stays domain-free (decision #8).** The
  human line ("Transitioned to Driving, speed = 72 km/h … within safe limit") is built by
  reusing the **same FSM predicates/thresholds** the FSM uses to decide warnings, so the
  wording can't drift from the rule — while what crosses the channel is still a plain
  `DiagnosticMessage` (zero domain types). "Good coupling."
- **Test ergonomics (WI-6).** A `#[cfg(test)]` helper returns `(controller, actuation_rx,
  guard)` and one-liner ack/nack injectors that round-trip through the *real* physical-ingress
  path — the harness stands in for the future actuation child actor.
- **`EMULATOR_TUNNEL_PROB` env knob** added so demo runs can tune how often low-lux tunnels
  drive the headlamp ON.

---

## 8. Limitations consciously KEPT in iteration 2 (the "living with it" list)

Worth stating plainly — the series is as much about *what I deliberately didn't fix yet*:

- Still a **local `VssSignal` enum**, not COVESA VSS / databroker.
- Still **one RPM → speed multiplier**; `WheelRpm` is 4 fields but `uniform()`; no slip/gear.
- `VehicleSpeed` (`0x101`) still **decoded but rejected** at projection (no observed-speed ECU).
- Still **stdout-only** dashboard; `PublishStateSync` is a log stub; no Zenoh/MQTT/HMI.
- Still **`vcan0` hardcoded**, one reader thread, one actuator device, no security/routing.
- `PowertrainMode` derived but **not consumed** by the FSM yet.
- `VehicleContext` assembly fields are still **public** (compile-compat shim, not yet encapsulated).
- The **offline file-writer + verifier** are designed/unblocked but **not built** in this repo.

---

## 9. Explicitly DEFERRED to iteration 3 ("the actorification clone")

The design log has a hard rule: *anything tagged "actorification" moves to a fresh cloned
project — do NOT do it in this repo.* That is literally the boundary between this iteration
and the next. Carry-forward items (great "coming next" section for the post):

- **WI-8** — single-writer **ledger actor** owns `record_seq` (true total order as more
  actuators/children appear); correlation counters stay per-source.
- **WI-9** — **correlation IDs end-to-end** (recorded action → command → feedback event →
  resulting record), turning the journey into a **causal DAG** across actor boundaries.
- **WI-10** — state-transition diagnostics become a **projection** of the ledger via an
  observer/telemetry actor (parent stops emitting them directly).
- **WI-11** — move buzzer/egress I/O into an **actuation child actor**.
- **WI-13** — **actuation resilience** (bounded retry/backoff, dedup of identical pending
  requests, a `LightingState::Unknown`/degraded terminal state, rate-limited "peer
  persistently unavailable" diagnostic). Today a down actuator → re-request every tick +
  per-tick timeout spam, no recovery. **Blog:** do not surface — keep in design notes / roadmap only.
- **WI-14** — unified diagnostic fan-in across child actors (shared `Arc<dyn DiagnosticSink>`
  vs a dedicated diagnostics-aggregator actor) — decide *before* wiring the first child's
  diagnostics.
- **WI-3** — `Clock` seam in runtime options (deferred; pure core is already time-as-input, so
  best co-designed with the future ticker/timer child actor).
- The rule to enshrine: **child actors never mutate parent state; they only feed results back
  as new events.** The parent's pure `step` stays the sole mutator.

---

## 10. Suggested blog spine (one way to tell it)

1. **Recap & the twist.** Same three processes, same wire, same behaviour as Stage II —
   "so what did I do for a whole iteration?" Set the expectation that this is a
   *structure/contract* milestone.
2. **The zone decomposition.** Flat `VehicleContext` + 238-line `step` monolith → four
   self-sufficient assemblies + a thin orchestrator. Show the before/after `step` snippets.
   Explain it as *mirroring the physical car's zones before making them concurrent.*
3. **Realism on a leash.** `WheelRpm` (4 wheels, broadcast for now), `PowertrainMode`
   (derived, not consumed) — the pattern of "introduce the shape, defer the cost, label the
   seam."
4. **Making the twin observable & provable.** Records carry intended actions; serializable
   published mirror (two clocks); `record_seq` vs command-seq naming; `as_of_seq` staleness;
   fact-ledger vs diagnostics; enforce/announce/detect; correct-by-construction twin.
5. **Lessons from a live run.** The `Ctrl-S` freeze → hot-path logging fix; silent-success
   diagnostic; `LogWarning` reclassification. (Relatable war-story section.)
6. **What I'm living with, and what's next.** The kept-limitations list (§8) and the
   actorification hand-off (§9) — set up iteration 3 as "now I split the monolith into the
   child actors this iteration was secretly preparing for."

---

## Appendix — concrete file-level evidence (for accuracy while drafting)

- **New in iteration 2:** `crates/common/src/fsm/assembly/{mod,powertrain,visibility,
  health,front_headlamp}.rs`, `crates/common/src/published.rs`.
- **`step.rs`:** 238 → ~158 lines; inline headlamp/lighting logic moved onto
  `HeadlampContext`; `TransitionRecord` → `RawTransitionRecord` (+ `actions` field).
- **`VehicleContext`:** moved from a flat struct in `fsm/machineries.rs` (iter 1) to an
  aggregate in `fsm/assembly/mod.rs` (iter 2).
- **`car_behaviour_checker.rs`:** `pub(super)` law fns (iter 1) → public `verify_state_laws`
  + `STATE_LAWS` catalog + `LawViolation` (iter 2); predicates now read
  `ctx.powertrain.{speed_kph, wheel_rpm.front_left}`.
- **`gateway_runtime.rs`:** iter 2 adds the bounded ingress-log side-channel
  (`try_send`, drop-on-full, "off the hot path"); iter 1 logs inline.
- **`digital_twin/mod.rs`:** iter 2 adds `CarSnapshot { car, as_of_seq }`, checked
  constructor, `apply_step`.
- **Design log:** `docs/design-notes-runtime-observation.md` (iteration-2 only) — the
  authoritative "why", with the `WI-*` ledger and `ADR-1..4`. The READMEs and the three
  `episode-0{1,2,3}` files are (intentionally) near-identical across the two repos, so the
  *diff* lives in this design log + the source tree, not in the episode notes.
