//! Unit tests for the FSM step contract (`step`).

use std::collections::BTreeSet;
use crate::digital_twin::{verify_state_laws, DigitalTwinCar, DigitalTwinCarError};
use crate::fsm::{DomainAction, FsmEvent, FsmState, ZoneId};
use crate::twin_runtime::twin_turn;
use crate::vehicle_state::VehicleContext;

fn ctx_with_headlamp_pending() -> VehicleContext {
    let mut c = VehicleContext::default();
    c.pending_assemblies = BTreeSet::from([ZoneId::Headlamp]);
    c
}
use crate::vehicle_physics::{
    EXTREME_OPERATION_WARNING_MESSAGE, SPEED_THRESHOLD_WARNING_MESSAGE,
};
use std::time::{Duration, Instant};

fn valid_twin_context() -> VehicleContext {
    VehicleContext::default()
}

#[test]
fn test_step_derive_ctx_and_warning_flow() {
    let mut current_ctx = valid_twin_context();
    let mut current_state = FsmState::Idle;

    let warmup = twin_turn(
        &current_state,
        &current_ctx,
        &FsmEvent::UpdateRpm(1200),
        Instant::now(),
    );
    assert_eq!(warmup.next_state, FsmState::Driving);
    assert_eq!(warmup.modified_ctx.powertrain.wheel_rpm.front_left, 1200);

    current_state = warmup.next_state;
    current_ctx = warmup.modified_ctx;

    let warning = twin_turn(
        &current_state,
        &current_ctx,
        &FsmEvent::UpdateRpm(5600),
        Instant::now(),
    );
    assert_eq!(warning.modified_ctx.powertrain.wheel_rpm.front_left, 5600);
    assert!(matches!(
        warning.next_state,
        FsmState::ExtremeOperationWarning(_)
    ));
    assert!(warning.actions.contains(&DomainAction::StartBuzzer));
    assert!(warning.actions.contains(&DomainAction::LogWarning(
        SPEED_THRESHOLD_WARNING_MESSAGE.to_string()
    )));
    assert!(warning.actions.contains(&DomainAction::LogWarning(
        EXTREME_OPERATION_WARNING_MESSAGE.to_string()
    )));
}

#[test]
fn test_transition_record_carries_intended_actions_without_enter_mode() {
    // Driving + redline RPM enters ExtremeOperationWarning, which emits StartBuzzer (+
    // warnings) plus an EnterMode hint for the actor.
    let result = twin_turn(
        &FsmState::Driving,
        &valid_twin_context(),
        &FsmEvent::UpdateRpm(5600),
        Instant::now(),
    );

    // The execution feed keeps EnterMode (the actor consumes it to set its mode).
    assert!(result
        .actions
        .iter()
        .any(|action| matches!(action, DomainAction::EnterMode(_))));

    // The ledger projection records the genuine domain intents but drops EnterMode.
    let recorded = &result.transition_record.actions;
    assert!(recorded.contains(&DomainAction::StartBuzzer));
    assert!(recorded
        .iter()
        .all(|action| !matches!(action, DomainAction::EnterMode(_))));

    // Lossless otherwise: record == execution feed minus EnterMode.
    let expected: Vec<DomainAction> = result
        .actions
        .iter()
        .filter(|action| !matches!(action, DomainAction::EnterMode(_)))
        .cloned()
        .collect();
    assert_eq!(recorded, &expected);
}

#[test]
fn test_step_high_speed_below_rpm_threshold_still_warns_on_speed() {
    let current_ctx = valid_twin_context();
    let current_state = FsmState::Driving;

    let result = twin_turn(
        &current_state,
        &current_ctx,
        &FsmEvent::UpdateRpm(3600),
        Instant::now(),
    );
    assert_eq!(result.modified_ctx.powertrain.wheel_rpm.front_left, 3600);
    assert!(matches!(
        result.next_state,
        FsmState::ExtremeOperationWarning(_)
    ));
    assert!(result.actions.contains(&DomainAction::LogWarning(
        SPEED_THRESHOLD_WARNING_MESSAGE.to_string()
    )));
    assert!(!result
        .actions
        .contains(&DomainAction::LogWarning(EXTREME_OPERATION_WARNING_MESSAGE.to_string())));
}

#[test]
fn test_step_standard_commute_flow() {
    let mut car = DigitalTwinCar::new("NASHIK-VC-001", FsmState::Off, valid_twin_context())
        .expect("non-blank identity");

    let sequence: Vec<(FsmEvent, FsmState)> = vec![
        (FsmEvent::PowerOn, FsmState::PreparingToStart),
        // AssemblyZoneReady drains the startup barrier; `step` removes Headlamp from pending.
        (FsmEvent::AssemblyZoneReady(ZoneId::Headlamp), FsmState::Idle),
        (FsmEvent::UpdateRpm(1500), FsmState::Driving),
        (FsmEvent::UpdateRpm(1300), FsmState::Driving),
        (FsmEvent::UpdateRpm(0), FsmState::Idle),
        (FsmEvent::PowerOff, FsmState::PreparingToStop),
        // AssemblyZoneReady drains the shutdown barrier.
        (FsmEvent::AssemblyZoneReady(ZoneId::Headlamp), FsmState::Off),
    ];

    for (event, expected_state) in sequence {
        // For AssemblyZoneReady events, supply a context with Headlamp pending so the
        // transition table can compute the countdown correctly.
        let ctx_for_event = if matches!(event, FsmEvent::AssemblyZoneReady(_)) {
            ctx_with_headlamp_pending()
        } else {
            car.context().clone()
        };
        let result = twin_turn(car.current_state(), &ctx_for_event, &event, Instant::now());
        car.apply_step(result.next_state, result.modified_ctx);
        assert_eq!(*car.current_state(), expected_state, "event={event:?}");
    }
}

#[test]
fn test_state_laws_hold_over_a_legal_journey_and_records_carry_intents() {
    // Demonstrates the intended external-verifier usage: fold the pure `verify_state_laws`
    // primitive over each captured `(state, ctx)` cut of a journey. The library ships no
    // journey-fold helper (that consumer-side concern lives outside the twin — see ADR-1/-3);
    // a verifier/offline tool folds the primitive itself, exactly like this.
    let mut state = FsmState::Off;
    let mut ctx = valid_twin_context();
    let mut reached_warning = false;

    // Phase 1: PowerOn bridges via PreparingToStart before Idle.
    // After PowerOn, step() initialises pending_assemblies={Headlamp}, so the next
    // twin_turn call sees the correct context for AssemblyZoneReady.
    for event in [
        FsmEvent::PowerOn,
        FsmEvent::AssemblyZoneReady(ZoneId::Headlamp),
        FsmEvent::UpdateRpm(1500),
        FsmEvent::UpdateRpm(5600),
    ] {
        let result = twin_turn(&state, &ctx, &event, Instant::now());

        // Every cut the journey passes through satisfies the state laws.
        assert!(
            verify_state_laws(&result.next_state, &result.modified_ctx).is_ok(),
            "legal journey must not breach any state law at event={event:?}"
        );

        // Records carry intents (WI-1): entering ExtremeOperationWarning emits StartBuzzer.
        if matches!(result.next_state, FsmState::ExtremeOperationWarning(_)) {
            assert!(result.transition_record.actions.contains(&DomainAction::StartBuzzer));
            reached_warning = true;
        }

        state = result.next_state;
        ctx = result.modified_ctx;
    }

    assert!(reached_warning, "journey should reach ExtremeOperationWarning");
}

#[test]
fn test_blank_identity_is_unconstructable() {
    // A twin with a blank identity is no longer a representable value: construction is the
    // only way in, and it rejects empty / whitespace-only identities (the old runtime check
    // in verify_all_invariants is now structurally dead).
    assert_eq!(
        DigitalTwinCar::new("", FsmState::Off, valid_twin_context()).unwrap_err(),
        DigitalTwinCarError::BlankIdentity
    );
    assert_eq!(
        DigitalTwinCar::new("   ", FsmState::Off, valid_twin_context()).unwrap_err(),
        DigitalTwinCarError::BlankIdentity
    );

    // A non-blank identity is stored trimmed.
    let car = DigitalTwinCar::new("  VC-7  ", FsmState::Off, valid_twin_context())
        .expect("non-blank identity");
    assert_eq!(car.identity(), "VC-7");
}

#[test]
fn test_state_laws_flag_an_illegal_cut() {
    // The pure primitive an external verifier relies on: a Driving cut with sub-stall RPM
    // breaches `rpm_above_threshold`, reported by name.
    let illegal_ctx = {
        let mut c = valid_twin_context();
        c.powertrain.wheel_rpm.front_left = 100; // below RPM_DRIVING_THRESHOLD
        c
    };

    let violations = verify_state_laws(&FsmState::Driving, &illegal_ctx)
        .expect_err("Driving with sub-stall RPM must breach a law");
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].law, "rpm_above_threshold");
}

#[test]
fn test_step_warning_recovery_on_tick_uses_passed_time() {
    let base = Instant::now();
    let ctx = {
        let mut c = valid_twin_context();
        c.powertrain.wheel_rpm.front_left = 1000;
        c.powertrain.refresh_speed();
        c
    };

    let warning_state = FsmState::ExtremeOperationWarning(base);

    let early = twin_turn(
        &warning_state,
        &ctx,
        &FsmEvent::TimerTick,
        base + Duration::from_secs(2),
    );
    assert!(matches!(
        early.next_state,
        FsmState::ExtremeOperationWarning(_)
    ));

    let recovered = twin_turn(
        &warning_state,
        &ctx,
        &FsmEvent::TimerTick,
        base + Duration::from_secs(6),
    );
    assert_eq!(recovered.next_state, FsmState::Driving);
    assert!(recovered.actions.contains(&DomainAction::StopBuzzer));
}
