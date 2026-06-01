//! **L5 public API** for L6 application binaries (gateway and integration tests).
//!
//! Gateway and other edge processes must depend on this module only — not on
//! [`crate::fsm`], [`crate::twin_runtime`], or other internal modules directly.
//! See `docs/design-notes-pyramid-layers.md` (Phase A).

// --- Controller (composition root / single doorway) ---

pub use crate::twin_runtime::controller::{
    ActuationCommand, CorrelationId, VehicleController, VehicleControllerError,
    VehicleControllerRuntimeOptions,
};

// --- Physical-world ingress vocabulary ---

pub use crate::domain_types::{PhysicalCarVocabulary, VehicleEvent, VehicleState};
pub use crate::signals::VssSignal;

// --- Read model (snapshots + observable assembly state) ---

pub use crate::digital_twin::CarSnapshot;
/// Lighting sub-state as exposed on [`CarSnapshot::context`]; not an FSM-internal import path.
pub use crate::fsm::machineries::LightingState;

// --- Observation / optional runtime wiring ---

pub use crate::diagnostic::spawn_stdout_diagnostic_observer;
pub use crate::published::PublishedTransitionRecord;

// --- Headlamp ingress/egress log tokens (gateway CAN loop display) ---

pub use crate::front_headlamp_log::{
    ACK_OFF, ACK_ON, CMD_OFF, CMD_ON, MSG_ACK_OFF, MSG_ACK_ON, MSG_NACK_OFF, MSG_NACK_ON,
    MSG_REQUEST_OFF, MSG_REQUEST_ON, MSG_TIMEOUT_OFF, MSG_TIMEOUT_ON, NACK_OFF, NACK_ON,
    TIMEOUT_OFF, TIMEOUT_ON,
};

// --- Integration-test timing (prefer diagnostics assertions when sufficient) ---

pub use crate::vehicle_physics::FRONT_HEADLAMP_ON_ACK_WAIT;
