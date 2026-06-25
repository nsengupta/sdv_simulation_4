//! CAN envelope adapter for wiper device payloads.
//!
//! Fire-and-forget protocol: gateway encodes a command frame; no ACK/NACK.

use common::ActuationCommand;
use socketcan::{CanFrame, EmbeddedFrame, StandardId};

use crate::can::wire_kinds::{KIND_WIPER_CMD_START, KIND_WIPER_CMD_STOP};
use crate::devices::wiper::codec::{decode_payload, encode_payload, WiperCommandPayload};

pub const ID_WIPER: u16 = 0x205;

fn standard_id() -> Result<StandardId, socketcan::Error> {
    StandardId::new(ID_WIPER).ok_or_else(|| {
        socketcan::Error::from(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid wiper standard id",
        ))
    })
}

fn build_frame(kind: u8) -> Result<CanFrame, socketcan::Error> {
    let sid = standard_id()?;
    let data = encode_payload(WiperCommandPayload { kind });
    CanFrame::new(sid, &data).ok_or_else(|| {
        socketcan::Error::from(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "wiper CAN frame build",
        ))
    })
}

/// Encode an [`ActuationCommand::StartWiper`] or [`ActuationCommand::StopWiper`] as a CAN frame.
///
/// Returns `Err` for any other command variant (headlamp commands are not encoded here).
pub fn encode_wiper_command_frame(cmd: &ActuationCommand) -> Result<CanFrame, socketcan::Error> {
    let kind = match cmd {
        ActuationCommand::StartWiper => KIND_WIPER_CMD_START,
        ActuationCommand::StopWiper  => KIND_WIPER_CMD_STOP,
        _ => {
            return Err(socketcan::Error::from(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "non-wiper command passed to wiper CAN encoder",
            )));
        }
    };
    build_frame(kind)
}

fn is_wiper_frame(frame: &CanFrame) -> bool {
    match frame.id() {
        socketcan::Id::Standard(s) => s.as_raw() == ID_WIPER,
        _ => false,
    }
}

/// Decode a raw CAN frame into a [`WiperCommandPayload`], returning `None` if the frame does
/// not belong to the wiper device or the payload is malformed.
pub fn decode_payload_from_can_frame(frame: &CanFrame) -> Option<WiperCommandPayload> {
    if !is_wiper_frame(frame) {
        return None;
    }
    decode_payload(frame.data())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn given_start_wiper_command_when_encoded_then_correct_can_id() {
        let frame = encode_wiper_command_frame(&ActuationCommand::StartWiper)
            .expect("StartWiper must encode");
        let id = match frame.id() {
            socketcan::Id::Standard(s) => s.as_raw(),
            _ => panic!("expected standard CAN id"),
        };
        assert_eq!(id, ID_WIPER);
    }

    #[test]
    fn given_stop_wiper_command_when_encoded_then_correct_can_id() {
        let frame = encode_wiper_command_frame(&ActuationCommand::StopWiper)
            .expect("StopWiper must encode");
        let id = match frame.id() {
            socketcan::Id::Standard(s) => s.as_raw(),
            _ => panic!("expected standard CAN id"),
        };
        assert_eq!(id, ID_WIPER);
    }

    #[test]
    fn given_wiper_and_headlamp_can_ids_when_compared_then_distinct() {
        use crate::devices::front_headlamp::can::ID_FRONT_HEADLAMP;
        assert_ne!(ID_WIPER, ID_FRONT_HEADLAMP, "wiper CAN id must not collide with headlamp");
    }

    #[test]
    fn given_start_wiper_frame_when_decoded_then_start_kind() {
        let frame = encode_wiper_command_frame(&ActuationCommand::StartWiper)
            .expect("encode");
        let payload = decode_payload_from_can_frame(&frame).expect("decode");
        assert_eq!(payload.kind, KIND_WIPER_CMD_START);
    }

    #[test]
    fn given_stop_wiper_frame_when_decoded_then_stop_kind() {
        let frame = encode_wiper_command_frame(&ActuationCommand::StopWiper)
            .expect("encode");
        let payload = decode_payload_from_can_frame(&frame).expect("decode");
        assert_eq!(payload.kind, KIND_WIPER_CMD_STOP);
    }

    #[test]
    fn given_headlamp_command_frame_when_decoded_as_wiper_then_none() {
        use common::CorrelationId;
        use crate::devices::front_headlamp::can::encode_command_frame;
        let corr = CorrelationId {
            source_id: "test".into(),
            session_id: 1,
            sequence_no: 1,
        };
        let headlamp_frame = encode_command_frame(
            &ActuationCommand::SwitchFrontHeadlampOn { correlation_id: corr }
        ).expect("encode headlamp");
        assert!(decode_payload_from_can_frame(&headlamp_frame).is_none());
    }

    #[test]
    fn given_headlamp_command_when_wiper_encoded_then_error() {
        use common::CorrelationId;
        let corr = CorrelationId { source_id: "t".into(), session_id: 0, sequence_no: 0 };
        let result = encode_wiper_command_frame(
            &ActuationCommand::SwitchFrontHeadlampOn { correlation_id: corr }
        );
        assert!(result.is_err());
    }
}
