//! Wiper actuator — listens for CMD frames on SocketCAN and logs motor state.
//!
//! Fire-and-forget protocol: no ACK/NACK is sent back to the gateway.

use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::thread;

use anyhow::Result;
use socketcan::{CanSocket, Socket};
use vehicle_device_bus::can::wire_kinds::{KIND_WIPER_CMD_START, KIND_WIPER_CMD_STOP};
use vehicle_device_bus::devices::wiper::can::decode_payload_from_can_frame;

pub const DEFAULT_CAN_INTERFACE: &str = "vcan0";

/// If set to a float in `0.0..=1.0`, the actuator randomly **ignores** a CMD (no motor actuation).
pub const ENV_DROP_RESPONSE_PROB: &str = "WIPER_ACTUATOR_DROP_RESPONSE_PROB";

const LOG_CHANNEL_CAPACITY: usize = 512;

fn parse_prob_env(key: &str) -> Option<f64> {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|p| (0.0..=1.0).contains(p))
}

fn should_respond(dont_respond_probability: f64) -> bool {
    dont_respond_probability <= 0.0 || rand::random::<f64>() >= dont_respond_probability
}

fn log_line(tx: &SyncSender<String>, dropped: &mut u64, line: String) {
    match tx.try_send(line) {
        Ok(()) => {
            if *dropped > 0
                && tx
                    .try_send(format!(
                        "[wiper-actuator]: ⚠️ dropped {dropped} log line(s) under console back-pressure"
                    ))
                    .is_ok()
            {
                *dropped = 0;
            }
        }
        Err(TrySendError::Full(_)) => *dropped += 1,
        Err(TrySendError::Disconnected(_)) => {}
    }
}

fn main() -> Result<()> {
    let interface = DEFAULT_CAN_INTERFACE;
    let dont_respond_prob = parse_prob_env(ENV_DROP_RESPONSE_PROB).unwrap_or(0.0);

    let socket = CanSocket::open(interface)?;
    println!("🌧️  Wiper actuator on {interface} (CMD in → motor log out; no ACK/NACK)");
    if dont_respond_prob > 0.0 {
        println!(
            "[wiper-actuator] {ENV_DROP_RESPONSE_PROB}={dont_respond_prob} — may ignore CMD (no motor actuation)"
        );
    }

    let (log_tx, log_rx) = sync_channel::<String>(LOG_CHANNEL_CAPACITY);
    thread::spawn(move || {
        for line in log_rx {
            eprintln!("{line}");
        }
    });
    let mut dropped: u64 = 0;

    loop {
        let frame = match socket.read_frame() {
            Ok(frame) => frame,
            Err(e) => {
                log_line(
                    &log_tx,
                    &mut dropped,
                    format!("[wiper-actuator]: read_frame failed: {e:?}"),
                );
                continue;
            }
        };
        let Some(payload) = decode_payload_from_can_frame(&frame) else {
            continue;
        };

        let motor_state = match payload.kind {
            KIND_WIPER_CMD_START => "ON",
            KIND_WIPER_CMD_STOP => "OFF",
            _ => continue,
        };

        if !should_respond(dont_respond_prob) {
            log_line(
                &log_tx,
                &mut dropped,
                format!("[wiper-actuator]: sit tight — ignored {motor_state} CMD"),
            );
            continue;
        }

        log_line(
            &log_tx,
            &mut dropped,
            format!("[wiper-actuator]: wiper motor {motor_state}"),
        );
    }
}
