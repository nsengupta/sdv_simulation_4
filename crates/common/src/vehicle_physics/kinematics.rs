//! Wheel RPM → ground speed (kinematic expectation for the digital twin).
//!
//! Pure math only: no FSM or [`VehicleContext`] types. Callers (e.g. the powertrain
//! assembly) invoke [`calculate_speed_from_rpm`] and store the result in their own state.

/// Combined multiplier for `(2 * π * radius * 3.6) / 60` with tire radius ~0.303 m.
const RPM_TO_KMH_MULTIPLIER: f64 = 0.114;

/// Convert a single wheel's RPM to ground speed (km/h).
///
/// Assumes perfect traction and a standard tire radius (~0.303 m). Returns the uncapped
/// kinematic value (no artificial 255 km/h limit). The powertrain assembly rounds and
/// stores this in `speed_kph`.
///
/// When observed-speed ECUs exist (slip, clutch, gear), they can supply a separate measurement;
/// compare against this kinematic expectation in policy / invariants.
pub fn calculate_speed_from_rpm(rpm: u16) -> f64 {
    let speed_kmh = f64::from(rpm) * RPM_TO_KMH_MULTIPLIER;
    speed_kmh.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_rpm_is_zero_kph() {
        assert_eq!(calculate_speed_from_rpm(0), 0.0);
    }

    #[test]
    fn known_rpm_maps_to_expected_kph() {
        assert!((calculate_speed_from_rpm(1000) - 114.0).abs() < f64::EPSILON);
    }

    #[test]
    fn speed_is_monotonic_in_rpm() {
        assert!(calculate_speed_from_rpm(1500) > calculate_speed_from_rpm(500));
        assert!(calculate_speed_from_rpm(3000) > calculate_speed_from_rpm(1500));
    }

    #[test]
    fn high_rpm_is_not_capped_at_255_kph() {
        assert!((calculate_speed_from_rpm(3000) - 342.0).abs() < f64::EPSILON);
    }
}
