//! Health assembly: fuel, oil pressure, tyre status.
//!
//! No telemetry events feed it yet; it exposes the `is_healthy` predicate that
//! gates `PowerOn` in the operational FSM. Step 2: becomes a health child actor
//! (or stays a parent-owned struct, per the agreed topology).

#[derive(Debug, Clone, PartialEq)]
pub struct VehicleHealthContext {
    pub fuel_level_pct: u8,
    pub oil_pressure_kpa: u8,
    pub tyre_pressure_ok: bool,
}

impl Default for VehicleHealthContext {
    fn default() -> Self {
        Self {
            fuel_level_pct: 85,
            oil_pressure_kpa: 30,
            tyre_pressure_ok: true,
        }
    }
}

impl VehicleHealthContext {
    pub fn is_healthy(&self) -> bool {
        self.fuel_level_pct > 5 && self.oil_pressure_kpa > 10 && self.tyre_pressure_ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_healthy() {
        assert!(VehicleHealthContext::default().is_healthy());
    }

    #[test]
    fn low_fuel_is_unhealthy() {
        let h = VehicleHealthContext {
            fuel_level_pct: 5,
            ..VehicleHealthContext::default()
        };
        assert!(!h.is_healthy());
    }

    #[test]
    fn low_oil_is_unhealthy() {
        let h = VehicleHealthContext {
            oil_pressure_kpa: 10,
            ..VehicleHealthContext::default()
        };
        assert!(!h.is_healthy());
    }

    #[test]
    fn bad_tyre_is_unhealthy() {
        let h = VehicleHealthContext {
            tyre_pressure_ok: false,
            ..VehicleHealthContext::default()
        };
        assert!(!h.is_healthy());
    }
}
