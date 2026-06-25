use common::domain_types::{
    RPM_EXTREME_OPERATION_THRESHOLD, RPM_IDLE, RPM_REDLINE_THRESHOLD,
};

#[derive(Debug, Clone)]
pub struct SpeedModelConfig {
    pub min_kph: f64,
    pub max_kph: f64,
    pub random_nudge_min: f64,
    pub random_nudge_max: f64,
}

#[derive(Debug, Clone)]
pub struct RpmModelConfig {
    pub idle_rpm: u16,
    pub extreme_operation_rpm: u16,
    pub redline_rpm: u16,
    pub high_target_rpm: f32,
    pub low_target_rpm: f32,
    pub target_flip_period_secs: u64,
    pub proportional_gain: f32,
    pub jitter_amplitude: f32,
}

#[derive(Debug, Clone)]
pub struct AmbientRoadLightModelConfig {
    pub min_lux: u16,
    pub max_lux: u16,
    pub baseline_daylight_lux: u16,
    pub jitter_amplitude_lux: i16,
    pub tunnel_event_probability_per_tick: f32,
    pub tunnel_lux_drop: u16,
    pub tunnel_duration_ticks_min: u16,
    pub tunnel_duration_ticks_max: u16,
    pub cycle_secs: u64,
}

#[derive(Debug, Clone)]
pub struct RainModelConfig {
    /// Per 100 ms tick, probability of rain starting when currently dry.
    pub rain_event_probability_per_tick: f32,
    pub rain_duration_ticks_min: u16,
    pub rain_duration_ticks_max: u16,
}

#[derive(Debug, Clone)]
pub struct PhysicalWorldModelConfig {
    pub speed: SpeedModelConfig,
    pub rpm: RpmModelConfig,
    pub ambient_road_light: AmbientRoadLightModelConfig,
    pub rain: RainModelConfig,
}

impl PhysicalWorldModelConfig {
    pub fn daytime_tunnel_profile() -> Self {
        Self {
            speed: SpeedModelConfig {
                min_kph: 0.0,
                max_kph: 160.0,
                random_nudge_min: -0.5,
                random_nudge_max: 0.6,
            },
            rpm: RpmModelConfig {
                idle_rpm: RPM_IDLE,
                extreme_operation_rpm: RPM_EXTREME_OPERATION_THRESHOLD,
                redline_rpm: RPM_REDLINE_THRESHOLD,
                high_target_rpm: 6500.0,
                low_target_rpm: 1200.0,
                target_flip_period_secs: 15,
                proportional_gain: 0.1,
                jitter_amplitude: 5.0,
            },
            ambient_road_light: AmbientRoadLightModelConfig {
                min_lux: 0,
                max_lux: 1200,
                baseline_daylight_lux: 850,
                // ±35 → ~815–885 lux; crosses LUX_ON (840) / LUX_OFF (860) for headlamp demo cycles.
                jitter_amplitude_lux: 35,
                // Default ≈ a tunnel every ~10 s (demo-friendly); override at startup with
                // `EMULATOR_TUNNEL_PROB` (e.g. 0.002 for infrequent tunnels). See `main.rs`.
                tunnel_event_probability_per_tick: 0.01,
                tunnel_lux_drop: 900,
                tunnel_duration_ticks_min: 20,
                tunnel_duration_ticks_max: 80,
                // TODO(profile-injection): accept full handcrafted profile selection at startup
                // (test/demo/realistic). Today only `tunnel_event_probability_per_tick` is
                // env-overridable (`EMULATOR_TUNNEL_PROB`); the rest of the profile is fixed.
                cycle_secs: 90,
            },
            rain: RainModelConfig {
                // Default ≈ rain every ~12 s when dry (demo-friendly); override at startup with
                // `EMULATOR_RAIN_PROB` (e.g. 0.002 for infrequent rain). See `main.rs`.
                rain_event_probability_per_tick: 0.008,
                rain_duration_ticks_min: 30,
                rain_duration_ticks_max: 60,
            },
        }
    }
}
