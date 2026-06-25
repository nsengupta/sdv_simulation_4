# Contract tests

**Purpose:** what each named contract suite in `crates/common` validates. The README shows
how to run tests; **this document is the reference table**.

Run all common tests:

```bash
cargo test -p common
```

Run a specific suite:

```bash
cargo test -p common -- test::<suite_name>
```

---

| Suite                            | What it validates                                                             |
| -------------------------------- | ----------------------------------------------------------------------------- |
| `quiescence_actor_contract`      | End-to-end Brain turns: ingress → tell → tell-back → commit → ledger          |
| `zone_tell_back_contract`        | Per-assembly retry, exhaustion, synthetic reply, concurrent assembly timeouts |
| `turn_barrier_contract`          | ROB ordering: reverse-reply-order, backlog drainage, multi-barrier drain      |
| `headlamp_ack_timer_contract`    | ACK deadline, spontaneous incomplete, DrivingDangerously transition           |
| `headlamp_lifecycle_contract`    | `HeadlampContext::on_receiving_message` in isolation (no actor)               |
| `wiper_zone_contract`            | Wiper routing, L1 transitions, startup barrier, ROB ordering with wiper       |
| `wiper_actuation_contract`       | Domain actions, outcome_map, actuation channel, physical rain e2e             |
| `wiper_signal_contract`          | `VssSignal::RainDetected` CAN encode/decode                                   |
| `wiper_startup_failure_contract` | Silent wiper → tell-back exhaustion → diagnostic warning                      |
| `scenarios_smoke`                | Full scenario: PowerOn → Idle → Driving → PowerOff                            |

Wiper CAN codec (`vehicle_device_bus`):

```bash
cargo test -p vehicle_device_bus --test wiper_can_codec
```
