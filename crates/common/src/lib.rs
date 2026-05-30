// Sibling order is *dependee before dependent* (foundation first), not "flow" order.
// `digital_twin` imports `fsm`; `fsm` does not import `digital_twin`.
pub mod domain_types;
pub mod engine;
pub mod fsm;
pub mod digital_twin;
pub mod signals;
pub mod published;
pub mod transition_sink;
pub mod front_headlamp_log;
pub mod vehicle_constants;
pub mod vehicle_kinematics;
pub mod diagnostic;
mod virtual_car_actor;

#[cfg(test)]
mod test;

pub use digital_twin::{
    verify_state_laws, DigitalTwinCar, DigitalTwinCarError, DigitalTwinCarVocabulary, LawViolation,
    NotFsmVocabulary, StateLaw, STATE_LAWS,
};
pub use domain_types::{PhysicalCarVocabulary, VehicleEvent, VehicleState};
pub use engine::connectors::{PhysicalToDigitalProjector, Projector, ProjectionError};
pub use engine::context::VehicleControllerContext;
pub use engine::controller::{
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
pub use vehicle_kinematics::{calculate_speed_from_rpm, refresh_context_speed};
pub use vehicle_constants::{
    extreme_operation_active, operational_warning_active, speed_threshold_exceeded,
    EXTREME_OPERATION_WARNING_MESSAGE, RPM_EXTREME_OPERATION_THRESHOLD,
    SPEED_EXTREME_OPERATION_THRESHOLD_KPH, SPEED_THRESHOLD_WARNING_MESSAGE,
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
