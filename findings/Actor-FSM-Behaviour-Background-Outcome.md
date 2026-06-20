
## Why have we chosen an Actor to implement the bheviour of a Virtual Car's brain?

- An Actor is single-threaded, event-driven by design and therefore, independently testable; an 
  excellent building block for distributed design
- Such an Actor can be relatively easily integrated with an FSM; they work great together. Again,
  an FSM must be a pure function, testable and predictable under any circumstances
- A Virtual Car is collecting a large number of messages from various parts/assemblies/zones of 
  the Physical car; each message is considered an event that the Virtual Car Actor's vocabulary 
  includes. Processing is easy to understand: one event at a time (all events external).
- The FSM can, additionally, depend on internal events to make sure that the State Transitions 
  are accurate.

## Virtual Car Actor's constraints

- We are using #ractor library of #rustlang. Guarantees sequential execution of events arriving 
  at the Actor's mailbox. However, it doesn't allow events to jump the queue ever; no priority 
  message handling.
- The Actor is single-threaded; so, the quicker it plays its role in handling an event, the 
  better for the overall throughput and responsiveness. 
- The functions that are called from handle(..) function, will be synch functions because they are 
  expected to process in-memory data.
- Whenever handle() call-chain invokes a function that may benefit by being run asynchronously, 
  we spawn a task, let that task handle the work, and then post the result back to the same 
  Actor through its mailbox.

## How does an Actor and FSM coordinate?

- The Actor's current state is always derived from the FSM; FSM always starts with SwitchedOff 
  state, indicating that the Virtual Car is always 'switched off' when we run the code from 
  command-line
- The FSM must run in the same thread that runs the handle() function; always synchronously!
- The FSM is a pure function; the entire execution is supposed to be in-memory; so, we are fine
  for the time being
- The FSM's Transition table (refer to transition() function here - 
  crates/common/src/fsm/transition_map.rs:39), determines the next state, the Actor is supposed 
  to be in
- The VehicleContext, passed as a param, may be modified during the transition
- Post-transition, the FSM needs to tell what actions to be taken (refer to output() function 
  called from here: crates/common/src/fsm/step.rs:45)
- The calls to transition() and output() must happen in strict order and always together and 
  only once in the codebase; this is because (a) the output() is direct outcome of the 
  transition and (b) transitions cannot be interleaved (determinism is non-negotiable)

## The case of FSM's internal states
- The Virtual Car actor's handle() and its call-tree reaches the FSM in the same sync-call 
  thread. Therefore, the transition() happens in the same thread. By the time the thread returns 
  to the handle() function, actor's mailbox may contain a new event and the actor is bound to 
  handle() that next. By this time though, the FSM has determined the next state of the Virtual Car.
- This arrangement is perfect as long as the FSM doesn't need transition to an internal state 
  and transition again based on that internal state. Importantly, this transition - a re-entry 
  into the FSM - happens in the same thread; the VirtualCar actor is unaware of this transient 
  state that it is in.
- Handling these internal states is extremely important (NFA -> DFA) to keep the purity of the 
  FSM intact
- Because 
  - the FSM doesn't have a queue of its own - actor's mailbox's top/head is what it 
    processes everytime - and 
  - DFA-based transition has to happen in the same thread, 
  a mechanism is needed to feed the internal event into the FSM, so that the transition() function returns 
  only once and by that time the next state is determined.
  - Refer to docs/adr-007-fsm-quiescence-and-cut.md for some context
- These internal State/Event transitions must be captured in the transition ledger because 
  replay must be accurate (no transition ignored)

## Assembly (child) Actors
- Each Assembly is supposed to be a child actor, with its own data, logic and state machine, if 
  necessary.
- At the moment, the Headlamp is such an assembly and is implemented as actor
- Events that are meant for such an assembly, are routed to it by the brain actor. No assembly 
  actor interacts with Physical world directly.
- Because Brain and Assembly actors interact between themselves using events (or messages, as 
  applicable semantically), the calls are asynchronous (tell / fire-and-forget)
- Because it expects Assembly to respond ASAP, the Brain keeps a timer for each such 'tell'; 
  either this timer fires ('assembly has not responded within stipulated time') or the 
  assembly's tell-back message arrives. In case of the latter, the timer is canceled (best 
  effort; the timer may still fire; to be ignored; every timer has an ID to facilitate this).
- The design challenge: 
  - An event A arrives at Brain's mailbox
  - Brain diverts that to the Assembly (say, Headlamp)
  - Another event B arrives at Brain's mailbox, before Assembly's tell-back arrives
  - It is possible that the logic to process B depends on whether Tellback/A has arrived already 
    or not
  - It is possible that meanwhile event C already arrives in Brain's mailbox
  - How does Brain deal with this?

  The current design solves it by introducing an intermediate FIFO queue for the Brain Actor 
  (separate from the mailbox's queue) but the implementation tends to be complicated.
- With multiple assemblies, the story becomes even more entangled

## General points

- We don't want a State/Event explosion in Brain's FSM, but
- We don't want to dilute the FSM-basis of Brain's Actor either
- Brain has to coordinate; therefore, it has to 'remember' some of its earlier actions (viz, 'I have 
  told Headlamp to switch itself on, but it is yet to confirm that it has') in order to take the 
  present action correctly (viz., 'Visibility is better now but it is raining').
- Logic to deal with specific Tell-back messages must not be sprinkled in the main functions 
  that tie the Brain Actor and its FSM together
- Headlamp assembly will be a template for the next assembly (yet to be introduced)
- Health, Powertrain etc. are also assemblies but at the moment, they will remain in-memory 
  contexts  but they will hold the current state of the assemblies, as if they are actors too; 
  only the inquiries / updates are synchronous