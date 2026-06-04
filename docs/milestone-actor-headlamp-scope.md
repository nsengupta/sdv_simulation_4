# Milestone: `milestone/actor-headlamp` — scope & handoff

**Repo:** `sdv_simulation_3`  
**Base:** `main` @ tag **`pyramid-m2-complete`**  
**Blog arc:** Iteration 3 (actorification) starts on this branch.

Use this file to resume work in a new chat: `@docs/milestone-actor-headlamp-scope.md`.

---

## Done on `main` (do not redo here)

- **Pyramid (modules in `common`):** L0–L6 layout; TangleGuard **clean**
- **ADR-5 M1:** L1 alphabets (`HeadlampState` / `Message` / `Outcome`; other zones stubbed)
- **ADR-5 M2:** `zone_turn` → slim `fsm::step` → `twin_turn`; L1 emits **`HeadlampOutcome`** only
- **ADR-6:** target brain / ingress / power coordination — **documented**, not implemented
- **Deferred:** `sdv_core` crate split; full `TwinIngress`; power barrier; ledger `applied`; offline replay tool — see ADR-6 [Ledger tool / shutdown observability](adr-006-twin-brain-ingress-coordination.md#ledger-tool--shutdown-observability) (`applied: false` rows during `PowerOff`, not silent backlog)

---

## Branch goal

Turn the **headlamp zone** from in-process `HeadlampContext` + parent `zone_turn` into a **child actor (twinlet)**, with **one parent brain actor** and **unchanged** user-visible behaviour (CAN, three processes, tests green).

---

## Naming (use consistently)

| Name | Meaning |
| ---- | ------- |
| **`HeadlampZoneReply`** | Zone twinlet reply after **one** [`HeadlampMessage`] — `{ ctx, outcomes }`. Not a brain/FSM *turn*. |
| **`HeadlampOutcome`** | Zone egress only (RequestOn, LogWarning, …) — L4 maps to `DomainAction`. |
| **`HeadlampContext::on_receiving_message`** | L1 pure handler → `HeadlampZoneReply` (pattern for all zones). |
| **`tell_headlamp_zone`** | Brain **tell** to twinlet (`send_message`, no reply port). |
| **`HeadlampActorVocabulary`** | Tell payload: message, `turn_id`, brain `ActorRef`. |
| **`DigitalTwinCarVocabulary::HeadlampZoneReady`** | Twinlet tell-back; brain then [`commit_brain_turn`]. |

Avoid `*Turn` for zone replies — reserved for brain/FSM (`twin_turn`, `brain_twin_turn`).

---

## Q5 — summated view (decided)

| Phase | L3 `VehicleContext.headlamp` | Source of truth per event |
| ----- | ---------------------------- | ------------------------- |
| **Now (A)** | Embed full `HeadlampContext` | Copy `HeadlampZoneReply.ctx` before `apply_step` |
| **Target (C)** | Handle / slim projection | Whatever the child still puts in `HeadlampZoneReply` |

**Rule:** `HeadlampZoneReply` is semantic truth; parent does not `apply` in parallel with the actor. Shrinking the reply surfaces gaps via tests (ledger / `GetStatus`).

**A→C bridge:** Brain is *ask child → wait → refresh embed → ledger/diagnostics*. Optional `headlamp_reply` on [`zone_turn`](crates/common/src/twin_runtime/zone_turn.rs) only skips local `on_receiving_message` when the twinlet already handled that message — **temporary** until demux splits.

**L1 pattern (other zones):** `{Zone}Context::on_receiving_message(msg, now) -> {Zone}ZoneReply`.

---

## Brain operational policy (on tell-back — important)

The twin tells the **world** how the **physical sibling** is behaving **right now** (per assembly embed in `HeadlampZoneReply`). The brain applies **operational policy** when that tell-back is merged — not a separate “waiting” `FsmState`.

| Phase | Who | What |
| ----- | --- | ---- |
| **While actuation pending** | Assembly | e.g. `OnRequested`, `ack_pending_since` — enough for observers and policy inputs |
| **Operational mode** | L2 `FsmState` | May stay **unchanged** (e.g. `Driving`) until the **world model** says otherwise |
| **On tell-back** | Brain | `step` / journey rules read **summated** `VehicleContext` (lux + `headlamp` + speed, …) |

**Example (driving in the dark without a confirmed lamp):**

```text
Driving + lux low → tell headlamp → OnRequested, CMD sent
… N seconds, no ACK …
tell-back: timed out, lamp Off, LogWarning
brain policy: “driving without lighting is unsafe” → e.g. DrivingDangerously + alarm
```

- **Not** `FsmState::WaitingForHeadlamp` — assembly data is sufficient while remaining in the current operational state; no extra enum for “mailbox pending.”
- **Mode change** when the **aggregate** says unsafe (product/L2 rule), at **tell-back apply** time.
- **Stay** in the new operational state until a **corrective action** clears the condition (e.g. speed lowered, lamp confirmed ON, lux band recovery) — latched world model, not a one-shot log line.

Zone owns **timing and actuation truth**; brain owns **what that means for Driving / Danger / warnings**. Implement rules in L2 `transition_map` or a small journey-policy table beside `FsmState`, fed by embed after tell-back.

---

## Shutdown order (remember)

**Target:** assembly twinlets stop **before** the brain stops (supervisor-ordered teardown).  
**Interim:** brain `post_stop` stops headlamp — acceptable only until linked supervision / explicit ordered shutdown exists. Do not treat brain-owned `child.stop()` as the long-term model.

---

## Child → parent contract

```text
HeadlampMessage → apply_headlamp_zone → HeadlampZoneReply
Brain merges outcomes; embeds ctx; apply_step
```

---

## Tests (layers)

| Layer | Runs | Friction signal toward C |
| ----- | ---- | ------------------------ |
| **L1** | `on_receiving_message` / lighting contracts | Policy without ractor |
| **L4 pure** | `twin_turn` (sync zone reply) | Demux + FSM |
| **Actor** | Brain + headlamp child | RPC + embed |
| **Step 4** | `headlamp_reply_contract` | Ledger `current_ctx.headlamp` vs `GetStatus`; pending/settled |

Gateway e2e (Phase B) — deferred; will fail if snapshot fields shrink without reply/query path.

---

## Gate before more twinlets (step 6 — done)

**Vocabulary (author / team):**

| Word | Meaning |
| ---- | ------- |
| **Tell** | Fire-and-forget — no one blocked waiting for an answer on that hop. |
| **Send** | Request with a **receive side waiting** (sync coupling until reply). |

(ractor: **tell** ≈ `cast` / mailbox put without reply port; **send** ≈ `call` + `RpcReplyPort`.)

**Was (pre–step 6):** sync **send** (`call`) — brain `handle` blocked until headlamp replied.

| Was | Now (step 6) |
| --- | ------------ |
| Brain **send** / `call` | Brain **tell** via [`tell_headlamp_zone`](crates/common/src/twin_runtime/headlamp_actor.rs) |
| Reply in same `handle` | Twinlet **tell** [`HeadlampZoneReady`](crates/common/src/digital_twin/mod.rs) |
| One brain message = full turn | Tell out → tell back → `commit_brain_turn` / ledger |

**Target flow (one zone message):**

```text
Controller → Brain: Fsm(…)
Brain:      tell HeadlampActor { msg, turn_id }   // no receive side waiting; mailbox free
…           GetStatus / other Fsm may run …
Headlamp:   on_receiving_message → tell Brain: ZoneReady { turn_id, HeadlampZoneReply }
Brain:      merge reply → apply_step → ledger → actuation / diagnostics
```

**Still brain-owned:** `apply_step`, ledger, `record_seq`, `diag_front_headlamp_confirmed`, actuation egress — not in the twinlet.

**Still one apply per message** in the twinlet; only **coupling** changes (no RPC hold on brain mailbox).

**Do not add** other zone twinlets until this pattern is copied from headlamp. **Next:** ADR-6 power barrier (not step 6 `fsm_backlog` — see shutdown observability below).

**Open design (when implementing):** `turn_id` / correlation for out-of-order replies; ADR-6 M4 replaces step-6 `fsm_backlog` with power barrier + ledger-suppressed ingress (see ADR-6 shutdown observability).

---

## Shutdown observability (target — ADR-6)

During **`PowerOff` coordination**, stray ingress must still appear in the **ledger** with
`applied: false` so an offline tool can report (1) time from `PowerOff` to full shutdown and
(2) which messages arrived meanwhile. Step 6 **`fsm_backlog`** is interim only (no row until
commit). Full spec: [`adr-006` § Ledger tool / shutdown observability](adr-006-twin-brain-ingress-coordination.md#ledger-tool--shutdown-observability).

---

## In scope / out of scope

**In (this branch):** headlamp actor, tell/tell-back, embed from `HeadlampZoneReply`, tests, README.  
**Out:** other zone actors until headlamp pattern is stable; ADR-6 power barrier + `applied` ledger; `TwinIngress` on controller; `sdv_core` split; actuation child; M5 observability tool implementation.

---

## Architecture

```text
Controller → VirtualCarActor (brain): Fsm(…)
Brain → tell HeadlampActor → HeadlampZoneReady → commit_brain_turn → apply_step / ledger
```

---

## Success criteria (merge to `main`)

1. Headlamp behind child actor boundary.  
2. All tests green.  
3. Layering intact; Q5 + naming + shutdown notes in PR.  
4. Step 4 ledger/embed alignment tests.

---

## Key paths

| What | Path |
| ---- | ---- |
| L1 reply + apply | `crates/common/src/vehicle_state/front_headlamp.rs` |
| Headlamp actor | `crates/common/src/twin_runtime/headlamp_actor.rs` |
| Demux / twin turn | `crates/common/src/twin_runtime/{zone_turn,twin_turn}.rs` |
| Brain | `crates/common/src/twin_runtime/controller/virtual_car_actor.rs` |
| Step 4 tests | `crates/common/src/test/headlamp_reply_contract.rs` |

---

## Completion log

| Step | Status | Notes |
| ---- | ------ | ----- |
| 1 `on_receiving_message` | done | `HeadlampZoneReply` |
| 2 `HeadlampActor` | done | `apply_headlamp_zone` / vocabulary struct |
| 3 Brain dispatch | done | `commit_brain_turn` (was sync `brain_twin_turn`) |
| 4 Ledger/reply tests | done | `headlamp_reply_contract.rs` |
| 5 README | done | `e18fd35` — first zone actorification slice |
| 6 Tell / tell-back (no send/wait) | done | `tell_headlamp_zone`, `HeadlampZoneReady`, backlog |
| 7 Operational policy on tell-back | after 6 | e.g. DrivingDangerously until corrective action |

---

## Process

- **Commits:** confirm before commit  
- **One line:** First zone actorification — headlamp child under unchanged parent brain.
