//! L2 operational FSM: mode ([`FsmState`]) + [`crate::fsm::transition_map`] only mutates mode.
//!
//! **Cut** — one twin snapshot `(FsmState, VehicleContext)` at an instant; each ledger hop is
//! entry → exit. **Quiescence** — process external + [`FsmEvent::Internal`] hops before commit
//! (ADR-7: `docs/adr-007-fsm-quiescence-and-cut.md`).

pub mod machineries;
pub mod step;
pub mod transition_map;

pub use crate::vehicle_state::HeadlampState;
pub use machineries::{
    DomainAction, FrontHeadlampIncompleteCause,
    FrontHeadlampSwitchDirection, FsmAction, FsmEvent, FsmState, Operational, ZoneId,
};
pub use step::{step, StepResult, RawTransitionRecord};
pub use transition_map::{output, transition, TransitionNote, TransitionResult};
