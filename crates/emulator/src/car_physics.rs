use std::time::{SystemTime, UNIX_EPOCH};
use common::domain_types::RPM_IDLE;
use common::vehicle_physics::calculate_speed_from_rpm;

use crate::models::{AmbientRoadLightModel, PhysicalWorldModelConfig, RainModel, RpmModel};

pub struct PhysicalCar {
    rpm: u16,
    ambient_lux: u16,
    rain_detected: bool,
    rpm_model: RpmModel,
    ambient_road_light_model: AmbientRoadLightModel,
    rain_model: RainModel,
}

impl PhysicalCar {
    pub fn new() -> Self {
        Self::new_with_config(PhysicalWorldModelConfig::daytime_tunnel_profile())
    }

    pub fn new_with_config(cfg: PhysicalWorldModelConfig) -> Self {
        let rpm_model = RpmModel::new(cfg.rpm);
        let ambient_road_light_model = AmbientRoadLightModel::new(cfg.ambient_road_light);
        let rain_model = RainModel::new(cfg.rain);

        Self {
            rpm: RPM_IDLE,
            ambient_lux: 850,
            rain_detected: false,
            rpm_model,
            ambient_road_light_model,
            rain_model,
        }
    }

    pub fn rpm(&self) -> u16 {
        self.rpm
    }

    /// Kinematic ground speed derived from composite wheel RPM (for debug only; not published on CAN).
    pub fn derived_speed_kph(&self) -> f64 {
        calculate_speed_from_rpm(self.rpm)
    }

    pub fn ambient_lux(&self) -> u16 {
        self.ambient_lux
    }

    pub fn rain_detected(&self) -> bool {
        self.rain_detected
    }

    pub fn update(&mut self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        self.rpm = self.rpm_model.next_rpm(self.rpm, now);
        self.ambient_lux = self.ambient_road_light_model.next_ambient_lux(now);
        self.rain_detected = self.rain_model.next_rain_detected();

        let target_rpm = self.rpm_model.target_rpm_for_epoch(now);
        println!(
            "DEBUG: Time={}s | CompositeRPM={} (Target={}) | DerivedSpeedKph={:.2} | AmbientLux={} | Rain={}",
            now % 60,
            self.rpm,
            target_rpm,
            self.derived_speed_kph(),
            self.ambient_lux,
            self.rain_detected
        );
    }
}

#[cfg(test)]
mod tests {
    use super::PhysicalCar;
    use common::domain_types::{RPM_IDLE, RPM_REDLINE_THRESHOLD};
    use common::vehicle_physics::calculate_speed_from_rpm;

    #[test]
    fn smoke_new_car_starts_at_idle_rpm() {
        let car = PhysicalCar::new();
        assert_eq!(car.rpm(), RPM_IDLE);
        assert!((car.derived_speed_kph() - calculate_speed_from_rpm(RPM_IDLE)).abs() < f64::EPSILON);
        assert!((0..=1200).contains(&car.ambient_lux()));
    }

    #[test]
    fn smoke_update_keeps_values_within_expected_bounds() {
        let mut car = PhysicalCar::new();
        for _ in 0..32 {
            car.update();
            assert_eq!(car.derived_speed_kph(), calculate_speed_from_rpm(car.rpm()));
            assert!((RPM_IDLE..=RPM_REDLINE_THRESHOLD).contains(&car.rpm()));
            assert!((0..=1200).contains(&car.ambient_lux()));
        }
    }
}
