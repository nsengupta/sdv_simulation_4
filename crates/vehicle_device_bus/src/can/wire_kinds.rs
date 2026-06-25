//! CAN wire-kind constants shared by device adapters/codecs.

pub const KIND_FRONT_HEADLAMP_CMD_ON: u8 = 0x01;
pub const KIND_FRONT_HEADLAMP_CMD_OFF: u8 = 0x02;
pub const KIND_FRONT_HEADLAMP_ACK_ON: u8 = 0x81;
pub const KIND_FRONT_HEADLAMP_ACK_OFF: u8 = 0x82;
pub const KIND_FRONT_HEADLAMP_NACK_ON: u8 = 0xC1;
pub const KIND_FRONT_HEADLAMP_NACK_OFF: u8 = 0xC2;

/// Wiper actuator commands (fire-and-forget; no ACK/NACK).
pub const KIND_WIPER_CMD_START: u8 = 0x11;
pub const KIND_WIPER_CMD_STOP: u8 = 0x12;
