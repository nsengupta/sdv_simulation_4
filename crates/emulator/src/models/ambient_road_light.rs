use rand::RngExt;

use super::config::AmbientRoadLightModelConfig;

#[derive(Debug, Clone)]
pub struct AmbientRoadLightModel {
    cfg: AmbientRoadLightModelConfig,
    tunnel_ticks_remaining: u16,
}

impl AmbientRoadLightModel {
    pub fn new(cfg: AmbientRoadLightModelConfig) -> Self {
        Self {
            cfg,
            tunnel_ticks_remaining: 0,
        }
    }

    /// Current milestone assumption:
    /// - Baseline is daytime driving.
    /// - Lower light mostly comes from occasional tunnel travel.
    /// - A full day/night waveform is deferred.
    pub fn next_ambient_lux(&mut self, _epoch_secs: u64) -> u16 {
        let mut rng = rand::rng();

        if !self.in_tunnel() {
            let is_about_to_enter_a_tunnel = rng.random_bool(self.cfg.tunnel_event_probability_per_tick as f64);
            if is_about_to_enter_a_tunnel {
                self.tunnel_ticks_remaining = rng
                    .random_range(self.cfg.tunnel_duration_ticks_min..=self.cfg.tunnel_duration_ticks_max);
            }
        }

        let jitter = rng.random_range(
            -(self.cfg.jitter_amplitude_lux as i32)..=(self.cfg.jitter_amplitude_lux as i32),
        );
        let mut lux = self.cfg.baseline_daylight_lux as i32 + jitter;

        if self.in_tunnel() {
            lux -= self.cfg.tunnel_lux_drop as i32;
            self.tunnel_ticks_remaining = self.tunnel_ticks_remaining.saturating_sub(1);
        }

        lux.clamp(self.cfg.min_lux as i32, self.cfg.max_lux as i32) as u16
    }

    pub fn in_tunnel(&self) -> bool {
        self.tunnel_ticks_remaining > 0
    }
}
