use common::{PhysicalCarVocabulary, VehicleEvent};

/// Maps ingress/domain events to the canonical physical vocabulary.
pub fn vehicle_event_to_physical_vocabulary(ev: VehicleEvent) -> PhysicalCarVocabulary {
    match ev {
        VehicleEvent::TelemetryUpdate(vss) => PhysicalCarVocabulary::TelemetryUpdate(vss),
        VehicleEvent::TimerTick => PhysicalCarVocabulary::TimerTick,
        VehicleEvent::SystemReset => PhysicalCarVocabulary::SystemReset,
    }
}

#[cfg(test)]
mod tests {
    use super::vehicle_event_to_physical_vocabulary;
    use common::{PhysicalCarVocabulary, VehicleEvent, VssSignal};

    #[test]
    fn given_timer_tick_when_mapped_then_returns_physical_timer_tick() {
        let msg = vehicle_event_to_physical_vocabulary(VehicleEvent::TimerTick);
        match msg {
            PhysicalCarVocabulary::TimerTick => {}
            other => panic!("unexpected mapping: {other:?}"),
        }
    }

    #[test]
    fn given_system_reset_when_mapped_then_returns_physical_reset() {
        let msg = vehicle_event_to_physical_vocabulary(VehicleEvent::SystemReset);
        match msg {
            PhysicalCarVocabulary::SystemReset => {}
            other => panic!("unexpected mapping: {other:?}"),
        }
    }

    #[test]
    fn given_engine_rpm_signal_when_mapped_then_preserves_value() {
        let msg = vehicle_event_to_physical_vocabulary(VehicleEvent::TelemetryUpdate(
            VssSignal::EngineRpm(4567),
        ));
        match msg {
            PhysicalCarVocabulary::TelemetryUpdate(VssSignal::EngineRpm(v)) => {
                assert_eq!(v, 4567)
            }
            other => panic!("unexpected rpm mapping: {other:?}"),
        }
    }

    #[test]
    fn given_ambient_lux_signal_when_mapped_then_preserves_value() {
        let msg = vehicle_event_to_physical_vocabulary(VehicleEvent::TelemetryUpdate(
            VssSignal::AmbientLux(33),
        ));
        match msg {
            PhysicalCarVocabulary::TelemetryUpdate(VssSignal::AmbientLux(v)) => {
                assert_eq!(v, 33)
            }
            other => panic!("unexpected ambient lux mapping: {other:?}"),
        }
    }
}
