//! Bus-level codec tests for wiper CMD frames (Step 10).

use common::ActuationCommand;
use socketcan::EmbeddedFrame;
use vehicle_device_bus::can::wire_kinds::{KIND_WIPER_CMD_START, KIND_WIPER_CMD_STOP};
use vehicle_device_bus::devices::wiper::can::{
    decode_payload_from_can_frame, encode_wiper_command_frame, ID_WIPER,
};

#[test]
fn given_start_wiper_command_when_encoded_then_frame_has_wiper_id_and_start_kind() {
    let frame = encode_wiper_command_frame(&ActuationCommand::StartWiper).expect("encode start");
    let id = match frame.id() {
        socketcan::Id::Standard(s) => s.as_raw(),
        _ => panic!("expected standard CAN id"),
    };
    assert_eq!(id, ID_WIPER);
    assert_eq!(frame.data()[0], KIND_WIPER_CMD_START);
}

#[test]
fn given_stop_wiper_command_when_encoded_then_frame_has_wiper_id_and_stop_kind() {
    let frame = encode_wiper_command_frame(&ActuationCommand::StopWiper).expect("encode stop");
    let id = match frame.id() {
        socketcan::Id::Standard(s) => s.as_raw(),
        _ => panic!("expected standard CAN id"),
    };
    assert_eq!(id, ID_WIPER);
    assert_eq!(frame.data()[0], KIND_WIPER_CMD_STOP);
}

#[test]
fn given_encoded_start_wiper_frame_when_decoded_then_start_kind_round_trips() {
    let frame = encode_wiper_command_frame(&ActuationCommand::StartWiper).expect("encode");
    let payload = decode_payload_from_can_frame(&frame).expect("decode");
    assert_eq!(payload.kind, KIND_WIPER_CMD_START);
}
