//! Front headlamp actuator — listens for CMD frames on SocketCAN and responds with ACK/NACK.

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

/// If set to a float in `0.0..=1.0`, the actuator randomly sends **no** ACK/NACK after the CMD.
pub const ENV_DROP_RESPONSE_PROB: &str = "FRONT_HEADLAMP_PLANT_DROP_RESPONSE_PROB";
/// If set to a float in `0.0..=1.0`, controls ACK-vs-NACK split **when the actuator responds** (`P(ACK)`).
pub const ENV_ACK_NACK_RESPONSE_PROB: &str = "FRONT_HEADLAMP_PLANT_ACK_NACK_RESPONSE_PROB";

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

    loop {
        let frame = match socket.read_frame() {
            Ok(frame) => frame,
            Err(e) => {
                eprintln!("[front-headlamp-actuator]: read_frame failed: {e:?}");
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
        };
        let (session, seq) = actuation_command_wire_meta(&cmd);
        eprintln!(
            "[front-headlamp-actuator]: received {command_direction} CMD (wire session={session} seq={seq})"
        );

        thread::sleep(ack_delay);

        if !should_ack_or_not(dont_respond_prob) {
            eprintln!(
                "[front-headlamp-actuator]: sit tight — no ACK/NACK after delay (wire session={session} seq={seq})"
            );
            continue;
        }

        let send_ack = should_send_ack_response(ack_nack_response_prob);
        eprintln!(
            "[front-headlamp-actuator]: responding with {} for {command_direction} (wire session={session} seq={seq})",
            if send_ack { "ACK" } else { "NACK" }
        );

        let response_frame = if send_ack {
            encode_ack_frame(&cmd)
        } else {
            encode_nack_frame(&cmd)
        };
        match response_frame {
            Ok(f) => {
                if let Err(e) = socket.write_frame(&f) {
                    eprintln!(
                        "[front-headlamp-actuator]: {} write_frame failed: {e:?}",
                        if send_ack { "ACK" } else { "NACK" }
                    );
                }
            }
            Err(e) => eprintln!(
                "[front-headlamp-actuator]: encode {} failed: {e:?}",
                if send_ack { "ACK" } else { "NACK" }
            ),
        }
    }
}
