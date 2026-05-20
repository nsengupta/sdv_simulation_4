//! Transport-agnostic front-headlamp payload codec and semantic mapping.

use common::PhysicalCarVocabulary;

pub use crate::can::wire_kinds::{
    KIND_FRONT_HEADLAMP_ACK_OFF as KIND_ACK_OFF, KIND_FRONT_HEADLAMP_ACK_ON as KIND_ACK_ON,
    KIND_FRONT_HEADLAMP_CMD_OFF as KIND_CMD_OFF, KIND_FRONT_HEADLAMP_CMD_ON as KIND_CMD_ON,
    KIND_FRONT_HEADLAMP_NACK_OFF as KIND_NACK_OFF, KIND_FRONT_HEADLAMP_NACK_ON as KIND_NACK_ON,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrontHeadlampActuationPayload {
    pub kind: u8,
    pub session_id: u16,
    pub sequence_no: u32,
}

pub fn encode_payload(payload: FrontHeadlampActuationPayload) -> [u8; 8] {
    let mut data = [0u8; 8];
    data[0] = payload.kind;
    data[1..3].copy_from_slice(&payload.session_id.to_be_bytes());
    data[3..7].copy_from_slice(&payload.sequence_no.to_be_bytes());
    data
}

pub fn decode_payload(data: &[u8]) -> Option<FrontHeadlampActuationPayload> {
    if data.len() < 8 {
        return None;
    }
    Some(FrontHeadlampActuationPayload {
        kind: data[0],
        session_id: u16::from_be_bytes([data[1], data[2]]),
        sequence_no: u32::from_be_bytes([data[3], data[4], data[5], data[6]]),
    })
}

pub fn payload_to_physical(payload: FrontHeadlampActuationPayload) -> Option<PhysicalCarVocabulary> {
    match payload.kind {
        KIND_ACK_ON => Some(PhysicalCarVocabulary::FrontHeadlampCommandConfirmed { on_command: true }),
        KIND_ACK_OFF => Some(PhysicalCarVocabulary::FrontHeadlampCommandConfirmed {
            on_command: false,
        }),
        KIND_NACK_ON => Some(PhysicalCarVocabulary::FrontHeadlampCommandRejected { on_command: true }),
        KIND_NACK_OFF => Some(PhysicalCarVocabulary::FrontHeadlampCommandRejected {
            on_command: false,
        }),
        KIND_CMD_ON | KIND_CMD_OFF => None,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_round_trip() {
        let payload = FrontHeadlampActuationPayload {
            kind: KIND_ACK_ON,
            session_id: 0xabcd,
            sequence_no: 0x11223344,
        };
        let data = encode_payload(payload);
        assert_eq!(decode_payload(&data), Some(payload));
    }

    #[test]
    fn decode_rejects_short_payload() {
        assert!(decode_payload(&[KIND_ACK_ON, 0x00, 0x01]).is_none());
    }
}
