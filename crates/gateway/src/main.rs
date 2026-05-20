//! Gateway — CAN ingress speaks [`common::VehicleEvent`]; the actor consumes [`common::DigitalTwinCarVocabulary`].
//!
//! Runtime wiring lives in [`gateway_runtime`] so this file stays a thin entrypoint.

use anyhow::Result;
use std::env;

mod gateway_runtime;
mod ingress;

const VIRTUAL_CAR_IDENTITY: &str = "NASHIK-VC-001";

#[tokio::main]
async fn main() -> Result<()> {
    let print_timer_tick = env::args().any(|arg| arg == "--print-timer-tick");

    gateway_runtime::run(gateway_runtime::GatewayLaunchConfig {
        car_identity: VIRTUAL_CAR_IDENTITY,
        print_timer_tick,
        can_interface: gateway_runtime::DEFAULT_CAN_INTERFACE,
    })
    .await
}
