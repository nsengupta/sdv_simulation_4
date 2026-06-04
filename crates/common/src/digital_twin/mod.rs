//! **L3** twin state capsule: snapshot, invariants, and actor mailbox vocabulary.
//!
//! Depends on [`crate::fsm`] (L2) for [`FsmState`] and [`FsmEvent`], and on
//! [`crate::vehicle_state`] (L1) for [`VehicleContext`]. State laws in
//! [`car_behaviour_checker`] pair L0 constants from [`crate::vehicle_physics`] with L2 enforce paths.
//!
//! The pure decision core does not import this module (no `fsm → digital_twin` edge).
//! Runtime orchestration lives in [`crate::twin_runtime`] (L4). See
//! `docs/design-notes-pyramid-layers.md`.

mod car_behaviour_checker;

pub use car_behaviour_checker::{verify_state_laws, LawViolation, StateLaw, STATE_LAWS};

use crate::fsm::{FsmEvent, FsmState};
use crate::vehicle_state::VehicleContext;
use ractor::RpcReplyPort;

/// Returned when a [`DigitalTwinCar`] cannot be constructed because a constituent is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DigitalTwinCarError {
    /// Identity was empty or whitespace-only. A twin must have a non-blank identity.
    BlankIdentity,
}

impl std::fmt::Display for DigitalTwinCarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BlankIdentity => write!(f, "DigitalTwinCar identity must not be blank"),
        }
    }
}

impl std::error::Error for DigitalTwinCarError {}

/// Runtime snapshot of the vehicle digital twin: identity, FSM state, and sensor context.
///
/// Fields are **private**: a `DigitalTwinCar` can only come to exist via [`Self::new`] (which
/// guarantees a non-blank identity), and after birth its mutable state can only evolve through
/// [`Self::apply_step`] — the recorded result of the pure `fsm::step`, which is the *sole*
/// state mutator (see Q9 / ADR-3 in `docs/design-notes-runtime-observation.md`). External code
/// cannot set `current_state`/`context` to arbitrary values; this makes "twin with a blank
/// identity" and "twin mutated outside the FSM step" unrepresentable rather than runtime-checked.
#[derive(Debug, Clone)]
pub struct DigitalTwinCar {
    identity: String,
    current_state: FsmState,
    context: VehicleContext,
}

impl DigitalTwinCar {
    /// Construct a twin, validating the only structurally-invalid constituent: a blank
    /// identity (empty or whitespace-only). The identity is stored trimmed. `current_state`
    /// and `context` are caller-supplied (e.g. a freshly-born twin passes `FsmState::Off` +
    /// `VehicleContext::default()`).
    pub fn new(
        identity: impl Into<String>,
        current_state: FsmState,
        context: VehicleContext,
    ) -> Result<Self, DigitalTwinCarError> {
        let identity = identity.into();
        let trimmed = identity.trim();
        if trimmed.is_empty() {
            return Err(DigitalTwinCarError::BlankIdentity);
        }
        Ok(Self {
            identity: trimmed.to_owned(),
            current_state,
            context,
        })
    }

    /// The twin's (non-blank, trimmed) identity.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// The twin's current logical FSM state.
    pub fn current_state(&self) -> &FsmState {
        &self.current_state
    }

    /// The twin's sensor / health context.
    pub fn context(&self) -> &VehicleContext {
        &self.context
    }

    /// Evolve the twin by recording the result of a pure `fsm::step`. This is the **only**
    /// mutation path after construction, structurally enforcing that the FSM step is the sole
    /// state mutator (Q9 / ADR-3).
    pub fn apply_step(&mut self, next_state: FsmState, context: VehicleContext) {
        self.current_state = next_state;
        self.context = context;
    }

    /// Checks identity and context invariants on a snapshot (e.g. after `GetStatus`).
    /// The "Master Guardian"
    /// Returns Ok(()) if all safety laws are satisfied, or an Err describing the violation.
    ///
    /// Thin wrapper over the snapshot-only *runtime* concerns (health) plus the pure
    /// [`verify_state_laws`] catalog. The identity is no longer checked here: a non-blank
    /// identity is now guaranteed by construction ([`Self::new`]). Health stays a runtime
    /// check because it is time-varying (sensors change), not a construction invariant.
    pub fn verify_all_invariants(&self) -> Result<(), String> {
        if !self.context.is_healthy() {
            return Err("vehicle context failed health invariants".to_owned());
        }

        verify_state_laws(&self.current_state, &self.context).map_err(|violations| {
            violations
                .iter()
                .map(|v| format!("{}: {}", v.law, v.detail))
                .collect::<Vec<_>>()
                .join("; ")
        })
    }
}

/// A read-only snapshot returned by [`DigitalTwinCarVocabulary::GetStatus`].
///
/// Carries the twin plus `as_of_seq` — the ledger sequence (Counter A, `record_seq`) of the last
/// FSM event this snapshot reflects. A `GetStatus` reply is never "wrong", only *as-of* a point in
/// the event stream (Q3 / decision #2): it reflects events with sequence ≤ `as_of_seq`. Stamping it
/// makes that staleness legible and lets a consumer reconcile a snapshot against the `transition_tx`
/// ledger. `0` means no FSM event has been applied yet (freshly-born twin).
#[derive(Debug, Clone)]
pub struct CarSnapshot {
    car: DigitalTwinCar,
    as_of_seq: u64,
}

impl CarSnapshot {
    pub fn new(car: DigitalTwinCar, as_of_seq: u64) -> Self {
        Self { car, as_of_seq }
    }

    /// Ledger sequence of the last event this snapshot reflects (`0` = none applied yet).
    pub fn as_of_seq(&self) -> u64 {
        self.as_of_seq
    }

    /// The underlying twin value.
    pub fn car(&self) -> &DigitalTwinCar {
        &self.car
    }

    /// Delegating accessor: the twin's identity.
    pub fn identity(&self) -> &str {
        self.car.identity()
    }

    /// Delegating accessor: the twin's current FSM state.
    pub fn current_state(&self) -> &FsmState {
        self.car.current_state()
    }

    /// Delegating accessor: the twin's sensor / health context.
    pub fn context(&self) -> &VehicleContext {
        self.car.context()
    }

    /// Delegating check: run the twin's snapshot invariants (see [`DigitalTwinCar::verify_all_invariants`]).
    pub fn verify_all_invariants(&self) -> Result<(), String> {
        self.car.verify_all_invariants()
    }
}

/// Actor mailbox vocabulary for the digital twin: FSM traffic plus request/reply such as [`Self::GetStatus`].
///
/// [`FsmEvent`] stays `Clone` and free of [`RpcReplyPort`]; embed domain events via [`Self::Fsm`].
#[derive(Debug)]
pub enum DigitalTwinCarVocabulary {
    /// Drive the FSM (`crate::fsm::step` derives context from event payloads and computes transitions).
    Fsm(FsmEvent),
    /// Headlamp twinlet tell-back after [`crate::twin_runtime::headlamp_actor::HeadlampActor`] applied one message.
    HeadlampZoneReady {
        turn_id: u64,
        reply: crate::vehicle_state::HeadlampZoneReply,
    },
    /// Return an as-of snapshot of the twin (stamped with `as_of_seq`); does **not** call
    /// [`crate::fsm::transition`].
    GetStatus(RpcReplyPort<CarSnapshot>),
}

impl From<FsmEvent> for DigitalTwinCarVocabulary {
    fn from(evt: FsmEvent) -> Self {
        Self::Fsm(evt)
    }
}

/// Returned when a [`DigitalTwinCarVocabulary`] is not an [`FsmEvent`] wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotFsmVocabulary;

impl TryFrom<DigitalTwinCarVocabulary> for FsmEvent {
    type Error = NotFsmVocabulary;

    fn try_from(value: DigitalTwinCarVocabulary) -> Result<Self, Self::Error> {
        match value {
            DigitalTwinCarVocabulary::Fsm(e) => Ok(e),
            DigitalTwinCarVocabulary::GetStatus(_) | DigitalTwinCarVocabulary::HeadlampZoneReady { .. } => {
                Err(NotFsmVocabulary)
            }
        }
    }
}

impl DigitalTwinCarVocabulary {
    /// Borrow the inner [`FsmEvent`] when this message is [`Self::Fsm`].
    pub fn as_fsm_event(&self) -> Option<&FsmEvent> {
        match self {
            Self::Fsm(e) => Some(e),
            Self::GetStatus(_) | Self::HeadlampZoneReady { .. } => None,
        }
    }

    /// Take the inner [`FsmEvent`] when this message is [`Self::Fsm`].
    pub fn into_fsm_event(self) -> Option<FsmEvent> {
        match self {
            Self::Fsm(e) => Some(e),
            Self::GetStatus(_) | Self::HeadlampZoneReady { .. } => None,
        }
    }
}
