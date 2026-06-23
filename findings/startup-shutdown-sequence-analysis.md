# Startup Sequence Analysis

## Objective
We want to 
- start the car this way:  
`PoweredOff -- [Ignition on] --> Idle`  
- Stop the car this way:
`Idle -> [Ignition off] --> PoweredOff`

## Analysis

### Current Design
When the VirtualCarActor starts (`pre_start` → `handle(Startup)` → FSM transitions from `Off` to `Idle`):
1. The FSM transition `Off + PowerOn + is_healthy() → Idle` is processed
2. `begin_fsm_turn` is called which checks `fsm_event_headlamp_message(PowerOn)` → returns `None`
3. Then checks `fsm_step_lands_off()` with `(Off, Idle)` → returns `false` (entering Off would be no-change)
4. So the turn is **committed directly** — no headlamp wait at all
5. Car is now `Idle` but headlamps have never been told about ambient light

### Implications
- **Startup is fast**: No headlamp zone interaction delays the `Off → Idle` transition
- **Headlamps remain Off**: Since no headlamp message was sent, headlamp zone stays in whatever state it was in (likely Off at system startup)
- **Lazy initialization**: The first `UpdateAmbientLux` event will trigger `begin_fsm_turn` with a headlamp message, which is when the headlamp zone gets its first instruction
- **Potential concern**: If the car starts in a dark environment, the first moment headlamps are told about ambient conditions is when `UpdateAmbientLux` arrives, not at startup

### Overall design approach

- Starting
  - Emulator sends a specific CAN ID-bearing message (IgnitionOn) through the CAN Bus
  - Virtual Car's Brain actor receives this event
  - Goes into a 'PreparingToStart' state 
  - In this state:
    - Checks Health assembly (always OK in this iteration)
    - Checks PowerTrain assembly (always OK in this iteration)
    - Wakes up Headlamp assembly (a child/zone actor)
    - Wakes up other assembly (a child/zone actor; clone of Headlamp; may be Wiper) by sending a 
      'BecomeOn' message/event
    - Till the time, all the assemblies announce OK (or NOT OK), current state remains 'PreparingToStart'
    - One or more assembly may not respond within stipulated time (timers are needed)
    - While in 'PreparingToStart', all other events arriving are ignored (transition ledger makes a note)
    - Finally, the Digital Twin (FSM) moves to 'Idle' state
- Stopping
  - Emulator sends a specific CAN ID-bearing message (IgnitionOff) through the CAN Bus
  - Virtual Car's Brain actor receives this event
  - Proceeds only if the current state is 'Idle'
  - Goes into a 'PreparingToStop' state
  - Instructs every assembly with a  'BecomeOff' message/event
  - Till the time, all the assemblies announce OK (or NOT OK), current state remains 'PreparingToStop'
  -  While in 'PreparingToStop', all other events arriving are ignored (transition ledger makes a note)
  - Finally, the Digital Twin (FSM) moves to 'PoweredOff' state

### General considerations

- Headlamp Assembly/ZoneActor is a child actor of Brain actor, so its lifecycle is controlled 
    by the Brain by #ractor's internal design
- Same is the case with all other such assemblies (clones of Headlamp)
- Health, Powertrain etc. are in-memory data structures in this iteration. They have a function 
  that receives events like 'BecomeOn'/'BecomeOff', responds in synch calls
- Brain actor always prepares for conversation with Zone actors, assuming that response may not 
  arrive in stipulated time; timer for each such conversation is set up and torn down, if not fired
