//! CAN wire protocols for vehicle actuators and devices.
//!
//! One module per device under [`devices`] (e.g. [`devices::front_headlamp`]).
//! Gateway and standalone actuator binaries depend on this crate; domain logic stays in [`common`].

pub mod can;
pub mod devices;
