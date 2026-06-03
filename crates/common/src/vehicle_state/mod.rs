//! L1 vehicle state: per-zone alphabet (ADR-5), contexts, and [`VehicleContext`].
//!
//! Each zone exposes `{Zone}State`, `{Zone}Message`, `{Zone}Outcome` where applicable.
//! Zones import L0 only — no L2/L4. Consumed by `fsm::step` and held by `DigitalTwinCar`.

pub mod front_headlamp;
pub mod health;
pub mod powertrain;
pub mod visibility;

pub use front_headlamp::{
    FrontHeadlampIncompleteCause, FrontHeadlampSwitchDirection, HeadlampContext, HeadlampMessage,
    HeadlampOutcome, HeadlampState,
};
pub use health::{HealthState, VehicleHealthContext};
pub use powertrain::{
    PowertrainContext, PowertrainMessage, PowertrainMode, PowertrainOutcome, PowertrainState,
    WheelRpm,
};
pub use visibility::{VisibilityContext, VisibilityMessage, VisibilityOutcome, VisibilityState};

/// Aggregate of all vehicle assemblies held by the digital twin.
///
/// Fields stay public for now so existing call sites keep compiling; behavior
/// lives on the per-assembly types, not here. Each assembly carries its own
/// `Default`, so the aggregate derives it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VehicleContext {
    pub powertrain: PowertrainContext,
    pub health: VehicleHealthContext,
    pub visibility: VisibilityContext,
    pub headlamp: HeadlampContext,
}

impl VehicleContext {
    /// Thin delegate retained for Step 1 so existing callers stay unchanged.
    /// Inline-remove in Step 2 in favor of `health.is_healthy()`.
    pub fn is_healthy(&self) -> bool {
        self.health.is_healthy()
    }
}
