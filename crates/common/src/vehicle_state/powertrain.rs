//! Powertrain zone (L1): alphabet + context + behavior.
//!
//! **ADR-5:** [`PowertrainState`] (same shape as [`PowertrainContext`] today),
//! [`PowertrainMessage`], [`PowertrainOutcome`].
//!
//! Self-sufficient: it receives a bus RPM reading, derives speed (via
//! [`crate::vehicle_physics::kinematics`]), and exposes raw + derived values and a local
//! mode. Nothing here performs I/O. In Step 2 this becomes `PowertrainActor`'s
//! local state + flat FSM.

use crate::vehicle_physics::constants::{operational_warning_active, RPM_IDLE, RPM_REDLINE_THRESHOLD};
use crate::vehicle_physics::kinematics::calculate_speed_from_rpm;

/// L1 powertrain snapshot (wheel RPM + derived speed).
pub type PowertrainState = PowertrainContext;

/// Inputs — future child-actor mailbox vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowertrainMessage {
    UpdateRpm(u16),
}

/// Zone-local egress; none in milestone 1 (operational FSM owns buzzer/sync).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowertrainOutcome {
    #[doc(hidden)]
    __NoEgress,
}

/// Per-wheel RPM. Today a single bus RPM is broadcast to all four wheels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WheelRpm {
    pub front_left: u16,
    pub front_right: u16,
    pub rear_left: u16,
    pub rear_right: u16,
}

impl WheelRpm {
    /// Same RPM on all four wheels (single-input assumption).
    pub fn uniform(rpm: u16) -> Self {
        Self {
            front_left: rpm,
            front_right: rpm,
            rear_left: rpm,
            rear_right: rpm,
        }
    }
}

/// Derived local powertrain mode.
///
/// Step 1: exposed for observability/tests only — **not** consumed by the
/// operational FSM (no behavior change). Step 2: this becomes the actor's
/// stored flat-FSM state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowertrainMode {
    Stalled,
    Rolling,
    Redline,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PowertrainContext {
    pub wheel_rpm: WheelRpm,
    /// Derived ground speed in km/h (from wheel RPM via [`crate::vehicle_physics::kinematics`]).
    pub speed_kph: u16,
}

impl Default for PowertrainContext {
    fn default() -> Self {
        Self {
            wheel_rpm: WheelRpm::uniform(0),
            speed_kph: 0,
        }
    }
}

impl PowertrainContext {
    /// Apply one bus RPM reading to all four wheels (single-input assumption today).
    pub fn apply_rpm(&mut self, rpm: u16) {
        self.wheel_rpm = WheelRpm::uniform(rpm);
    }

    /// Recompute derived speed (km/h) from current wheel RPM.
    pub fn refresh_speed(&mut self) {
        let kph = calculate_speed_from_rpm(self.primary_rpm()).round();
        self.speed_kph = kph.min(f64::from(u16::MAX)) as u16;
    }

    /// Ignition off: kinematic speed is not meaningful; hold standstill for invariants.
    pub fn freeze_standstill(&mut self) {
        self.speed_kph = 0;
    }

    /// Representative RPM for control decisions (front-left under the uniform model).
    pub fn primary_rpm(&self) -> u16 {
        self.wheel_rpm.front_left
    }

    pub fn is_stationary(&self) -> bool {
        self.speed_kph == 0
    }

    /// Operational-warning predicate over this assembly's own fields.
    pub fn is_operational_warning_active(&self) -> bool {
        operational_warning_active(self.primary_rpm(), self.speed_kph)
    }

    /// Derived local mode from current RPM (Step 1: exposed only).
    pub fn mode(&self) -> PowertrainMode {
        let rpm = self.primary_rpm();
        if rpm < RPM_IDLE {
            PowertrainMode::Stalled
        } else if rpm >= RPM_REDLINE_THRESHOLD {
            PowertrainMode::Redline
        } else {
            PowertrainMode::Rolling
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_rpm_broadcasts_to_all_wheels() {
        let mut pt = PowertrainContext::default();
        pt.apply_rpm(1500);
        assert_eq!(pt.wheel_rpm, WheelRpm::uniform(1500));
        assert_eq!(pt.primary_rpm(), 1500);
    }

    #[test]
    fn refresh_speed_derives_from_rpm() {
        let mut pt = PowertrainContext::default();
        pt.apply_rpm(3000);
        pt.refresh_speed();
        assert_eq!(pt.speed_kph, 342);
    }

    #[test]
    fn freeze_standstill_zeroes_speed_but_keeps_rpm() {
        let mut pt = PowertrainContext::default();
        pt.apply_rpm(3000);
        pt.refresh_speed();
        pt.freeze_standstill();
        assert_eq!(pt.speed_kph, 0);
        assert_eq!(pt.primary_rpm(), 3000);
    }

    #[test]
    fn mode_is_stalled_below_idle() {
        let mut pt = PowertrainContext::default();
        pt.apply_rpm(RPM_IDLE - 1);
        assert_eq!(pt.mode(), PowertrainMode::Stalled);
    }

    #[test]
    fn mode_is_rolling_in_band() {
        let mut pt = PowertrainContext::default();
        pt.apply_rpm(RPM_IDLE);
        assert_eq!(pt.mode(), PowertrainMode::Rolling);
        pt.apply_rpm(RPM_REDLINE_THRESHOLD - 1);
        assert_eq!(pt.mode(), PowertrainMode::Rolling);
    }

    #[test]
    fn mode_is_redline_at_threshold() {
        let mut pt = PowertrainContext::default();
        pt.apply_rpm(RPM_REDLINE_THRESHOLD);
        assert_eq!(pt.mode(), PowertrainMode::Redline);
    }
}
