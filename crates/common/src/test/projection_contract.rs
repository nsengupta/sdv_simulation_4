//! Contract tests for projection boundaries between physical and digital vocabularies.

use crate::digital_twin::DigitalTwinCarVocabulary;
use crate::engine::connectors::{PhysicalToDigitalProjector, ProjectionError, Projector};
use crate::fsm::FsmEvent;
use crate::{PhysicalCarVocabulary, VssSignal};

#[test]
fn given_timer_tick_when_projected_then_maps_to_fsm_timer_tick() {
    let projector = PhysicalToDigitalProjector;
    let out = projector
        .project(PhysicalCarVocabulary::TimerTick)
        .expect("projection must succeed");
    match out {
        DigitalTwinCarVocabulary::Fsm(FsmEvent::TimerTick) => {}
        other => panic!("unexpected timer tick mapping: {other:?}"),
    }
}

#[test]
fn given_system_reset_when_projected_then_maps_to_fsm_power_off() {
    let projector = PhysicalToDigitalProjector;
    let out = projector
        .project(PhysicalCarVocabulary::SystemReset)
        .expect("projection must succeed");
    match out {
        DigitalTwinCarVocabulary::Fsm(FsmEvent::PowerOff) => {}
        other => panic!("unexpected reset mapping: {other:?}"),
    }
}

#[test]
fn given_observed_speed_signal_when_projected_then_rejects_until_ecu_path_exists() {
    let projector = PhysicalToDigitalProjector;
    let err = projector
        .project(PhysicalCarVocabulary::TelemetryUpdate(VssSignal::VehicleSpeed(50.0)))
        .expect_err("observed speed must not ingress as UpdateSpeed");
    assert!(matches!(err, ProjectionError::InvalidPayload(_)));
}

#[test]
fn given_rpm_signal_when_projected_then_maps_exact_rpm() {
    let projector = PhysicalToDigitalProjector;
    let out = projector
        .project(PhysicalCarVocabulary::TelemetryUpdate(VssSignal::EngineRpm(4321)))
        .expect("rpm projection must succeed");
    match out {
        DigitalTwinCarVocabulary::Fsm(FsmEvent::UpdateRpm(v)) => assert_eq!(v, 4321),
        other => panic!("unexpected rpm mapping: {other:?}"),
    }
}

#[test]
fn given_ambient_lux_signal_when_projected_then_maps_to_fsm_ambient_lux() {
    let projector = PhysicalToDigitalProjector;
    let out = projector
        .project(PhysicalCarVocabulary::TelemetryUpdate(VssSignal::AmbientLux(28)))
        .expect("ambient lux projection must succeed");
    match out {
        DigitalTwinCarVocabulary::Fsm(FsmEvent::UpdateAmbientLux(v)) => assert_eq!(v, 28),
        other => panic!("unexpected ambient lux mapping: {other:?}"),
    }
}

#[test]
fn given_front_headlamp_on_confirmed_when_projected_then_maps_to_fsm() {
    let projector = PhysicalToDigitalProjector;
    let out = projector
        .project(PhysicalCarVocabulary::FrontHeadlampCommandConfirmed { on_command: true })
        .expect("on ack projection must succeed");
    match out {
        DigitalTwinCarVocabulary::Fsm(FsmEvent::FrontHeadlampOnAck) => {}
        other => panic!("unexpected on ack mapping: {other:?}"),
    }
}

#[test]
fn given_front_headlamp_off_confirmed_when_projected_then_maps_to_fsm() {
    let projector = PhysicalToDigitalProjector;
    let out = projector
        .project(PhysicalCarVocabulary::FrontHeadlampCommandConfirmed { on_command: false })
        .expect("off ack projection must succeed");
    match out {
        DigitalTwinCarVocabulary::Fsm(FsmEvent::FrontHeadlampOffAck) => {}
        other => panic!("unexpected off ack mapping: {other:?}"),
    }
}

#[test]
fn given_front_headlamp_rejected_when_projected_then_maps_to_incomplete_with_negative_ack() {
    let projector = PhysicalToDigitalProjector;
    let out = projector
        .project(PhysicalCarVocabulary::FrontHeadlampCommandRejected { on_command: true })
        .expect("reject projection must succeed");
    match out {
        DigitalTwinCarVocabulary::Fsm(FsmEvent::FrontHeadlampActuationIncomplete {
            direction,
            cause,
        }) => {
            assert!(matches!(direction, crate::fsm::FrontHeadlampSwitchDirection::On));
            assert!(matches!(cause, crate::fsm::FrontHeadlampIncompleteCause::NegativeAck));
        }
        other => panic!("unexpected rejected mapping: {other:?}"),
    }
}
