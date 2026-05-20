//! Vehicle-wide calibration constants shared across FSM/domain logic.
//!
//! Keep only globally meaningful tuning values here. Module-local implementation
//! constants should remain in their owning modules.

use std::time::Duration;

pub const RPM_IDLE: u16 = 800;
pub const RPM_REDLINE_THRESHOLD: u16 = 7000;

pub const RPM_STRESS_DURATION_THRESHOLD_SECS: u64 = 5;

/// RPM above this while speed exceeds [`SPEED_EXTREME_OPERATION_THRESHOLD_KPH`] triggers
/// [`crate::fsm::FsmState::ExtremeOperationWarning`] (approaches redline / thermal stress).
pub const RPM_EXTREME_OPERATION_THRESHOLD: u16 = 5500;

/// Ground speed (km/h) above this while RPM exceeds [`RPM_EXTREME_OPERATION_THRESHOLD`] triggers
/// extreme-operation warning (unusual for commuter traffic).
pub const SPEED_EXTREME_OPERATION_THRESHOLD_KPH: u16 = 160;

/// Log text when derived speed alone exceeds the commuter threshold.
pub const SPEED_THRESHOLD_WARNING_MESSAGE: &str =
    "SpeedThresholdExceeded: ground speed > 160 km/h";

/// Log text when both speed and RPM indicate sustained extreme operation.
pub const EXTREME_OPERATION_WARNING_MESSAGE: &str =
    "ExtremeOperationWarning: speed > 160 km/h and RPM > 5500 (redline / thermal or oil stress risk)";

/// Request headlamp ON when ambient lux falls to or below this value (dim / tunnel).
/// Pair with [`LUX_OFF_THRESHOLD`] and emulator jitter (~815–885 lux) for demo ON/OFF cycles.
pub const LUX_ON_THRESHOLD: u16 = 840;

/// Request headlamp OFF when ambient lux rises to or above this value (bright).
/// Deadband: lux in `(LUX_ON_THRESHOLD, LUX_OFF_THRESHOLD)` holds current lighting state.
pub const LUX_OFF_THRESHOLD: u16 = 860;

/// Maximum time to wait for an ON command ACK before timeout recovery.
pub const FRONT_HEADLAMP_ON_ACK_WAIT: Duration = Duration::from_secs(2);
/// Maximum time to wait for an OFF command ACK before timeout recovery.
pub const FRONT_HEADLAMP_OFF_ACK_WAIT: Duration = Duration::from_secs(2);

#[inline]
pub fn speed_threshold_exceeded(speed_kph: u16) -> bool {
    speed_kph > SPEED_EXTREME_OPERATION_THRESHOLD_KPH
}

/// Sustained high RPM near redline while already above the speed threshold (AND).
#[inline]
pub fn extreme_operation_active(rpm: u16, speed_kph: u16) -> bool {
    rpm > RPM_EXTREME_OPERATION_THRESHOLD && speed_threshold_exceeded(speed_kph)
}

/// Enter operational warning when speed alone or full extreme-operation criteria are met (OR).
#[inline]
pub fn operational_warning_active(rpm: u16, speed_kph: u16) -> bool {
    speed_threshold_exceeded(speed_kph) || extreme_operation_active(rpm, speed_kph)
}
