//! Front headlamp actuator — listens for CMD frames on SocketCAN and responds with ACK/NACK.

use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use common::ActuationCommand;
use socketcan::{CanSocket, Socket};
use vehicle_device_bus::devices::front_headlamp::can::{
    actuation_command_from_cmd_payload, actuation_command_wire_meta, decode_payload_from_can_frame,
    encode_ack_frame, encode_nack_frame,
};

pub const DEFAULT_CAN_INTERFACE: &str = "vcan0";
const DEFAULT_ACK_DELAY_MS: u64 = 150;
/// Default ACK probability when the actuator chooses to send a response frame.
pub const DEFAULT_ACK_NACK_RESPONSE_PROB: f64 = 0.7;

/// Bound on the off-thread log queue. Logging is best-effort: a frozen console (Ctrl-S / XOFF)
/// must never stall the CAN read/respond loop, so lines are dropped once this fills.
const LOG_CHANNEL_CAPACITY: usize = 512;

/// Best-effort, non-blocking log hand-off to the drainer thread. On a full queue (console
/// back-pressure) the line is dropped and counted; the running drop count is surfaced on the
/// next successful send so the gap is visible rather than silent.
fn log_line(tx: &SyncSender<String>, dropped: &mut u64, line: String) {
    match tx.try_send(line) {
        Ok(()) => {
            if *dropped > 0
                && tx
                    .try_send(format!(
                        "[front-headlamp-actuator]: ⚠️ dropped {dropped} log line(s) under console back-pressure"
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

/// If set to a float in `0.0..=1.0`, the actuator randomly sends **no** ACK/NACK after the CMD.
pub const ENV_DROP_RESPONSE_PROB: &str = "FRONT_HEADLAMP_ACTUATOR_DROP_RESPONSE_PROB";
/// If set to a float in `0.0..=1.0`, controls ACK-vs-NACK split **when the actuator responds** (`P(ACK)`).
pub const ENV_ACK_NACK_RESPONSE_PROB: &str = "FRONT_HEADLAMP_ACTUATOR_ACK_NACK_RESPONSE_PROB";

fn parse_prob_env(key: &str) -> Option<f64> {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|p| (0.0..=1.0).contains(p))
}

fn should_ack_or_not(dont_respond_probability: f64) -> bool {
    dont_respond_probability <= 0.0 || rand::random::<f64>() >= dont_respond_probability
}

fn should_send_ack_response(ack_nack_response_probability: f64) -> bool {
    let p = ack_nack_response_probability.clamp(0.0, 1.0);
    rand::random::<f64>() < p
}

fn main() -> Result<()> {
    let interface = DEFAULT_CAN_INTERFACE;
    let ack_delay = Duration::from_millis(DEFAULT_ACK_DELAY_MS);
    let dont_respond_prob = parse_prob_env(ENV_DROP_RESPONSE_PROB).unwrap_or(0.0);
    let ack_nack_response_prob =
        parse_prob_env(ENV_ACK_NACK_RESPONSE_PROB).unwrap_or(DEFAULT_ACK_NACK_RESPONSE_PROB);

    let socket = CanSocket::open(interface)?;
    println!("💡 Front headlamp actuator on {interface} (CMD in → ACK/NACK out, delay {ack_delay:?})");
    if dont_respond_prob > 0.0 {
        println!(
            "[front-headlamp-actuator] {ENV_DROP_RESPONSE_PROB}={dont_respond_prob} — may sit tight (no response)"
        );
    }
    println!(
        "[front-headlamp-actuator] {ENV_ACK_NACK_RESPONSE_PROB}={ack_nack_response_prob} (P(ACK) when responding)"
    );

    // Off-hot-path logger: keep all in-loop logging off the CAN read/respond path so that console
    // back-pressure (a paused terminal via Ctrl-S/XOFF, a slow pipe, a full disk) can never block
    // `read_frame`/`write_frame` and starve the digital twin of ACK/NACK responses.
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
                    format!("[front-headlamp-actuator]: read_frame failed: {e:?}"),
                );
                continue;
            }
        };
        let Some(payload) = decode_payload_from_can_frame(&frame) else {
            continue;
        };
        let Some(cmd) = actuation_command_from_cmd_payload(payload) else {
            continue;
        };

        let command_direction = match &cmd {
            ActuationCommand::SwitchFrontHeadlampOn { .. } => "ON",
            ActuationCommand::SwitchFrontHeadlampOff { .. } => "OFF",
            // Wiper commands are never decoded by the front-headlamp actuator's
            // codec (`actuation_command_from_cmd_payload`), so this arm is unreachable
            // in practice — but required for `ActuationCommand` match exhaustiveness.
            ActuationCommand::StartWiper | ActuationCommand::StopWiper => continue,
        };
        let (session, seq) = actuation_command_wire_meta(&cmd);
        log_line(
            &log_tx,
            &mut dropped,
            format!(
                "[front-headlamp-actuator]: received {command_direction} CMD (wire session={session} seq={seq})"
            ),
        );

        thread::sleep(ack_delay);

        if !should_ack_or_not(dont_respond_prob) {
            log_line(
                &log_tx,
                &mut dropped,
                format!(
                    "[front-headlamp-actuator]: sit tight — no ACK/NACK after delay (wire session={session} seq={seq})"
                ),
            );
            continue;
        }

        let send_ack = should_send_ack_response(ack_nack_response_prob);
        log_line(
            &log_tx,
            &mut dropped,
            format!(
                "[front-headlamp-actuator]: responding with {} for {command_direction} (wire session={session} seq={seq})",
                if send_ack { "ACK" } else { "NACK" }
            ),
        );

        let response_frame = if send_ack {
            encode_ack_frame(&cmd)
        } else {
            encode_nack_frame(&cmd)
        };
        match response_frame {
            Ok(f) => {
                if let Err(e) = socket.write_frame(&f) {
                    log_line(
                        &log_tx,
                        &mut dropped,
                        format!(
                            "[front-headlamp-actuator]: {} write_frame failed: {e:?}",
                            if send_ack { "ACK" } else { "NACK" }
                        ),
                    );
                }
            }
            Err(e) => log_line(
                &log_tx,
                &mut dropped,
                format!(
                    "[front-headlamp-actuator]: encode {} failed: {e:?}",
                    if send_ack { "ACK" } else { "NACK" }
                ),
            ),
        }
    }
}
