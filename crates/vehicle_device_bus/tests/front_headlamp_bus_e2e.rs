//! Bus-level integration test for front-headlamp ACK ingress on `vcan0`.
//!
//! This test exercises real SocketCAN I/O:
//! writer socket -> `vcan0` -> reader socket -> wire decode helpers.

use std::time::{Duration, Instant};

use common::{ActuationCommand, CorrelationId, PhysicalCarVocabulary};
use socketcan::{CanSocket, EmbeddedFrame, Socket};
use vehicle_device_bus::devices::front_headlamp::codec::{payload_to_physical, KIND_ACK_ON, KIND_CMD_ON, KIND_NACK_ON};
use vehicle_device_bus::devices::front_headlamp::can::{
    actuation_command_wire_meta, decode_payload_from_can_frame, encode_ack_frame, encode_command_frame,
    encode_nack_frame,
};

const TEST_CAN_INTERFACE: &str = "vcan0";

fn sample_corr() -> CorrelationId {
    CorrelationId {
        source_id: "gateway-bus-e2e".to_string(),
        session_id: 0x1234,
        sequence_no: 0x89abcdef,
    }
}

fn open_bus_pair() -> Option<(CanSocket, CanSocket)> {
    let tx = match CanSocket::open(TEST_CAN_INTERFACE) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("skipping test: cannot open {TEST_CAN_INTERFACE}: {e}");
            return None;
        }
    };
    let rx = match CanSocket::open(TEST_CAN_INTERFACE) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("skipping test: cannot open reader on {TEST_CAN_INTERFACE}: {e}");
            return None;
        }
    };
    Some((tx, rx))
}

async fn recv_first_frame_with_kind(rx: CanSocket, kind: u8, timeout: Duration) -> Option<socketcan::CanFrame> {
    tokio::task::spawn_blocking(move || {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let frame = match rx.read_frame() {
                Ok(frame) => frame,
                Err(_) => continue,
            };
            if frame.data().first().copied() == Some(kind) {
                return Some(frame);
            }
        }
        None
    })
    .await
    .expect("join read loop task")
}

async fn recv_first_ack_or_nack(rx: CanSocket, timeout: Duration) -> Option<socketcan::CanFrame> {
    tokio::task::spawn_blocking(move || {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let frame = match rx.read_frame() {
                Ok(frame) => frame,
                Err(_) => continue,
            };
            let Some(kind) = frame.data().first().copied() else {
                continue;
            };
            if matches!(kind, KIND_ACK_ON | KIND_NACK_ON) {
                return Some(frame);
            }
        }
        None
    })
    .await
    .expect("join read loop task")
}

#[tokio::test]
async fn front_headlamp_ack_frame_round_trips_over_vcan_and_decodes() {
    let Some((tx, rx)) = open_bus_pair() else {
        return;
    };

    let cmd = ActuationCommand::SwitchFrontHeadlampOn {
        correlation_id: sample_corr(),
    };
    let frame = encode_ack_frame(&cmd).expect("encode ACK frame");

    tx.write_frame(&frame).expect("write ACK frame to vcan");

    let expected_wire = actuation_command_wire_meta(&cmd);
    let got = recv_first_frame_with_kind(rx, KIND_ACK_ON, Duration::from_secs(2))
        .await
        .expect("did not receive expected ACK frame kind on vcan0 before timeout");
    let payload = decode_payload_from_can_frame(&got)
        .expect("decode front-headlamp payload from CAN");
    assert_eq!((payload.session_id, payload.sequence_no), expected_wire);
    let physical = payload_to_physical(payload)
        .expect("ACK frame should map to physical vocabulary");
    assert!(matches!(
        physical,
        PhysicalCarVocabulary::FrontHeadlampCommandConfirmed { on_command: true }
    ));
}

#[tokio::test]
async fn front_headlamp_nack_frame_round_trips_over_vcan_and_decodes() {
    let Some((tx, rx)) = open_bus_pair() else {
        return;
    };
    let cmd = ActuationCommand::SwitchFrontHeadlampOn {
        correlation_id: sample_corr(),
    };
    let frame = encode_nack_frame(&cmd).expect("encode NACK frame");
    tx.write_frame(&frame).expect("write NACK frame to vcan");

    let got = recv_first_frame_with_kind(rx, KIND_NACK_ON, Duration::from_secs(2))
        .await
        .expect("did not receive expected NACK frame kind on vcan0 before timeout");
    let payload = decode_payload_from_can_frame(&got)
        .expect("decode front-headlamp payload from CAN");
    let physical = payload_to_physical(payload)
        .expect("NACK frame should map to physical vocabulary");
    assert!(matches!(
        physical,
        PhysicalCarVocabulary::FrontHeadlampCommandRejected { on_command: true }
    ));
}

#[tokio::test]
async fn front_headlamp_command_frame_is_not_ingressed_as_physical_event() {
    let Some((tx, rx)) = open_bus_pair() else {
        return;
    };
    let cmd = ActuationCommand::SwitchFrontHeadlampOn {
        correlation_id: sample_corr(),
    };
    let frame = encode_command_frame(&cmd).expect("encode CMD frame");
    tx.write_frame(&frame).expect("write command frame to vcan");

    let got = recv_first_frame_with_kind(rx, KIND_CMD_ON, Duration::from_secs(2))
        .await
        .expect("did not receive expected CMD frame kind on vcan0 before timeout");
    let payload = decode_payload_from_can_frame(&got)
        .expect("decode front-headlamp payload from CAN");
    assert!(
        payload_to_physical(payload).is_none(),
        "command frames must not be ingressed as physical ACK/NACK events"
    );
}

#[tokio::test]
async fn front_headlamp_no_response_window_has_no_ack_or_nack_frames() {
    let Some((_tx, rx)) = open_bus_pair() else {
        return;
    };
    let maybe_response = recv_first_ack_or_nack(rx, Duration::from_millis(300)).await;
    assert!(
        maybe_response.is_none(),
        "unexpected ACK/NACK observed in no-response window"
    );
}
