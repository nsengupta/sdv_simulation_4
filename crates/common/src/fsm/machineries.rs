//! FSM state, context, events, and domain actions for the vehicle twin.
//!
//! ## Front-headlamp incomplete / timeout
//!
//! ACK wait policy and recovery live in `crate::fsm::step` and `crate::vehicle_constants`. See
//! README *Known Demo Behaviors* for user-visible effects.

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
/// `TimedOut` is applied from `TimerTick` policy in `step` and may later be sent explicitly on ingress.
/// Future CAN work: add e.g. bus negative-ack codes here and map from `PhysicalCarVocabulary` / gateway decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FrontHeadlampIncompleteCause {
    /// No confirming ACK (and no bus-level failure frame) before the policy deadline — detected on [`FsmEvent::TimerTick`] in `step`.
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
pub struct VehicleContext {
    pub rpm: u16,
    /// Derived ground speed in km/h (from wheel RPM via [`crate::vehicle_kinematics`]).
    pub speed: u16,
    pub fuel_level: u8,
    pub oil_pressure: u8,
    pub tyre_pressure_ok: bool,
    pub ambient_lux: u16,
    pub lighting_state: LightingState,
    /// When set, we are waiting for a front-headlamp ACK for the current `OnRequested` / `OffRequested` state.
    pub lighting_ack_pending_since: Option<Instant>,
}

impl Default for VehicleContext {
    fn default() -> Self {
        Self {
            rpm: 0,
            speed: 0,
            fuel_level: 85,
            oil_pressure: 30,
            tyre_pressure_ok: true,
            ambient_lux: 100,
            lighting_state: LightingState::Off,
            lighting_ack_pending_since: None,
        }
    }
}

impl VehicleContext {
    pub fn is_healthy(&self) -> bool {
        self.fuel_level > 5 && self.oil_pressure > 10 && self.tyre_pressure_ok
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FsmState {
    Off,
    Idle,
    Driving,
    /// Speed > 160 km/h and RPM > 5500 sustained (see [`crate::vehicle_constants`]).
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
