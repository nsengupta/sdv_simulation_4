# Project structure

**Purpose:** directory layout for Iteration 4 — workspace crates, the `common` pyramid (L0–L5),
L6 binaries, diagrams, and supporting docs. The README summarises the architecture; **this
document is the tree**.

---

```text
crates/
├── common/                     # L0–L5 library pyramid
│   └── src/
│       ├── vehicle_physics/    # L0: constants, kinematics
│       ├── vehicle_state/      # L1: assembly contexts (powertrain, health, visibility, headlamp, wiper)
│       ├── fsm/                # L2: FsmState, FsmEvent, step(), transition_map
│       │   ├── machineries.rs  #    State/event/action enums, AssemblyId, ALL_ASSEMBLIES
│       │   ├── transition_map.rs  # transition() + output()
│       │   └── step.rs         #    step() orchestrator
│       ├── digital_twin/       # L3: DigitalTwinCar, mailbox vocabulary
│       ├── published/          # L3': PublishedTransitionRecord
│       ├── twin_runtime/       # L4: actors, turn_barrier, detectors
│       │   ├── controller/     #    VirtualCarActor (Brain)
│       │   ├── headlamp_actor.rs  # HeadlampActor twinlet
│       │   ├── wiper_actor.rs     # WiperActor twinlet
│       │   ├── turn_barrier.rs    # TurnBarrier, BarrierEntry, TellBackWait
│       │   ├── zone_tell_back.rs  # TellBackWait, retry logic (legacy, migrating to turn_barrier)
│       │   ├── zone_turn.rs       # zone_message_for_event routing
│       │   ├── zone_replies.rs    # ZoneReplies helper
│       │   └── detectors/      #    LightingUnsafe, etc.
│       ├── facade.rs           # L5: public API surface
│       └── lib.rs              # module declarations
├── gateway/                    # L6: CAN I/O, Brain wiring, actuation publishers
├── emulator/                   # L6: RPM + lux + rain publisher
├── front_headlamp_actuator/    # L6: headlamp body ECU stand-in
├── wiper_actuator/             # L6: wiper motor stand-in (fire-and-forget)
└── vehicle_device_bus/         # L6: per-device CAN codecs (headlamp, wiper)
diagrams/
├── brain_transitions.md              # Brain FSM state diagram
├── headlamp_assembly_state_transition.md  # Headlamp assembly lifecycle
└── wiper_assembly_state_transition.md     # Wiper assembly lifecycle
docs/
├── contract-tests.md           # Contract suite reference table
├── design-documents.md         # Architecture and diagram index
├── library-reorg.md            # Pyramid layering detail
├── project-structure.md        # This tree
└── rpm-model-tutorial.md       # Emulator RPM model explanation
```

Layering detail: [`library-reorg.md`](library-reorg.md).
