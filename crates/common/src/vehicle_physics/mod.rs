//! L0 foundation: vehicle calibration constants and pure kinematic formulas.
//!
//! Depends on std only. No FSM, runtime, or I/O types.

pub mod constants;
pub mod kinematics;

pub use constants::{
    extreme_operation_active, operational_warning_active, speed_threshold_exceeded,
    EXTREME_OPERATION_WARNING_MESSAGE, FRONT_HEADLAMP_OFF_ACK_WAIT, FRONT_HEADLAMP_ON_ACK_WAIT,
    LUX_OFF_THRESHOLD, LUX_ON_THRESHOLD, RPM_DRIVING_THRESHOLD, RPM_EXTREME_OPERATION_THRESHOLD,
    RPM_IDLE, RPM_REDLINE_THRESHOLD, RPM_STRESS_DURATION_THRESHOLD_SECS,
    SPEED_EXTREME_OPERATION_THRESHOLD_KPH, SPEED_THRESHOLD_WARNING_MESSAGE,
};
pub use kinematics::calculate_speed_from_rpm;
