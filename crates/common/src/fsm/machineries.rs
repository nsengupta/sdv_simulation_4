//! L2 operational vocabulary: mode, ledger events, and domain actions.
//!
//! Zone snapshots and alphabets live in [`crate::vehicle_state`]. Headlamp ingress
//! direction/cause types are re-exported here for [`FsmEvent`] only.

use crate::domain_types::VehicleState;
use std::time::Instant;

pub use crate::vehicle_state::{FrontHeadlampIncompleteCause, FrontHeadlampSwitchDirection};

#[derive(Debug, Clone, PartialEq)]
pub enum FsmState {
    Off,
    /// Assemblies are being started; waiting for `AssemblyZoneReady` events to drain
    /// `ctx.pending_assemblies` before transitioning to `Idle`.
    PreparingToStart,
    Idle,
    Driving,
    /// Driving in the dark without confirmed lighting (step 7 operational policy).
    DrivingDangerously,
    /// Speed > 160 km/h and RPM > 5500 sustained (see [`crate::vehicle_physics`]).
    ExtremeOperationWarning(Instant),
    /// Assemblies are being stopped; waiting for `AssemblyZoneReady` events to drain
    /// `ctx.pending_assemblies` before transitioning to `Off`.
    PreparingToStop,
}

/// Identity of a managed zone assembly.
///
/// Used by Phase-2+ messages to correlate zone replies with the originating assembly
/// without coupling the brain to zone-specific types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ZoneId {
    Headlamp,
}

/// Brain-synthesized facts (detectors). Ledger-visible; not assembly / wire ingress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operational {
    LightingUnsafe,
    /// All managed assemblies confirmed ready after a `StartAssemblies` barrier.
    AssembliesReady,
    /// All managed assemblies confirmed stopped after a `StopAssemblies` barrier.
    AssembliesStopped,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FsmEvent {
    PowerOn,
    PowerOff,
    UpdateRpm(u16),
    UpdateAmbientLux(u16),
    FrontHeadlampOnAck,
    FrontHeadlampOffAck,
    FrontHeadlampActuationIncomplete {
        direction: FrontHeadlampSwitchDirection,
        cause: FrontHeadlampIncompleteCause,
    },
    TimerTick,
    /// Brain-only hop (ADR-7): no `zone_turn`; table sets mode.
    Internal(Operational),
    /// An assembly zone has acknowledged a `BecomeOn` or `BecomeOff` tell.
    ///
    /// This is an *external* event — it arrives from an assembly actor mailbox via
    /// a drained [`crate::twin_runtime::turn_barrier::TurnBarrier`], exactly like
    /// `FrontHeadlampOnAck`.  The FSM transition table counts down
    /// `ctx.pending_assemblies` and transitions when the set becomes empty.
    AssemblyZoneReady(ZoneId),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FsmAction {
    StartBuzzer,
    StopBuzzer,
    LogWarning(String),
    PublishStateSync,
    /// Instruct the actor to start all managed assemblies (send `BecomeOn` barrier).
    StartAssemblies,
    /// Instruct the actor to stop all managed assemblies (send `BecomeOff` barrier).
    StopAssemblies,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorModeHintFromDomain {
    Normal,
    Transitioning,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DomainAction {
    StartBuzzer,
    StopBuzzer,
    PublishStateSync,
    LogWarning(String),
    RequestFrontHeadlampOn,
    RequestFrontHeadlampOff,
    /// Actor must start all managed assemblies (push startup `TurnBarrier`).
    /// Emitted on the `Off → PreparingToStart` transition.
    StartAssemblies,
    /// Actor must stop all managed assemblies (push shutdown `TurnBarrier`).
    /// Emitted on the `Idle → PreparingToStop` transition.
    StopAssemblies,
    EnterMode(ActorModeHintFromDomain),
}

impl From<&FsmState> for VehicleState {
    fn from(fsm: &FsmState) -> Self {
        match fsm {
            FsmState::Off => VehicleState::Off,
            FsmState::PreparingToStart => VehicleState::PreparingToStart,
            FsmState::Idle => VehicleState::Idle,
            FsmState::Driving => VehicleState::Driving,
            FsmState::DrivingDangerously => VehicleState::Critical,
            FsmState::ExtremeOperationWarning(_) => VehicleState::ExtremeOperationWarning,
            FsmState::PreparingToStop => VehicleState::PreparingToStop,
        }
    }
}
