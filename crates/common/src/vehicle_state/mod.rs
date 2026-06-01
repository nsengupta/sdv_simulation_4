//! L1 vehicle state: per-assembly (zone) contexts and the aggregate [`VehicleContext`].
//!
//! Each assembly owns its own data **and** the behavior over that data (receive,
//! derive, expose). [`VehicleContext`] is only an aggregate of the assemblies.
//!
//! Independent of the FSM pattern — consumed by `fsm::step`, held by `DigitalTwinCar`.
//! Step 2 groundwork for the zone-actor plan (ADR 0001).

pub mod front_headlamp;
pub mod health;
pub mod powertrain;
pub mod visibility;

pub use front_headlamp::HeadlampContext;
pub use health::VehicleHealthContext;
pub use powertrain::{PowertrainContext, PowertrainMode, WheelRpm};
pub use visibility::VisibilityContext;

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
