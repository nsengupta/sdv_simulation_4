//! Phase 1 contract tests: `PreparingToStart` / `PreparingToStop` FSM states and their
//! associated vocabulary (`AssembliesReady`, `AssembliesStopped`, `StartAssemblies`,
//! `StopAssemblies`).
//!
//! These tests are written RED-first: every assertion references new symbols that do not
//! yet exist in the production code. They must fail to compile until Phase 1 lands.

use crate::fsm::{step, transition, DomainAction, FsmEvent, FsmState, Operational};
use crate::vehicle_state::VehicleContext;
use std::time::Instant;

fn ctx() -> VehicleContext {
    VehicleContext::default()
}

// --- Transition table tests ---

#[test]
fn test_power_on_transitions_to_preparing_to_start() {
    let result = transition(&FsmState::Off, &FsmEvent::PowerOn, &ctx(), Instant::now());
    assert_eq!(result.next_state, FsmState::PreparingToStart);
}

#[test]
fn test_assemblies_ready_from_preparing_to_start_transitions_to_idle() {
    let result = transition(
        &FsmState::PreparingToStart,
        &FsmEvent::Internal(Operational::AssembliesReady),
        &ctx(),
        Instant::now(),
    );
    assert_eq!(result.next_state, FsmState::Idle);
}

#[test]
fn test_power_off_from_idle_transitions_to_preparing_to_stop() {
    let result = transition(&FsmState::Idle, &FsmEvent::PowerOff, &ctx(), Instant::now());
    assert_eq!(result.next_state, FsmState::PreparingToStop);
}

#[test]
fn test_assemblies_stopped_from_preparing_to_stop_transitions_to_off() {
    let result = transition(
        &FsmState::PreparingToStop,
        &FsmEvent::Internal(Operational::AssembliesStopped),
        &ctx(),
        Instant::now(),
    );
    assert_eq!(result.next_state, FsmState::Off);
}

#[test]
fn test_external_rpm_event_is_no_op_during_preparing_to_start() {
    let result = transition(
        &FsmState::PreparingToStart,
        &FsmEvent::UpdateRpm(3000),
        &ctx(),
        Instant::now(),
    );
    assert_eq!(result.next_state, FsmState::PreparingToStart);
    assert!(result.note.is_none(), "no RejectedPowerOff note expected");
}

#[test]
fn test_external_lux_event_is_no_op_during_preparing_to_stop() {
    let result = transition(
        &FsmState::PreparingToStop,
        &FsmEvent::UpdateAmbientLux(10),
        &ctx(),
        Instant::now(),
    );
    assert_eq!(result.next_state, FsmState::PreparingToStop);
    assert!(result.note.is_none(), "no RejectedPowerOff note expected");
}

// --- DomainAction emission tests (via `step`, which maps FsmAction → DomainAction) ---

#[test]
fn test_start_assemblies_action_emitted_on_power_on() {
    let result = step(&FsmState::Off, &ctx(), &FsmEvent::PowerOn, Instant::now());
    assert_eq!(result.next_state, FsmState::PreparingToStart);
    assert!(
        result.actions.contains(&DomainAction::StartAssemblies),
        "StartAssemblies must be in the action feed; got: {:?}",
        result.actions
    );
}

#[test]
fn test_stop_assemblies_action_emitted_on_power_off_from_idle() {
    let result = step(&FsmState::Idle, &ctx(), &FsmEvent::PowerOff, Instant::now());
    assert_eq!(result.next_state, FsmState::PreparingToStop);
    assert!(
        result.actions.contains(&DomainAction::StopAssemblies),
        "StopAssemblies must be in the action feed; got: {:?}",
        result.actions
    );
}

// --- Ledger record exclusion tests ---
//
// `StartAssemblies` / `StopAssemblies` are internal coordination signals (like `EnterMode`),
// not domain intents. They must be excluded from `transition_record.actions`.

#[test]
fn test_start_assemblies_excluded_from_ledger_record() {
    let result = step(&FsmState::Off, &ctx(), &FsmEvent::PowerOn, Instant::now());
    assert!(
        result
            .transition_record
            .actions
            .iter()
            .all(|a| !matches!(a, DomainAction::StartAssemblies)),
        "StartAssemblies must NOT appear in the ledger record; got: {:?}",
        result.transition_record.actions
    );
}

#[test]
fn test_stop_assemblies_excluded_from_ledger_record() {
    let result = step(&FsmState::Idle, &ctx(), &FsmEvent::PowerOff, Instant::now());
    assert!(
        result
            .transition_record
            .actions
            .iter()
            .all(|a| !matches!(a, DomainAction::StopAssemblies)),
        "StopAssemblies must NOT appear in the ledger record; got: {:?}",
        result.transition_record.actions
    );
}
