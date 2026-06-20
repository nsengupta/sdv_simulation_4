## Objectives

- Given that this REPO (sdv_simulation_4) is a starting from where the previous REPO 
(sdv_simulation_3) stopped, make a quick assessment of what already exists.
- The primary objectives of this iteration are
 
  - Categorize events coming to VirtualCarActor better, so that the handle() function doesn't 
    become unmanageably long to read
    - Events like this crates/common/src/twin_runtime/controller/virtual_car_actor.rs:234 are 
      captured at the handle() function itself. More such events (from other assemblies) -> more 
      crowded the handle() function is going to be
  - Revisit the whole begin_fsm_turn(..) call tree. It is more complicated than it should be.
      - Very few functions in this call tree have meaningful comments. Those are necessary for 
        helping readers, to form the mental map.
      - Logic to check if the event is Headlamp related or switch-off related are sprinkled around
      - transition(..) is called only from inside step(..) here crates/common/src/fsm/step.rs:43. 
        This is good and must be maintained
      - Revisit this `if` block: crates/common/src/fsm/step.rs:61. Do we really need this and if 
        yes, why? 
      - Is this function crates/common/src/twin_runtime/twin_turn.rs:126, really required? Can we wire it in a different way?
      - Function zone_turn(..) crates/common/src/twin_runtime/zone_turn.rs:49 is called from only 
        two places. That is good. But the function has all the knowledge of Headlamp etc. (will 
        increase tomorrow as more assembly actors are added). Consider a refactor.
      - Rearrange the call tree in such a way that the flow is obvious.
      - Testcases must be added/edited but all tests must be very strict
      - Earlier plan already mentions addition of tests that fire random events to the Actor and 
        establishes that the Actor always ends up in a predictable, steady state.
  - Introduce new Digital Twin (Virtual Car) Brain state: PoweredOff -> Idle and vice versa.
  - Ignition/Switch Off events are emulated by the emulator:
    - Switch On -> CAN ID 0x100 -> Payload 01 00 00 00 00 00 00 00
    - Switch Off -> CAN ID 0x100 -> Payload 00 00 00 00 00 00 00 00
  - SwitchOn event will lead the Digital Twin to Idle state from PoweredOff 
    (refer to: findings/startup-shutdown-sequence-analysis.md)
  - The Headlamp actuator is a synchronous call to a tokio Channel today; we should make it a 
    separate child actor so that actuation doesn't hold the main thread of virtual car actor 
    (refer to findings/single-thread-guarantee.md|Category 2 — actuation_manager.execute). This 
    child actor is the replacement of ActuationManager. It is responsible for sending out all 
    actuation instructions on the CAN Bus. As such, it must:
    - Be capable of translating Virtual Car's vocabulary to CAN Bus event
    - Abstract away the tokio channel to which is posts the actuation message (gateway is 
      already at the receiving end of this channel)
  - Introduce one more child actor assembly, similar to Headlamp (but this is Nice-To-Have in 
    this iteration). However, the work done in this iteration must lead to readiness for more 
    such child actor assemblies.
  - We will implement each of these main additional features/reorganizations, in clear, 
    identifiable, merge ready stages; each stage is a branch of main.