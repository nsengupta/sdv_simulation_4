//! Property-based tests for the FSM (`proptest` feature).

use proptest::prelude::*;
use std::time::{Duration, Instant};

use crate::fsm::{step, transition, DomainAction, FsmEvent, FsmState, LightingState, VehicleContext};
use crate::vehicle_constants::{LUX_OFF_THRESHOLD, LUX_ON_THRESHOLD};
use crate::vehicle_kinematics::refresh_context_speed;

fn ctx_with_rpm(rpm: u16) -> VehicleContext {
    let mut ctx = VehicleContext {
        rpm,
        ..VehicleContext::default()
    };
    refresh_context_speed(&mut ctx);
    ctx
}

prop_compose! {
    fn arb_context()(rpm in 0..8000u16) -> VehicleContext {
        let mut ctx = VehicleContext {
            rpm,
            fuel_level: 85,
            oil_pressure: 30,
            tyre_pressure_ok: true,
            ambient_lux: 100,
            lighting_state: LightingState::Off,
            ..VehicleContext::default()
        };
        refresh_context_speed(&mut ctx);
        ctx
    }
}

fn arb_fsm_event() -> impl Strategy<Value = FsmEvent> {
    // Avoid `Just(FsmEvent::...)` so this stays valid if `FsmEvent` ever drops `Clone` again.
    (0u8..5u8, any::<u16>()).prop_map(|(tag, rpm)| match tag {
        0 => FsmEvent::PowerOn,
        1 => FsmEvent::PowerOff,
        2 => FsmEvent::TimerTick,
        3 => FsmEvent::UpdateRpm(rpm),
        _ => FsmEvent::UpdateAmbientLux(100),
    })
}

fn arb_fsm_state() -> impl Strategy<Value = FsmState> {
    prop_oneof![
        Just(FsmState::Off),
        Just(FsmState::Idle),
        Just(FsmState::Driving),
        any::<u64>().prop_map(|n| {
            FsmState::ExtremeOperationWarning(Instant::now() - Duration::from_nanos(1 + (n % 1_000_000_000)))
        }),
    ]
}

proptest! {
    #[test]
    fn test_deadband_never_emits_light_requests_when_off(
        lux in (LUX_ON_THRESHOLD + 1)..LUX_OFF_THRESHOLD
    ) {
        let ctx = VehicleContext {
            lighting_state: LightingState::Off,
            ..VehicleContext::default()
        };
        let result = step(&FsmState::Idle, &ctx, &FsmEvent::UpdateAmbientLux(lux), Instant::now());
        prop_assert!(!result.actions.contains(&DomainAction::RequestFrontHeadlampOn));
        prop_assert!(!result.actions.contains(&DomainAction::RequestFrontHeadlampOff));
        prop_assert_eq!(result.modified_ctx.lighting_state, LightingState::Off);
    }

    #[test]
    fn test_deadband_never_emits_light_requests_when_on(
        lux in (LUX_ON_THRESHOLD + 1)..LUX_OFF_THRESHOLD
    ) {
        let ctx = VehicleContext {
            lighting_state: LightingState::On,
            ..VehicleContext::default()
        };
        let result = step(&FsmState::Driving, &ctx, &FsmEvent::UpdateAmbientLux(lux), Instant::now());
        prop_assert!(!result.actions.contains(&DomainAction::RequestFrontHeadlampOn));
        prop_assert!(!result.actions.contains(&DomainAction::RequestFrontHeadlampOff));
        prop_assert_eq!(result.modified_ctx.lighting_state, LightingState::On);
    }
}

proptest! {
    #[test]
    fn test_off_to_driving_invariant(
        event in arb_fsm_event(),
        ctx in arb_context()
    ) {
        let initial_state = FsmState::Off;
        let next_state = transition(&initial_state, &event, &ctx, Instant::now());

        // INVARIANT: You can NEVER go from Off directly to Driving
        prop_assert_ne!(
            next_state.next_state,
            FsmState::Driving,
            "Illegal transition detected: Off -> Driving directly via {:?}",
            event
        );
    }

    /// **PowerOff** is only a legal transition from **`Idle`** -> **`Off`**.
    /// From any other state it is wrong input: the FSM must **not** change.
    #[test]
    fn test_power_off_only_valid_from_idle(
        state in arb_fsm_state(),
        ctx in arb_context()
    ) {
        let before = state.clone();
        let next = transition(&state, &FsmEvent::PowerOff, &ctx, Instant::now());
        if before == FsmState::Idle {
            prop_assert_eq!(next.next_state, FsmState::Off);
        } else {
            prop_assert_eq!(
                next.next_state,
                before,
                "PowerOff outside Idle is invalid; state must be unchanged"
            );
        }
    }
}

proptest! {
    #[test]
    fn test_shutdown_invariants(
        state in prop_oneof![
            Just(FsmState::Driving),
            any::<u64>().prop_map(|n| {
                FsmState::ExtremeOperationWarning(Instant::now() - Duration::from_nanos(1 + (n % 1_000_000_000)))
            }),
        ],
        _tag in 0u8..1u8,
    ) {
        let event = FsmEvent::PowerOff;
        let ctx = ctx_with_rpm(3000);
        let next = transition(&state, &event, &ctx, Instant::now());

        // INVARIANT: In these states, PowerOff MUST be ignored.
        prop_assert_eq!(next.next_state, state, "Car allowed shutdown from a moving state!");
    }
}
