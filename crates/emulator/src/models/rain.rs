use rand::RngExt;

use super::config::RainModelConfig;

#[derive(Debug, Clone)]
pub struct RainModel {
    cfg: RainModelConfig,
    rain_ticks_remaining: u16,
}

impl RainModel {
    pub fn new(cfg: RainModelConfig) -> Self {
        Self {
            cfg,
            rain_ticks_remaining: 0,
        }
    }

    /// Advance one 100 ms tick; returns whether the rain sensor reads wet.
    pub fn next_rain_detected(&mut self) -> bool {
        let mut rng = rand::rng();

        if !self.raining() {
            let starts = rng.random_bool(self.cfg.rain_event_probability_per_tick as f64);
            if starts {
                self.rain_ticks_remaining = rng
                    .random_range(self.cfg.rain_duration_ticks_min..=self.cfg.rain_duration_ticks_max);
            }
        }

        let detected = self.raining();
        if detected {
            self.rain_ticks_remaining = self.rain_ticks_remaining.saturating_sub(1);
        }
        detected
    }

    pub fn raining(&self) -> bool {
        self.rain_ticks_remaining > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn given_zero_rain_probability_when_ticks_then_never_rains() {
        let mut model = RainModel::new(RainModelConfig {
            rain_event_probability_per_tick: 0.0,
            rain_duration_ticks_min: 10,
            rain_duration_ticks_max: 10,
        });
        for _ in 0..200 {
            assert!(!model.next_rain_detected());
        }
    }
}
