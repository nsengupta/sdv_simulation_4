//! CAN envelope adapter for front-headlamp device payloads.

use common::{ActuationCommand, CorrelationId};
use socketcan::{CanFrame, EmbeddedFrame, StandardId};

use crate::devices::front_headlamp::codec::{
    decode_payload, encode_payload, FrontHeadlampActuationPayload, KIND_ACK_OFF, KIND_ACK_ON,
    KIND_CMD_OFF, KIND_CMD_ON, KIND_NACK_OFF, KIND_NACK_ON,
};

pub const ID_FRONT_HEADLAMP: u16 = 0x204;

/// Placeholder `source_id` when reconstructing [`ActuationCommand`] from wire CMD frames (not on bus).
pub const CMD_PAYLOAD_SOURCE_ID: &str = "front-headlamp-actuator";

fn standard_id() -> Result<StandardId, socketcan::Error> {
    StandardId::new(ID_FRONT_HEADLAMP).ok_or_else(|| {
        socketcan::Error::from(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid standard id",
        ))
    })
}

fn build_frame(kind: u8, cmd: &ActuationCommand) -> Result<CanFrame, socketcan::Error> {
    let sid = standard_id()?;
    let (session_id, sequence_no) = actuation_command_wire_meta(cmd);
    let data = encode_payload(FrontHeadlampActuationPayload {
        kind,
        session_id,
        sequence_no,
    });
    CanFrame::new(sid, &data).ok_or_else(|| {
        socketcan::Error::from(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "CAN frame build",
        ))
    })
}

pub fn actuation_command_wire_meta(cmd: &ActuationCommand) -> (u16, u32) {
    let cid = match cmd {
        ActuationCommand::SwitchFrontHeadlampOn { correlation_id }
        | ActuationCommand::SwitchFrontHeadlampOff { correlation_id } => correlation_id,
        ActuationCommand::StartWiper | ActuationCommand::StopWiper => {
            panic!("wiper commands carry no CorrelationId; do not pass them to front_headlamp::can");
        }
    };
    (cid.session_id as u16, cid.sequence_no as u32)
}

pub fn encode_command_frame(cmd: &ActuationCommand) -> Result<CanFrame, socketcan::Error> {
    let kind = match cmd {
        ActuationCommand::SwitchFrontHeadlampOn { .. } => KIND_CMD_ON,
        ActuationCommand::SwitchFrontHeadlampOff { .. } => KIND_CMD_OFF,
        ActuationCommand::StartWiper | ActuationCommand::StopWiper => {
            return Err(socketcan::Error::from(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "wiper commands are not encoded by the front-headlamp CAN device",
            )));
        }
    };
    build_frame(kind, cmd)
}

pub fn encode_ack_frame(cmd: &ActuationCommand) -> Result<CanFrame, socketcan::Error> {
    let kind = match cmd {
        ActuationCommand::SwitchFrontHeadlampOn { .. } => KIND_ACK_ON,
        ActuationCommand::SwitchFrontHeadlampOff { .. } => KIND_ACK_OFF,
        ActuationCommand::StartWiper | ActuationCommand::StopWiper => {
            return Err(socketcan::Error::from(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "wiper commands are not encoded by the front-headlamp CAN device",
            )));
        }
    };
    build_frame(kind, cmd)
}

pub fn encode_nack_frame(cmd: &ActuationCommand) -> Result<CanFrame, socketcan::Error> {
    let kind = match cmd {
        ActuationCommand::SwitchFrontHeadlampOn { .. } => KIND_NACK_ON,
        ActuationCommand::SwitchFrontHeadlampOff { .. } => KIND_NACK_OFF,
        ActuationCommand::StartWiper | ActuationCommand::StopWiper => {
            return Err(socketcan::Error::from(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "wiper commands are not encoded by the front-headlamp CAN device",
            )));
        }
    };
    build_frame(kind, cmd)
}

/// Build an [`ActuationCommand`] from a decoded CMD payload (actuator ingress).
pub fn actuation_command_from_cmd_payload(
    payload: FrontHeadlampActuationPayload,
) -> Option<ActuationCommand> {
    let correlation_id = CorrelationId {
        source_id: CMD_PAYLOAD_SOURCE_ID.into(),
        session_id: payload.session_id as u64,
        sequence_no: payload.sequence_no as u64,
    };
    match payload.kind {
        KIND_CMD_ON => Some(ActuationCommand::SwitchFrontHeadlampOn { correlation_id }),
        KIND_CMD_OFF => Some(ActuationCommand::SwitchFrontHeadlampOff { correlation_id }),
        _ => None,
    }
}

fn is_front_headlamp_frame(frame: &CanFrame) -> bool {
    let id = match frame.id() {
        socketcan::Id::Standard(s) => s.as_raw(),
        _ => return false,
    };
    id == ID_FRONT_HEADLAMP
}

pub fn decode_payload_from_can_frame(frame: &CanFrame) -> Option<FrontHeadlampActuationPayload> {
    if !is_front_headlamp_frame(frame) {
        return None;
    }
    decode_payload(frame.data())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::front_headlamp::codec::payload_to_physical;
    use common::PhysicalCarVocabulary;

    fn sample_corr() -> CorrelationId {
        CorrelationId {
            source_id: "test".into(),
            session_id: 0xabcd,
            sequence_no: 0x11223344,
        }
    }

    #[test]
    fn round_trip_ack_on_to_physical() {
        let cmd = ActuationCommand::SwitchFrontHeadlampOn {
            correlation_id: sample_corr(),
        };
        let frame = encode_ack_frame(&cmd).expect("ack frame");
        let payload = decode_payload_from_can_frame(&frame).expect("decode payload");
        let phys = payload_to_physical(payload).expect("maps");
        assert!(matches!(
            phys,
            PhysicalCarVocabulary::FrontHeadlampCommandConfirmed { on_command: true }
        ));
    }

    #[test]
    fn command_frame_not_ingressed_as_ack() {
        let cmd = ActuationCommand::SwitchFrontHeadlampOff {
            correlation_id: sample_corr(),
        };
        let frame = encode_command_frame(&cmd).expect("cmd frame");
        let payload = decode_payload_from_can_frame(&frame).expect("decode payload");
        assert!(payload_to_physical(payload).is_none());
    }

    #[test]
    fn actuation_command_wire_meta_truncates_like_encode() {
        let cmd = ActuationCommand::SwitchFrontHeadlampOn {
            correlation_id: sample_corr(),
        };
        assert_eq!(actuation_command_wire_meta(&cmd), (0xabcd, 0x11223344));
    }

    #[test]
    fn cmd_payload_round_trips_to_actuation_command() {
        let cmd = ActuationCommand::SwitchFrontHeadlampOn {
            correlation_id: sample_corr(),
        };
        let frame = encode_command_frame(&cmd).expect("cmd frame");
        let payload = decode_payload_from_can_frame(&frame).expect("decode");
        let rebuilt = actuation_command_from_cmd_payload(payload).expect("rebuild");
        assert_eq!(actuation_command_wire_meta(&rebuilt), actuation_command_wire_meta(&cmd));
    }
}
