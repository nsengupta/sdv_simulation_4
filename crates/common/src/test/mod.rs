//! Crate-local tests for `common` (not the `tests/` integration harness).
//!
//! Focused test modules by contract boundary:
//! - `fsm_engine_contract` — deterministic unit tests for transition/output rules
//! - `fsm_step_contract` — step boundary contract tests
//! - `fsm_properties` — property tests behind the `proptest` feature
//! - `actor_contract` — actor and transition-sink behavior contracts
//! - `scenarios_smoke` — lightweight end-to-end behavior smoke tests

#[cfg(test)]
mod actor_contract;

#[cfg(test)]
mod controller_api_contract;

#[cfg(test)]
mod fsm_engine_contract;

#[cfg(all(test, feature = "proptest"))]
mod fsm_properties;

#[cfg(test)]
mod fsm_step_contract;

#[cfg(test)]
mod lighting_step_contract;

#[cfg(test)]
mod transition_map_contract;

#[cfg(test)]
mod projection_contract;

#[cfg(test)]
mod scenarios_smoke;

/// A RAII (Resource Acquisition Is Initialization) guard for Ractor tests.
///
/// Holding one binds a spawned actor's lifetime to a stack scope: on drop it calls
/// `stop(None)` so the actor shuts down at the end of the test, keeping tests isolated. Always
/// bind it to a name (`let _guard = ..`), never `let _ = ..`, or the actor stops immediately.
pub struct ActorGuard<T: ractor::Message> {
    pub addr: ractor::ActorRef<T>,
    // Held to keep ownership of the spawned task for the guard's lifetime; never read directly
    // (shutdown is driven via `addr.stop` in `drop`).
    #[allow(dead_code)]
    pub handle: ractor::concurrency::JoinHandle<()>,
}

impl<T: ractor::Message> Drop for ActorGuard<T> {
    fn drop(&mut self) {
        // 1. Tell the actor to stop immediately
        self.addr.stop(None);

        // Note: I cannot 'await' inside a synchronous drop() function.
        // However, stopping the actor here is usually enough to
        // clear the mailbox for the next test.
    }
}

// --- WI-6: actuation round-trip test helpers (Q2) ---
//
// "send command -> observe command -> inject ack -> observe resulting transition."
// The harness plays the role of the future actuation child actor: it owns the `rx` side of the
// injected `actuation_command_tx`, asserts the outbound command, then feeds the matching
// ack/nack back through the real physical-ingress path. No production code is involved; these
// reuse the existing public seams (`VehicleControllerRuntimeOptions`, `submit_physical_car_event`).

use crate::digital_twin::DigitalTwinCarVocabulary;
use crate::twin_runtime::controller::vehicle_controller::VehicleControllerRuntimeOptions;
use crate::{ActuationCommand, PhysicalCarVocabulary, VehicleController};
use tokio::sync::mpsc;

/// Install a controller with an injected actuation channel, returning the controller, the
/// `rx` end the harness observes, and the lifetime guard.
///
/// Bind the guard to a real name to keep the actor alive for the test:
/// `let (controller, mut actuation_rx, _guard) = install_with_actuation("ID", 16).await;`
pub async fn install_with_actuation(
    identity: &str,
    capacity: usize,
) -> (
    VehicleController,
    mpsc::Receiver<ActuationCommand>,
    ActorGuard<DigitalTwinCarVocabulary>,
) {
    let (tx, rx) = mpsc::channel(capacity);
    let runtime_options = VehicleControllerRuntimeOptions {
        actuation_command_tx: Some(tx),
        ..Default::default()
    };

    let (controller, handle) =
        VehicleController::install_and_start_with_options(identity.to_string(), runtime_options)
            .await
            .expect("install actor with actuation channel");

    let guard = ActorGuard {
        addr: controller.get_actor_ref().clone(),
        handle,
    };

    (controller, rx, guard)
}

/// Await the next outbound [`ActuationCommand`], panicking on timeout or channel close.
///
/// Assert on the command's *variant* and `sequence_no` monotonicity — never on `session_id`,
/// which is derived from wall-clock time and is non-deterministic.
pub async fn expect_actuation_command(
    rx: &mut mpsc::Receiver<ActuationCommand>,
    timeout: std::time::Duration,
) -> ActuationCommand {
    match tokio::time::timeout(timeout, rx.recv()).await {
        Ok(Some(command)) => command,
        Ok(None) => panic!("actuation command channel closed before a command arrived"),
        Err(_) => panic!("timed out after {timeout:?} waiting for an actuation command"),
    }
}

/// Inject the positive acknowledgement matching an observed command, via the real physical
/// ingress path (so projection is exercised end to end).
pub async fn inject_matching_ack(controller: &VehicleController, command: &ActuationCommand) {
    let confirmed = match command {
        ActuationCommand::SwitchFrontHeadlampOn { .. } => {
            PhysicalCarVocabulary::FrontHeadlampCommandConfirmed { on_command: true }
        }
        ActuationCommand::SwitchFrontHeadlampOff { .. } => {
            PhysicalCarVocabulary::FrontHeadlampCommandConfirmed { on_command: false }
        }
    };
    controller
        .submit_physical_car_event(confirmed)
        .await
        .expect("inject matching ack");
}

/// Inject the negative acknowledgement matching an observed command, via the real physical
/// ingress path.
pub async fn inject_matching_nack(controller: &VehicleController, command: &ActuationCommand) {
    let rejected = match command {
        ActuationCommand::SwitchFrontHeadlampOn { .. } => {
            PhysicalCarVocabulary::FrontHeadlampCommandRejected { on_command: true }
        }
        ActuationCommand::SwitchFrontHeadlampOff { .. } => {
            PhysicalCarVocabulary::FrontHeadlampCommandRejected { on_command: false }
        }
    };
    controller
        .submit_physical_car_event(rejected)
        .await
        .expect("inject matching nack");
}
