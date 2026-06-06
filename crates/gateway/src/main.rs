//! Gateway — CAN ingress speaks [`common::facade::VehicleEvent`]; the twin is driven via
//! [`common::facade::VehicleController`] only (no direct FSM or actor imports).
//!
//! Runtime wiring lives in [`gateway_runtime`] so this file stays a thin entrypoint.

use anyhow::Result;
use std::env;

mod gateway_runtime;
mod ingress;
mod transition_log;

const VIRTUAL_CAR_IDENTITY: &str = "My-Opel-Corsa-1.4-GSi";

#[tokio::main]
async fn main() -> Result<()> {
    let print_transitions_only = env::args().any(|arg| arg == "--print-transitions-only");
    let print_timer_tick = env::args().any(|arg| arg == "--print-timer-tick");
    let trace_actuation_ingress = env::args().any(|arg| arg == "--trace-actuation-ingress");

    gateway_runtime::run(gateway_runtime::GatewayLaunchConfig {
        car_identity: VIRTUAL_CAR_IDENTITY,
        print_timer_tick: print_timer_tick && !print_transitions_only,
        print_transitions_only,
        trace_actuation_ingress: trace_actuation_ingress && !print_transitions_only,
        can_interface: gateway_runtime::DEFAULT_CAN_INTERFACE,
    })
    .await
}
