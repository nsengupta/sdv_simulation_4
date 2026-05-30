//! World-facing, serializable projection of a transition record.
//!
//! The pure FSM core measures time with [`std::time::Instant`] — monotonic, process-local, and
//! deliberately **not** serializable (it has no defined zero). For anything that leaves the
//! process — a file, a wire, an offline verifier — every `Instant` is projected to a
//! [`Duration`] since [`UNIX_EPOCH`], anchored once per session by a [`SessionEpoch`].
//!
//! Design contract (see `docs/design-notes-runtime-observation.md`, item "(1)"):
//! - **Permanence of `Instant` inside:** [`crate::fsm::FsmState`],
//!   [`crate::fsm::VehicleContext`], and [`crate::fsm::RawTransitionRecord`] stay `Instant`-bearing
//!   and serde-free. Nothing here mutates the functional core.
//! - **Duration for the world:** this module owns the full, lossless mirror of those types with
//!   each `Instant` replaced by a wall-clock `Duration` since `UNIX_EPOCH`, plus all serde.
//!
//! Ordering for offline folding is `record_seq` (clock-independent); `at_unix` answers
//! *how long between transitions*; `session_epoch_unix_nanos` says *which run*.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::fsm::{
    ActorModeHintFromDomain, DomainAction, FrontHeadlampIncompleteCause,
    FrontHeadlampSwitchDirection, FsmEvent, FsmState, HeadlampContext, LightingState,
    PowertrainContext, RawTransitionRecord, VehicleContext, VehicleHealthContext,
    VisibilityContext, WheelRpm,
};

/// Per-session correlation between the monotonic clock and the wall clock.
///
/// Captured once at actor start. `started_at_instant` is the monotonic anchor used to *measure*
/// elapsed time; `started_at_unix` is the wall-clock placement of that same anchor. Any later
/// monotonic instant `t` projects to a wall stamp as `started_at_unix + (t - started_at_instant)`.
#[derive(Debug, Clone, Copy)]
pub struct SessionEpoch {
    started_at_instant: Instant,
    started_at_unix: Duration,
}

impl SessionEpoch {
    /// Capture the (monotonic, wall) anchor pair now. Reads the wall clock exactly once.
    pub fn capture() -> Self {
        Self {
            started_at_instant: Instant::now(),
            started_at_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default(),
        }
    }

    /// Project a monotonic instant to a wall-clock [`Duration`] since `UNIX_EPOCH`.
    ///
    /// `saturating_duration_since` guards the (not-expected) case of an instant before the
    /// anchor, yielding the anchor's own wall stamp rather than underflowing.
    pub fn project(&self, t: Instant) -> Duration {
        self.started_at_unix + t.saturating_duration_since(self.started_at_instant)
    }

    /// Stable identifier of this run: the session start as nanoseconds since `UNIX_EPOCH`.
    pub fn session_id_nanos(&self) -> u128 {
        self.started_at_unix.as_nanos()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublishedLightingState {
    Off,
    OnRequested,
    On,
    OffRequested,
}

impl From<&LightingState> for PublishedLightingState {
    fn from(s: &LightingState) -> Self {
        match s {
            LightingState::Off => Self::Off,
            LightingState::OnRequested => Self::OnRequested,
            LightingState::On => Self::On,
            LightingState::OffRequested => Self::OffRequested,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublishedFrontHeadlampSwitchDirection {
    On,
    Off,
}

impl From<&FrontHeadlampSwitchDirection> for PublishedFrontHeadlampSwitchDirection {
    fn from(d: &FrontHeadlampSwitchDirection) -> Self {
        match d {
            FrontHeadlampSwitchDirection::On => Self::On,
            FrontHeadlampSwitchDirection::Off => Self::Off,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublishedFrontHeadlampIncompleteCause {
    TimedOut,
    NegativeAck,
}

impl From<&FrontHeadlampIncompleteCause> for PublishedFrontHeadlampIncompleteCause {
    fn from(c: &FrontHeadlampIncompleteCause) -> Self {
        match c {
            FrontHeadlampIncompleteCause::TimedOut => Self::TimedOut,
            FrontHeadlampIncompleteCause::NegativeAck => Self::NegativeAck,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublishedFsmEvent {
    PowerOn,
    PowerOff,
    UpdateRpm(u16),
    UpdateAmbientLux(u16),
    FrontHeadlampOnAck,
    FrontHeadlampOffAck,
    FrontHeadlampActuationIncomplete {
        direction: PublishedFrontHeadlampSwitchDirection,
        cause: PublishedFrontHeadlampIncompleteCause,
    },
    TimerTick,
}

impl From<&FsmEvent> for PublishedFsmEvent {
    fn from(e: &FsmEvent) -> Self {
        match e {
            FsmEvent::PowerOn => Self::PowerOn,
            FsmEvent::PowerOff => Self::PowerOff,
            FsmEvent::UpdateRpm(rpm) => Self::UpdateRpm(*rpm),
            FsmEvent::UpdateAmbientLux(lux) => Self::UpdateAmbientLux(*lux),
            FsmEvent::FrontHeadlampOnAck => Self::FrontHeadlampOnAck,
            FsmEvent::FrontHeadlampOffAck => Self::FrontHeadlampOffAck,
            FsmEvent::FrontHeadlampActuationIncomplete { direction, cause } => {
                Self::FrontHeadlampActuationIncomplete {
                    direction: direction.into(),
                    cause: cause.into(),
                }
            }
            FsmEvent::TimerTick => Self::TimerTick,
        }
    }
}

/// World-facing domain intents. Mirrors [`DomainAction`] **minus** `EnterMode`, which is a runtime
/// control hint (not a domain intent) and is already excluded from the recorded action list (WI-1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublishedDomainAction {
    StartBuzzer,
    StopBuzzer,
    PublishStateSync,
    LogWarning(String),
    RequestFrontHeadlampOn,
    RequestFrontHeadlampOff,
}

impl PublishedDomainAction {
    /// Project a domain action, dropping the non-domain `EnterMode` control hint.
    fn project(action: &DomainAction) -> Option<Self> {
        match action {
            DomainAction::StartBuzzer => Some(Self::StartBuzzer),
            DomainAction::StopBuzzer => Some(Self::StopBuzzer),
            DomainAction::PublishStateSync => Some(Self::PublishStateSync),
            DomainAction::LogWarning(msg) => Some(Self::LogWarning(msg.clone())),
            DomainAction::RequestFrontHeadlampOn => Some(Self::RequestFrontHeadlampOn),
            DomainAction::RequestFrontHeadlampOff => Some(Self::RequestFrontHeadlampOff),
            DomainAction::EnterMode(ActorModeHintFromDomain::Normal)
            | DomainAction::EnterMode(ActorModeHintFromDomain::Transitioning) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublishedFsmState {
    Off,
    Idle,
    Driving,
    /// The monotonic warning anchor projected to wall-clock placement.
    ExtremeOperationWarning {
        began_at_unix: Duration,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedWheelRpm {
    pub front_left: u16,
    pub front_right: u16,
    pub rear_left: u16,
    pub rear_right: u16,
}

impl From<&WheelRpm> for PublishedWheelRpm {
    fn from(w: &WheelRpm) -> Self {
        Self {
            front_left: w.front_left,
            front_right: w.front_right,
            rear_left: w.rear_left,
            rear_right: w.rear_right,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedPowertrainContext {
    pub wheel_rpm: PublishedWheelRpm,
    pub speed_kph: u16,
}

impl From<&PowertrainContext> for PublishedPowertrainContext {
    fn from(p: &PowertrainContext) -> Self {
        Self {
            wheel_rpm: (&p.wheel_rpm).into(),
            speed_kph: p.speed_kph,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedHealthContext {
    pub fuel_level_pct: u8,
    pub oil_pressure_kpa: u8,
    pub tyre_pressure_ok: bool,
}

impl From<&VehicleHealthContext> for PublishedHealthContext {
    fn from(h: &VehicleHealthContext) -> Self {
        Self {
            fuel_level_pct: h.fuel_level_pct,
            oil_pressure_kpa: h.oil_pressure_kpa,
            tyre_pressure_ok: h.tyre_pressure_ok,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedVisibilityContext {
    pub ambient_lux: u16,
}

impl From<&VisibilityContext> for PublishedVisibilityContext {
    fn from(v: &VisibilityContext) -> Self {
        Self {
            ambient_lux: v.ambient_lux,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedHeadlampContext {
    pub state: PublishedLightingState,
    /// The monotonic ACK-wait anchor projected to wall-clock placement, if pending.
    pub ack_pending_since_unix: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedVehicleContext {
    pub powertrain: PublishedPowertrainContext,
    pub health: PublishedHealthContext,
    pub visibility: PublishedVisibilityContext,
    pub headlamp: PublishedHeadlampContext,
}

/// The serializable, `Instant`-free transition record emitted "to the world".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublishedTransitionRecord {
    pub car_identity: String,
    /// Which run produced this record (session start, nanoseconds since `UNIX_EPOCH`).
    pub session_epoch_unix_nanos: u128,
    /// Monotonic, clock-independent ledger order (Counter A).
    pub record_seq: u64,
    /// When this transition occurred, as a `Duration` since `UNIX_EPOCH`.
    pub at_unix: Duration,
    pub event: PublishedFsmEvent,
    pub old_state: PublishedFsmState,
    pub next_state: PublishedFsmState,
    pub old_ctx: PublishedVehicleContext,
    pub current_ctx: PublishedVehicleContext,
    pub actions: Vec<PublishedDomainAction>,
}

impl SessionEpoch {
    fn fsm_state(&self, state: &FsmState) -> PublishedFsmState {
        match state {
            FsmState::Off => PublishedFsmState::Off,
            FsmState::Idle => PublishedFsmState::Idle,
            FsmState::Driving => PublishedFsmState::Driving,
            FsmState::ExtremeOperationWarning(at) => PublishedFsmState::ExtremeOperationWarning {
                began_at_unix: self.project(*at),
            },
        }
    }

    fn headlamp(&self, h: &HeadlampContext) -> PublishedHeadlampContext {
        PublishedHeadlampContext {
            state: (&h.state).into(),
            ack_pending_since_unix: h.ack_pending_since.map(|t| self.project(t)),
        }
    }

    fn vehicle_context(&self, ctx: &VehicleContext) -> PublishedVehicleContext {
        PublishedVehicleContext {
            powertrain: (&ctx.powertrain).into(),
            health: (&ctx.health).into(),
            visibility: (&ctx.visibility).into(),
            headlamp: self.headlamp(&ctx.headlamp),
        }
    }
}

impl PublishedTransitionRecord {
    /// Project a pure [`RawTransitionRecord`] into its serializable, wall-clock-stamped form.
    ///
    /// This is the sole point that consumes the [`SessionEpoch`]; every `Instant` in the raw
    /// record (the timestamp, the warning anchor in either state, the headlamp ACK anchor in
    /// either context) becomes a `Duration` since `UNIX_EPOCH`.
    pub fn project(
        raw: &RawTransitionRecord,
        car_identity: &str,
        record_seq: u64,
        epoch: &SessionEpoch,
    ) -> Self {
        Self {
            car_identity: car_identity.to_owned(),
            session_epoch_unix_nanos: epoch.session_id_nanos(),
            record_seq,
            at_unix: epoch.project(raw.at),
            event: (&raw.event).into(),
            old_state: epoch.fsm_state(&raw.old_state),
            next_state: epoch.fsm_state(&raw.next_state),
            old_ctx: epoch.vehicle_context(&raw.old_ctx),
            current_ctx: epoch.vehicle_context(&raw.current_ctx),
            actions: raw
                .actions
                .iter()
                .filter_map(PublishedDomainAction::project)
                .collect(),
        }
    }
}
