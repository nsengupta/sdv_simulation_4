//! Lighting operational detector: driving in the dark without a confirmed ON lamp.
//!
//! Threshold: [`LUX_ON_THRESHOLD`] from [`crate::vehicle_physics`] (same as L1 headlamp zone).
//! Target layout: `fsm/detectors/` + per-state table slot — see ADR-7 § deferred.

use crate::fsm::{FsmEvent, FsmState, Operational};
use crate::vehicle_physics::LUX_ON_THRESHOLD;
use crate::vehicle_state::{HeadlampState, VehicleContext};

/// Exit cut after a hop → synthesize `Internal(LightingUnsafe)` when all guards pass.
///
/// Guards (ADR-7 step 7a confirmation #1):
/// - operational mode is **Driving** (not Idle/Off/latched danger/warning),
/// - `ambient_lux <= LUX_ON_THRESHOLD`,
/// - headlamp physical lamp is dark: state is `Off` (assembly not started) or `Ready`
///   (assembly active but no lux-triggered ON command received yet).
pub fn lighting_unsafe_detector(
    exit_state: &FsmState,
    exit_ctx: &VehicleContext,
) -> Option<FsmEvent> {
    if *exit_state != FsmState::Driving {
        return None;
    }
    if exit_ctx.visibility.ambient_lux > LUX_ON_THRESHOLD {
        return None;
    }
    if !matches!(exit_ctx.headlamp.state, HeadlampState::Off | HeadlampState::Ready) {
        return None;
    }
    Some(FsmEvent::Internal(Operational::LightingUnsafe))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vehicle_physics::RPM_DRIVING_THRESHOLD;
    use std::time::Instant;

    fn ctx_at(lux: u16, lamp: HeadlampState) -> VehicleContext {
        let mut ctx = VehicleContext::default();
        ctx.visibility.ambient_lux = lux;
        ctx.powertrain.apply_rpm(RPM_DRIVING_THRESHOLD + 200);
        ctx.powertrain.refresh_speed();
        ctx.headlamp.state = lamp;
        ctx
    }

    #[derive(Debug)]
    struct Case {
        state: FsmState,
        lux: u16,
        lamp: HeadlampState,
        expect: bool,
    }

    const NEGATIVE: &[Case] = &[
        Case {
            state: FsmState::Idle,
            lux: 20,
            lamp: HeadlampState::Off,
            expect: false,
        },
        Case {
            state: FsmState::Off,
            lux: 20,
            lamp: HeadlampState::Off,
            expect: false,
        },
        Case {
            state: FsmState::DrivingDangerously,
            lux: 20,
            lamp: HeadlampState::Off,
            expect: false,
        },
        Case {
            state: FsmState::Driving,
            lux: LUX_ON_THRESHOLD + 1,
            lamp: HeadlampState::Off,
            expect: false,
        },
        Case {
            state: FsmState::Driving,
            lux: 20,
            lamp: HeadlampState::OnRequested,
            expect: false,
        },
        Case {
            state: FsmState::Driving,
            lux: 20,
            lamp: HeadlampState::On,
            expect: false,
        },
        Case {
            state: FsmState::Driving,
            lux: 20,
            lamp: HeadlampState::OffRequested,
            expect: false,
        },
    ];

    #[test]
    fn lighting_unsafe_detector_negative_matrix() {
        for case in NEGATIVE {
            let ctx = ctx_at(case.lux, case.lamp);
            let got = lighting_unsafe_detector(&case.state, &ctx).is_some();
            assert_eq!(got, case.expect, "case={case:?}");
        }
        let ctx = ctx_at(20, HeadlampState::Off);
        assert!(
            lighting_unsafe_detector(&FsmState::ExtremeOperationWarning(Instant::now()), &ctx).is_none()
        );
    }

    #[test]
    fn lighting_unsafe_detector_fires_on_driving_dark_off() {
        let ctx = ctx_at(20, HeadlampState::Off);
        assert!(matches!(
            lighting_unsafe_detector(&FsmState::Driving, &ctx),
            Some(FsmEvent::Internal(Operational::LightingUnsafe))
        ));
    }

    #[test]
    fn lighting_unsafe_detector_inclusive_lux_on_threshold() {
        let ctx = ctx_at(LUX_ON_THRESHOLD, HeadlampState::Off);
        assert!(lighting_unsafe_detector(&FsmState::Driving, &ctx).is_some());
    }

    #[test]
    fn lighting_unsafe_detector_fires_on_driving_dark_ready() {
        // Ready = assembly active but physical lamp dark; unsafe while driving.
        let ctx = ctx_at(20, HeadlampState::Ready);
        assert!(matches!(
            lighting_unsafe_detector(&FsmState::Driving, &ctx),
            Some(FsmEvent::Internal(Operational::LightingUnsafe))
        ));
    }

    #[test]
    fn lighting_unsafe_detector_does_not_fire_when_ready_but_bright() {
        let ctx = ctx_at(LUX_ON_THRESHOLD + 1, HeadlampState::Ready);
        assert!(lighting_unsafe_detector(&FsmState::Driving, &ctx).is_none());
    }
}
