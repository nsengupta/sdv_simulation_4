//! Phase 2 generator — the "Virtual ECU" (composite wheel RPM + ambient lux on CAN).

pub mod car_physics;
pub mod models;

use anyhow::Result;
use car_physics::PhysicalCar;
use common::VssSignal;
use models::PhysicalWorldModelConfig;
use socketcan::{CanSocket, Socket};
use std::{thread, time::Duration};

/// Override for the per-tick probability of *entering* a tunnel (low lux → headlamp ON).
///
/// A "tick" is one 100 ms publish loop, so probability `p` ≈ one tunnel every `1/(p·10)` seconds
/// while not already in one. Must be a float in `0.0..=1.0`; unset → the profile default `0.01`
/// (≈ a tunnel every ~10 s — frequent, good for demos). For **infrequent** tunnels try
/// `EMULATOR_TUNNEL_PROB=0.002` (≈ every ~50 s) or `0.001` (≈ every ~100 s).
const ENV_TUNNEL_PROB: &str = "EMULATOR_TUNNEL_PROB";

/// Override for the per-tick probability of *entering* rain when dry.
///
/// A "tick" is one 100 ms publish loop, so probability `p` ≈ one rain event every `1/(p·10)`
/// seconds while not already raining. Must be a float in `0.0..=1.0`; unset → the profile
/// default `0.008` (≈ rain every ~12 s). For **infrequent** rain try `EMULATOR_RAIN_PROB=0.002`
/// (≈ every ~50 s) or `0.0` to disable rain entirely.
const ENV_RAIN_PROB: &str = "EMULATOR_RAIN_PROB";

fn parse_rain_prob_env() -> Option<f32> {
    let raw = std::env::var(ENV_RAIN_PROB).ok()?;
    match raw.trim().parse::<f32>() {
        Ok(p) if (0.0..=1.0).contains(&p) => Some(p),
        _ => {
            eprintln!(
                "[emulator] ignoring {ENV_RAIN_PROB}={raw:?} — expected a float in 0.0..=1.0"
            );
            None
        }
    }
}

fn parse_tunnel_prob_env() -> Option<f32> {
    let raw = std::env::var(ENV_TUNNEL_PROB).ok()?;
    match raw.trim().parse::<f32>() {
        Ok(p) if (0.0..=1.0).contains(&p) => Some(p),
        _ => {
            eprintln!(
                "[emulator] ignoring {ENV_TUNNEL_PROB}={raw:?} — expected a float in 0.0..=1.0"
            );
            None
        }
    }
}

fn main() -> Result<()> {
    let interface = "vcan0";
    let socket = CanSocket::open(interface)?;

    let mut cfg = PhysicalWorldModelConfig::daytime_tunnel_profile();
    if let Some(p) = parse_tunnel_prob_env() {
        cfg.ambient_road_light.tunnel_event_probability_per_tick = p;
        println!("[emulator] {ENV_TUNNEL_PROB}={p} — tunnel entry probability per 100 ms tick");
    }
    if let Some(p) = parse_rain_prob_env() {
        cfg.rain.rain_event_probability_per_tick = p;
        println!("[emulator] {ENV_RAIN_PROB}={p} — rain entry probability per 100 ms tick");
    }
    let mut car = PhysicalCar::new_with_config(cfg);

    println!("🚀 Emulator active on {interface}. Publishing RPM + ambient lux + rain sensor...");

    const TICK_MS: u64 = 100;

    loop {
        car.update();

        let rpm_signal = VssSignal::EngineRpm(car.rpm());
        socket.write_frame(&rpm_signal.to_can_frame()?)?;

        let ambient_lux_signal = VssSignal::AmbientLux(car.ambient_lux());
        socket.write_frame(&ambient_lux_signal.to_can_frame()?)?;

        let rain_signal = VssSignal::RainDetected(car.rain_detected());
        socket.write_frame(&rain_signal.to_can_frame()?)?;

        thread::sleep(Duration::from_millis(TICK_MS));
    }
}
