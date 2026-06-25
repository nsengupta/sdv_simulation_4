//! Wiper actuator CAN device: codec + CAN envelope adapter.
//!
//! Fire-and-forget command protocol — no ACK/NACK frame types.

pub mod can;
pub mod codec;
