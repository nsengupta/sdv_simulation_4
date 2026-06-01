use super::projection::{Projector, ProjectionError};
use crate::digital_twin::DigitalTwinCarVocabulary;
use crate::domain_types::PhysicalCarVocabulary;
use crate::fsm::{FrontHeadlampIncompleteCause, FrontHeadlampSwitchDirection, FsmEvent};
use crate::signals::VssSignal;

#[derive(Debug, Default, Clone, Copy)]
pub struct PhysicalToDigitalProjector;

impl Projector<PhysicalCarVocabulary, DigitalTwinCarVocabulary> for PhysicalToDigitalProjector {
    fn project(&self, input: PhysicalCarVocabulary) -> Result<DigitalTwinCarVocabulary, ProjectionError> {
        let fsm = match input {
            PhysicalCarVocabulary::TelemetryUpdate(vss) => match vss {
                VssSignal::VehicleSpeed(_) => {
                    return Err(ProjectionError::InvalidPayload(
                        "observed VehicleSpeed not wired yet; twin derives speed from EngineRpm",
                    ));
                }
                VssSignal::EngineRpm(rpm) => FsmEvent::UpdateRpm(rpm),
                VssSignal::AmbientLux(lux) => FsmEvent::UpdateAmbientLux(lux),
            },
            PhysicalCarVocabulary::TimerTick => FsmEvent::TimerTick,
            PhysicalCarVocabulary::SystemReset => FsmEvent::PowerOff,
            PhysicalCarVocabulary::FrontHeadlampCommandConfirmed { on_command } => {
                if on_command {
                    FsmEvent::FrontHeadlampOnAck
                } else {
                    FsmEvent::FrontHeadlampOffAck
                }
            }
            PhysicalCarVocabulary::FrontHeadlampCommandRejected { on_command } => {
                FsmEvent::FrontHeadlampActuationIncomplete {
                    direction: if on_command {
                        FrontHeadlampSwitchDirection::On
                    } else {
                        FrontHeadlampSwitchDirection::Off
                    },
                    cause: FrontHeadlampIncompleteCause::NegativeAck,
                }
            }
        };
        Ok(DigitalTwinCarVocabulary::Fsm(fsm))
    }
}
