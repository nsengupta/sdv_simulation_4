//! Transport-agnostic wiper payload codec.
//!
//! The wiper CAN protocol is fire-and-forget (no ACK/NACK): the gateway sends a one-byte
//! command frame and never expects a response.  No `CorrelationId` is needed.

use crate::can::wire_kinds::{KIND_WIPER_CMD_START, KIND_WIPER_CMD_STOP};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WiperCommandPayload {
    pub kind: u8,
}

pub fn encode_payload(payload: WiperCommandPayload) -> [u8; 8] {
    let mut data = [0u8; 8];
    data[0] = payload.kind;
    data
}

pub fn decode_payload(data: &[u8]) -> Option<WiperCommandPayload> {
    if data.is_empty() {
        return None;
    }
    Some(WiperCommandPayload { kind: data[0] })
}

pub fn kind_is_start(kind: u8) -> bool {
    kind == KIND_WIPER_CMD_START
}

pub fn kind_is_stop(kind: u8) -> bool {
    kind == KIND_WIPER_CMD_STOP
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn given_start_payload_when_encoded_then_round_trips() {
        let p = WiperCommandPayload { kind: KIND_WIPER_CMD_START };
        let data = encode_payload(p);
        assert_eq!(decode_payload(&data), Some(p));
    }

    #[test]
    fn given_stop_payload_when_encoded_then_round_trips() {
        let p = WiperCommandPayload { kind: KIND_WIPER_CMD_STOP };
        let data = encode_payload(p);
        assert_eq!(decode_payload(&data), Some(p));
    }

    #[test]
    fn given_empty_payload_when_decoded_then_none() {
        assert!(decode_payload(&[]).is_none());
    }
}
