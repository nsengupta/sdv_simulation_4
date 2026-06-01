// Sibling order is *dependee before dependent* (foundation first), not "flow" order.
// `digital_twin` imports `fsm`; `fsm` does not import `digital_twin`.
pub mod vehicle_physics;
pub mod vehicle_state;
pub mod domain_types;
pub mod twin_runtime;
pub mod fsm;
pub mod digital_twin;
pub mod signals;
pub mod published;
pub mod transition_sink;
pub mod front_headlamp_log;
pub mod diagnostic;
pub mod facade;

#[cfg(test)]
mod test;

pub use digital_twin::{
    verify_state_laws, CarSnapshot, DigitalTwinCar, DigitalTwinCarError, DigitalTwinCarVocabulary,
    LawViolation, NotFsmVocabulary, StateLaw, STATE_LAWS,
};
pub use domain_types::{PhysicalCarVocabulary, VehicleEvent, VehicleState};
pub use twin_runtime::connectors::{PhysicalToDigitalProjector, Projector, ProjectionError};
pub use twin_runtime::controller::{
    ActuationCommand, ActuationError, ActuationFeedback, ActuationManager, CorrelationId,
    DefaultActuationManager, VehicleController, VehicleControllerError,
    VehicleControllerRuntimeOptions,
};
pub use signals::VssSignal;
pub use front_headlamp_log::{
    ACK_OFF, ACK_ON, CMD_OFF, CMD_ON, MSG_ACK_OFF, MSG_ACK_ON, MSG_NACK_OFF, MSG_NACK_ON,
    MSG_REQUEST_OFF, MSG_REQUEST_ON, MSG_TIMEOUT_OFF, MSG_TIMEOUT_ON, NACK_OFF, NACK_ON,
    TIMEOUT_OFF, TIMEOUT_ON,
};
pub use vehicle_physics::{
    calculate_speed_from_rpm, extreme_operation_active, operational_warning_active,
    speed_threshold_exceeded, EXTREME_OPERATION_WARNING_MESSAGE, FRONT_HEADLAMP_OFF_ACK_WAIT,
    FRONT_HEADLAMP_ON_ACK_WAIT, LUX_OFF_THRESHOLD, LUX_ON_THRESHOLD, RPM_DRIVING_THRESHOLD,
    RPM_EXTREME_OPERATION_THRESHOLD, RPM_IDLE, RPM_REDLINE_THRESHOLD,
    RPM_STRESS_DURATION_THRESHOLD_SECS, SPEED_EXTREME_OPERATION_THRESHOLD_KPH,
    SPEED_THRESHOLD_WARNING_MESSAGE,
};
pub use published::{
    PublishedDomainAction, PublishedFrontHeadlampIncompleteCause,
    PublishedFrontHeadlampSwitchDirection, PublishedFsmEvent, PublishedFsmState,
    PublishedHeadlampContext, PublishedHealthContext, PublishedLightingState,
    PublishedPowertrainContext, PublishedTransitionRecord, PublishedVehicleContext,
    PublishedVisibilityContext, PublishedWheelRpm, SessionEpoch,
};
pub use transition_sink::{
    TokioMpscTransitionRecordSink, TransitionRecordSink, TransitionSinkError,
};
pub use diagnostic::{
    DiagnosticLevel, DiagnosticMessage, DiagnosticSink, DiagnosticSinkError,
    TokioMpscDiagnosticSink, diag_state_transition, diag_timer_tick,
    diag_actuation_failure, diag_warning, diag_transition_sink_full, diag_transition_sink_closed,
    spawn_stdout_diagnostic_observer,
};
