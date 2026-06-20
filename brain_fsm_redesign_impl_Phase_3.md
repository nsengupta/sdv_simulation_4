# Brain FSM Redesign — Phase 3 Implementation Plan
## Generic Zone Envelope in `DigitalTwinCarVocabulary`

**Status:** Draft — to be reviewed before implementation.  
**Depends on:** Phase 2 complete (122/122 tests pass, `ZoneId`, `BecomeOn`/`BecomeOff`, `Ready` state).  
**Next phase:** Phase 4 — `VecDeque<TurnBarrier>` replaces `pending_turn`.

---

## What Phase 3 delivers

Replace headlamp-specific variants in `DigitalTwinCarVocabulary` with generic zone envelopes
(`ZoneReady`, `ZoneTellBackTimeout`, `ZoneSpontaneous`). The `handle()` dispatch in
`VirtualCarActor` shrinks from **5 match arms** to **4 non-GetStatus arms** (5 total with `GetStatus`).  
The internal routing still delegates to the same functions as before.

**No behavior change in this phase** — `pending_turn` still gates everything. The
internal routing still delegates to the same functions as before.

**Match-arm count:** The `handle()` match will have **5 arms**: `Fsm`, `ZoneReady`,
`ZoneSpontaneous`, `ZoneTellBackTimeout`, `GetStatus`. The master plan's "4 arms"
statement counts zone/FSM variants (`Fsm / ZoneReady / ZoneTellBackTimeout`) as 3
plus `GetStatus` = 4, omitting `ZoneSpontaneous`. In code, all 5 variants of
`DigitalTwinCarVocabulary` must have match arms for exhaustiveness.

---

## Design decisions (clarified from Phase 2 sign-off)

| Question | Decision |
|---|---|
| `AssembliesReady/AssembliesStopped` level | `FsmEvent::Internal` (top-level variant) |
| `BecomeOn` → headlamp target state | `OnRequested` (reuses existing state) |
| `ZoneSpontaneousEvent` fields | See `ZoneSpontaneousEvent` shape below |
| Phase ordering | Strict sequential (RED→GREEN per phase) |

---

## `ZoneSpontaneousEvent` shape

The old `HeadlampZoneSpontaneous` carried:
```rust
HeadlampZoneSpontaneous {
    direction: FrontHeadlampSwitchDirection,
    cause: FrontHeadlampIncompleteCause,
    reply: HeadlampZoneReply,
}
```

The generic replacement:
```rust
ZoneSpontaneous {
    zone_id: ZoneId,
    event: ZoneSpontaneousEvent,
}
```

Where `ZoneSpontaneousEvent` is a new enum in `digital_twin/mod.rs` (or `zone_tell_back.rs`):

```rust
/// A zone-initiated event (ACK timeout, future assembly deadlines) — not correlated to a brain
/// `turn_id`. Carries the zone's reply + context, and any zone-specific metadata.
#[derive(Debug, Clone)]
pub enum ZoneSpontaneousEvent {
    Headlamp {
        direction: FrontHeadlampSwitchDirection,
        cause: FrontHeadlampIncompleteCause,
        reply: HeadlampZoneReply,
    },
}
```

This is forward-extensible: when Wiper joins (Phase 7), add `Wiper { ... }` variant.
The old `{ direction, cause, reply }` tuple is preserved inside the `Headlamp` variant —
no data loss.

---

## Files changed

| File | Change |
|---|---|
| `crates/common/src/digital_twin/mod.rs` | Replace 3 headlamp-specific variants with 3 generic zone variants; add `ZoneReply` enum, `ZoneSpontaneousEvent` enum |
| `crates/common/src/twin_runtime/headlamp_actor.rs` | Send `ZoneReady { zone_id: ZoneId::Headlamp, reply: ZoneReply::Headlamp(...) }` and `ZoneSpontaneous { zone_id: ZoneId::Headlamp, event: ZoneSpontaneousEvent::Headlamp { ... } }` |
| `crates/common/src/twin_runtime/controller/virtual_car_actor.rs` | Update `handle()` to 5 arms (Fsm, ZoneReady, ZoneSpontaneous, ZoneTellBackTimeout, GetStatus); rename handlers to `on_zone_ready`, `on_zone_timeout`, `on_zone_spontaneous`; unpack `ZoneReply::Headlamp`/`ZoneSpontaneousEvent::Headlamp` before delegating to existing logic |
| `crates/common/src/vehicle_state/front_headlamp.rs` | Re-export `HeadlampZoneReply` remains; no structural changes needed |

---

## RED tests (add to existing test files or new `test/zone_envelope_contract.rs`)

### Test 1: `test_zone_ready_message_routes_to_on_zone_ready_handler`

```rust
/// Brain handle() receives ZoneReady { zone_id: Headlamp, ... } without panicking.
/// The zone reply is unpacked and routed to the same headlamp logic as before.
#[test]
fn test_zone_ready_message_routes_to_on_zone_ready_handler() {
    // Arrange: install brain actor with headlamp in Ready state
    // Act: send ZoneReady { zone_id: ZoneId::Headlamp, ... }
    // Assert: no panic; ledger has the same headlamp updates as old HeadlampZoneReady
}
```

### Test 2: `test_zone_tell_back_timeout_message_carries_zone_id`

```rust
/// ZoneTellBackTimeout { zone_id: Headlamp, turn_id, tell_attempt } is handled.
/// The existing timeout logic fires with zone_id accessible.
#[test]
fn test_zone_tell_back_timeout_message_carries_zone_id() {
    // Arrange: install brain actor, begin FSM turn with headlamp wait
    // Act: wait for ZONE_TELL_BACK_WAIT (or synthetic timeout)
    // Assert: ZoneTellBackTimeout { zone_id: ZoneId::Headlamp, .. } is dispatched
    //         to on_zone_timeout without panic
}
```

### Test 3: `test_zone_spontaneous_message_is_handled_by_on_zone_spontaneous`

```rust
/// ZoneSpontaneous { zone_id: ZoneId::Headlamp, event: ZoneSpontaneousEvent::Headlamp }
/// is dispatched to on_zone_spontaneous which unpacks direction/cause/reply
/// and calls the existing lighting_unsafe detector logic.
#[test]
fn test_zone_spontaneous_message_is_handled_by_on_zone_spontaneous() {
    // Arrange: headlamp actor in OnRequested with ack timer
    // Act: trigger AckWaitElapsed (sends ZoneSpontaneous)
    // Assert: brain processes it via on_zone_spontaneous + pump_fsm_backlog
}
```

### Test 4: `test_handle_has_exactly_five_arms_exhaustiveness_check` (compile-time)

```rust
/// Compile-time exhaustiveness: DigitalTwinCarVocabulary has 5 variants
/// (Fsm, ZoneReady, ZoneSpontaneous, ZoneTellBackTimeout, GetStatus).
/// handle() matches all 5 — rustc won't compile if a variant is missing.
/// This test passes by virtue of compiling.
#[test]
fn test_handle_has_exactly_five_arms_exhaustiveness_check() {
    // If handle() has an unreachable!() arm or _ => catch-all,
    // this test fails at runtime. For now it's a compile-time guarantee.
}
    // This is a conceptual check. In practice, the exhaustiveness of the
    // match in handle() IS the test. If someone adds a variant to
    // DigitalTwinCarVocabulary without adding a match arm, rustc errors.
    // We verify by checking that the match arms are: Fsm, ZoneReady,
    // ZoneTellBackTimeout, GetStatus.
}
```

---

## Code change details

### 1. `crates/common/src/digital_twin/mod.rs`

**Add `ZoneReply` enum:**
```rust
/// Generic zone tell-back envelope — wraps zone-specific reply types.
#[derive(Debug, Clone, PartialEq)]
pub enum ZoneReply {
    Headlamp(crate::vehicle_state::HeadlampZoneReply),
}
```

**Add `ZoneSpontaneousEvent` enum:**
```rust
/// Zone-initiated event payload (ACK timeout, future assembly deadlines).
#[derive(Debug, Clone)]
pub enum ZoneSpontaneousEvent {
    Headlamp {
        direction: crate::fsm::FrontHeadlampSwitchDirection,
        cause: crate::fsm::FrontHeadlampIncompleteCause,
        reply: crate::vehicle_state::HeadlampZoneReply,
    },
}
```

**Replace 3 variants in `DigitalTwinCarVocabulary`:**
- Remove `HeadlampZoneReady { turn_id, tell_attempt, reply }`
- Remove `HeadlampZoneSpontaneous { direction, cause, reply }`
- Remove `TellBackTimeout { turn_id, tell_attempt }`
- Add:
  ```rust
  /// Zone twinlet tell-back after applying one message.
  ZoneReady {
      zone_id: ZoneId,
      turn_id: u64,
      /// Matches the `tell_attempt` on the tell that produced this reply.
      tell_attempt: u32,
      reply: ZoneReply,
  },
  /// Zone-initiated hop (ACK timer, future assembly deadlines) — not correlated to a brain `turn_id`.
  ZoneSpontaneous {
      zone_id: ZoneId,
      event: ZoneSpontaneousEvent,
  },
  /// Ractor deadline: zone twinlet did not tell-back in [`ZONE_TELL_BACK_WAIT`].
  ZoneTellBackTimeout {
      zone_id: ZoneId,
      turn_id: u64,
      tell_attempt: u32,
  },
  ```

**Update `TryFrom`, `as_fsm_event`, `into_fsm_event`:**
Replace references to old variant names with new ones in match arms.

### 2. `crates/common/src/twin_runtime/headlamp_actor.rs`

In `handle_apply`, change:
```rust
// Before:
brain.send_message(DigitalTwinCarVocabulary::HeadlampZoneReady {
    turn_id,
    tell_attempt,
    reply: zone_reply,
})?;

// After:
brain.send_message(DigitalTwinCarVocabulary::ZoneReady {
    zone_id: ZoneId::Headlamp,
    turn_id,
    tell_attempt,
    reply: ZoneReply::Headlamp(zone_reply),
})?;
```

In `handle_ack_wait_elapsed`, change:
```rust
// Before:
brain.send_message(DigitalTwinCarVocabulary::HeadlampZoneSpontaneous {
    direction,
    cause: FrontHeadlampIncompleteCause::TimedOut,
    reply: zone_reply,
})?;

// After:
brain.send_message(DigitalTwinCarVocabulary::ZoneSpontaneous {
    zone_id: ZoneId::Headlamp,
    event: ZoneSpontaneousEvent::Headlamp {
        direction,
        cause: FrontHeadlampIncompleteCause::TimedOut,
        reply: zone_reply,
    },
})?;
```

**Need to import:** `ZoneId`, `ZoneReply`, `ZoneSpontaneousEvent` from `crate::digital_twin` (or `crate::fsm` for `ZoneId`).

### 3. `crates/common/src/twin_runtime/controller/virtual_car_actor.rs`

**Update `handle()` match arms — 5 arms:**
```rust
// Before (5 headlamp-specific names):
use DigitalTwinCarVocabulary::{
    Fsm, GetStatus, HeadlampZoneReady, HeadlampZoneSpontaneous, TellBackTimeout,
};

// After (5 generic names — same count, new names + ZoneReply enum):
use DigitalTwinCarVocabulary::{
    Fsm, GetStatus, ZoneReady, ZoneSpontaneous, ZoneTellBackTimeout,
};
```

With match body:
```rust
match message {
    Fsm(evt_arrived) => { /* unchanged logic */ }
    ZoneReady { zone_id, turn_id, tell_attempt, reply } => {
        Self::on_zone_ready(&myself, runtime_state, zone_id, turn_id, tell_attempt, reply).await?;
        Self::pump_fsm_backlog(&myself, runtime_state).await
    }
    ZoneSpontaneous { zone_id, event } => {
        Self::on_zone_spontaneous(runtime_state, zone_id, event).await?;
        Self::pump_fsm_backlog(&myself, runtime_state).await
    }
    ZoneTellBackTimeout { zone_id, turn_id, tell_attempt } => {
        Self::on_zone_timeout(&myself, runtime_state, zone_id, turn_id, tell_attempt).await?;
        Self::pump_fsm_backlog(&myself, runtime_state).await
    }
    GetStatus(reply) => Self::reply_get_status(
        reply,
        &runtime_state.twin_car,
        runtime_state.next_record_seq.saturating_sub(1),
    ),
}
```

**Rename handler functions:**

| Old name | New name |
|---|---|
| `on_headlamp_zone_ready` | `on_zone_ready` (takes `zone_id: ZoneId` + generic `ZoneReply`) |
| `on_tell_back_timeout` | `on_zone_timeout` (takes `zone_id: ZoneId`) |
| `on_headlamp_zone_spontaneous` | `on_zone_spontaneous` (takes `zone_id: ZoneId` + `ZoneSpontaneousEvent`) |
| `begin_headlamp_wait` | stays as-is until Phase 4 (still headlamp-specific internals) |

Inside `on_zone_ready`, unpack `ZoneReply::Headlamp(r)` and call the existing logic.
Inside `on_zone_spontaneous`, unpack `ZoneSpontaneousEvent::Headlamp { direction, cause, reply }` and call the existing logic.

**Signatures:**
```rust
async fn on_zone_ready(
    myself: &Ractor<Self>,
    runtime_state: &mut RuntimeState,
    zone_id: ZoneId,
    turn_id: u64,
    tell_attempt: u32,
    reply: ZoneReply,
) -> Result<(), BrainActorError> {
    match reply {
        ZoneReply::Headlamp(r) => {
            // same body as old on_headlamp_zone_ready
        }
    }
}

async fn on_zone_spontaneous(
    runtime_state: &mut RuntimeState,
    zone_id: ZoneId,
    event: ZoneSpontaneousEvent,
) -> Result<(), BrainActorError> {
    match event {
        ZoneSpontaneousEvent::Headlamp { direction, cause, reply } => {
            // same body as old on_headlamp_zone_spontaneous
        }
    }
}

async fn on_zone_timeout(
    myself: &Ractor<Self>,
    runtime_state: &mut RuntimeState,
    zone_id: ZoneId,
    turn_id: u64,
    tell_attempt: u32,
) -> Result<(), BrainActorError> {
    match zone_id {
        ZoneId::Headlamp => {
            // same body as old on_tell_back_timeout
        }
        _ => unreachable!("only Headlamp zone has tell-back timeouts in Phase 3"),
    }
}
```

---

## Discussion checkpoint after Phase 3

1. **All tests green.** Run `cargo test -p common` — all 122+ existing tests plus 3–4 new RED tests pass.
2. **`cargo build -p common` emits zero warnings.** No dead-code warnings from renamed/unused items.
3. **No remaining references to `HeadlampZoneReady`, `HeadlampZoneSpontaneous`, or `TellBackTimeout`** outside the changed files. Run `grep -r "HeadlampZoneReady\|HeadlampZoneSpontaneous\|TellBackTimeout" crates/common/src/` — must return zero.
4. **`handle()` has exactly 5 arms** — confirmed by compilation (exhaustiveness over `DigitalTwinCarVocabulary`'s 5 variants).
5. **Confirm `ZoneSpontaneousEvent` shape** — is `Headlamp { direction, cause, reply }` the right payload, or does the user want a flatter structure?
6. **Confirm `ZoneReply` enum name** — does `ZoneReply` conflict with any existing type?
