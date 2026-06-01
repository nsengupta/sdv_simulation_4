//! FSM state, events, domain actions, and lighting vocabulary for the vehicle twin.
//!
//! Assembly (zone) **data** lives in `crate::vehicle_state`; this module holds the
//! shared FSM/domain vocabulary that assemblies, the operational FSM, and the
//! runtime actor all depend on.
//!
//! ## Front-headlamp incomplete / timeout
//!
//! ACK wait policy and recovery live in `crate::vehicle_state::front_headlamp` and
//! `crate::vehicle_physics`. See README *Known Demo Behaviors* for user-visible effects.

use crate::domain_types::VehicleState;
use std::time::Instant;

/// Which front-headlamp switch path an incomplete outcome refers to (ON vs OFF request in flight).
///
/// Complements [`LightingState::OnRequested`] / [`LightingState::OffRequested`] and pairs with
/// [`FsmEvent::FrontHeadlampOnAck`] / [`FsmEvent::FrontHeadlampOffAck`] for the success path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrontHeadlampSwitchDirection {
    On,
    Off,
}

/// Why a front-headlamp command did not **complete** with a positive acknowledgement.
///
/// `TimedOut` is applied from `TimerTick` policy in the headlamp assembly and may later be sent
/// explicitly on ingress. Future CAN work: add e.g. bus negative-ack codes here and map from
/// `PhysicalCarVocabulary` / gateway decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FrontHeadlampIncompleteCause {
    /// No confirming ACK (and no bus-level failure frame) before the policy deadline — detected on [`FsmEvent::TimerTick`].
    TimedOut,
    /// Actuator responded with an explicit negative acknowledgement for the command in flight.
    NegativeAck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightingState {
    Off,
    OnRequested,
    On,
    OffRequested,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FsmState {
    Off,
    Idle,
    Driving,
    /// Speed > 160 km/h and RPM > 5500 sustained (see [`crate::vehicle_physics`]).
    ExtremeOperationWarning(Instant),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FsmEvent {
    PowerOn,
    PowerOff,
    // Atomic updates from the bus
    UpdateRpm(u16),
    UpdateAmbientLux(u16),
    FrontHeadlampOnAck,
    FrontHeadlampOffAck,
    /// Front-headlamp command did not complete (see [`FrontHeadlampIncompleteCause`]).
    ///
    /// Gateway may inject this when CAN carries negative acknowledgement / failure (future).
    FrontHeadlampActuationIncomplete {
        direction: FrontHeadlampSwitchDirection,
        cause: FrontHeadlampIncompleteCause,
    },
    // Internal triggers
    TimerTick,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FsmAction {
    /// Trigger the physical buzzer (e.g., for overspeed/high RPM)
    StartBuzzer,
    /// Stop the physical buzzer
    StopBuzzer,
    /// Log a high-priority system alert
    LogWarning(String),
    /// Notify an external cloud/telemetry API of a state change
    PublishStateSync,
    /// No action required
    None,
}

/// Hint emitted by the domain step telling the runtime actor which mode to enter.
///
/// The domain emits the hint; the runtime actor owns `ActorMode` and mailbox behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorModeHintFromDomain {
    Normal,
    Transitioning,
}

/// Pure domain intents produced by [`crate::fsm::step`]; the runtime actor executes them.
#[derive(Debug, Clone, PartialEq)]
pub enum DomainAction {
    StartBuzzer,
    StopBuzzer,
    PublishStateSync,
    LogWarning(String),
    RequestFrontHeadlampOn,
    RequestFrontHeadlampOff,
    EnterMode(ActorModeHintFromDomain),
}

impl From<&FsmState> for VehicleState {
    fn from(fsm: &FsmState) -> Self {
        match fsm {
            FsmState::Off => VehicleState::Off,
            FsmState::Idle => VehicleState::Idle,
            FsmState::Driving => VehicleState::Driving,
            FsmState::ExtremeOperationWarning(_) => VehicleState::ExtremeOperationWarning,
        }
    }
}
