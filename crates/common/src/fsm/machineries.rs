//! L2 operational vocabulary: mode, ledger events, and domain actions.
//!
//! Zone snapshots and alphabets live in [`crate::vehicle_state`]. Headlamp ingress
//! direction/cause types are re-exported here for [`FsmEvent`] only.

use crate::domain_types::VehicleState;
use std::collections::BTreeSet;
use std::time::Instant;

pub use crate::vehicle_state::{FrontHeadlampIncompleteCause, FrontHeadlampSwitchDirection};

/// All assembly IDs the brain coordinates.
///
/// Single source of truth for assembly topology.  Used to seed the initial
/// `BTreeSet` inside `PreparingToStart` / `PreparingToStop` on the entry transitions
/// and to populate `StartAssemblies` / `StopAssemblies` action payloads.
pub(crate) const ALL_ASSEMBLIES: &[AssemblyId] = &[AssemblyId::Headlamp, AssemblyId::Wiper];

#[derive(Debug, Clone, PartialEq)]
pub enum FsmState {
    Off,
    /// Assemblies are being started.
    ///
    /// The inner `BTreeSet` holds the assembly IDs that have **not yet** acknowledged
    /// startup (i.e., have not sent `AssemblyZoneReady`).  Each acknowledgement shrinks
    /// the set; when it becomes empty the FSM transitions to `Idle`.
    ///
    /// The set is the authoritative countdown — `VehicleContext` carries no separate
    /// `remaining_assemblies` field.
    PreparingToStart(BTreeSet<AssemblyId>),
    Idle,
    Driving,
    /// Driving in the dark without confirmed lighting (step 7 operational policy).
    DrivingDangerously,
    /// Speed > 160 km/h and RPM > 5500 sustained (see [`crate::vehicle_physics`]).
    ExtremeOperationWarning(Instant),
    /// Assemblies are being stopped.
    ///
    /// Mirrors [`FsmState::PreparingToStart`]: the inner set holds assemblies that have
    /// not yet acknowledged shutdown.  Empty set → `Off`.
    PreparingToStop(BTreeSet<AssemblyId>),
}

/// Identity of a managed assembly zone.
///
/// Used by Phase-2+ messages to correlate zone replies with the originating assembly
/// without coupling the brain to zone-specific types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AssemblyId {
    Headlamp,
    /// Phase-7 assembly: windshield wiper.
    Wiper,
}

/// Brain-synthesized facts (detectors). Ledger-visible; not assembly / wire ingress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operational {
    LightingUnsafe,
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
    /// `ctx.remaining_assemblies` and transitions when the set becomes empty.
    AssemblyZoneReady(AssemblyId),
    /// Rain has started falling on the windshield.  Binary fact — no intensity payload.
    /// Routes to the wiper zone via `zone_message_for_event`; FSM self-loops.
    RainsStarted,
    /// Rain has stopped.  Complement of [`Self::RainsStarted`].
    RainsStopped,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FsmAction {
    StartBuzzer,
    StopBuzzer,
    LogWarning(String),
    PublishStateSync,
    /// Instruct the actor to start the listed assemblies (send `BecomeOn` barrier).
    StartAssemblies(Vec<AssemblyId>),
    /// Instruct the actor to stop the listed assemblies (send `BecomeOff` barrier).
    StopAssemblies(Vec<AssemblyId>),
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DomainAction {
    StartBuzzer,
    StopBuzzer,
    PublishStateSync,
    LogWarning(String),
    RequestFrontHeadlampOn,
    RequestFrontHeadlampOff,
    /// Instruct the wiper actuator to start wiping.
    RequestWiperStart,
    /// Instruct the wiper actuator to stop wiping.
    RequestWiperStop,
    /// Actor must start the listed assemblies (push startup `TurnBarrier`).
    /// Emitted on the `Off → PreparingToStart` transition.
    StartAssemblies(Vec<AssemblyId>),
    /// Actor must stop the listed assemblies (push shutdown `TurnBarrier`).
    /// Emitted on the `Idle → PreparingToStop` transition.
    StopAssemblies(Vec<AssemblyId>),
}

impl From<&FsmState> for VehicleState {
    fn from(fsm: &FsmState) -> Self {
        match fsm {
            FsmState::Off => VehicleState::Off,
            FsmState::PreparingToStart(_) => VehicleState::PreparingToStart,
            FsmState::Idle => VehicleState::Idle,
            FsmState::Driving => VehicleState::Driving,
            FsmState::DrivingDangerously => VehicleState::Critical,
            FsmState::ExtremeOperationWarning(_) => VehicleState::ExtremeOperationWarning,
            FsmState::PreparingToStop(_) => VehicleState::PreparingToStop,
        }
    }
}
