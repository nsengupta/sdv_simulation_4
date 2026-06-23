//! Phase 1 contract tests: `PreparingToStart` / `PreparingToStop` FSM states and their
//! associated vocabulary (`AssembliesReady`, `AssembliesStopped`, `StartAssemblies`,
//! `StopAssemblies`).
//!
//! These tests are written RED-first: every assertion references new symbols that do not
//! yet exist in the production code. They must fail to compile until Phase 1 lands.

use std::collections::BTreeSet;
use std::time::Instant;

use crate::fsm::{step, transition, DomainAction, FsmEvent, FsmState, ZoneId};
use crate::twin_runtime::zone_turn::zone_message_for_event;
use crate::vehicle_state::{HeadlampMessage, VehicleContext};

fn ctx() -> VehicleContext {
    VehicleContext::default()
}

/// A context that has `ZoneId::Headlamp` listed as a pending assembly — the state the
/// FSM enters immediately after `Off → PreparingToStart` or `Idle → PreparingToStop`.
fn ctx_with_headlamp_pending() -> VehicleContext {
    let mut c = VehicleContext::default();
    c.pending_assemblies = BTreeSet::from([ZoneId::Headlamp]);
    c
}

// --- Transition table tests ---

#[test]
fn test_power_on_transitions_to_preparing_to_start() {
    let result = transition(&FsmState::Off, &FsmEvent::PowerOn, &ctx(), Instant::now());
    assert_eq!(result.next_state, FsmState::PreparingToStart);
}

#[test]
fn test_assemblies_ready_from_preparing_to_start_transitions_to_idle() {
    // AssemblyZoneReady(Headlamp) with the last pending assembly → Idle.
    let result = transition(
        &FsmState::PreparingToStart,
        &FsmEvent::AssemblyZoneReady(ZoneId::Headlamp),
        &ctx_with_headlamp_pending(),
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
    // AssemblyZoneReady(Headlamp) with the last pending assembly → Off.
    let result = transition(
        &FsmState::PreparingToStop,
        &FsmEvent::AssemblyZoneReady(ZoneId::Headlamp),
        &ctx_with_headlamp_pending(),
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

// --- zone_message_for_event state-aware routing tests ---

#[test]
fn test_zone_message_for_event_returns_none_during_preparing_to_start() {
    let event = FsmEvent::UpdateAmbientLux(10);
    let result = zone_message_for_event(&event, &FsmState::PreparingToStart);
    assert!(
        result.is_none(),
        "zone_message_for_event must return None during PreparingToStart; got {result:?}"
    );
}

#[test]
fn test_zone_message_for_event_returns_none_during_preparing_to_stop() {
    let event = FsmEvent::UpdateAmbientLux(10);
    let result = zone_message_for_event(&event, &FsmState::PreparingToStop);
    assert!(
        result.is_none(),
        "zone_message_for_event must return None during PreparingToStop; got {result:?}"
    );
}

#[test]
fn test_zone_message_for_event_returns_some_during_idle() {
    use crate::digital_twin::ZoneMessage;
    let event = FsmEvent::UpdateAmbientLux(10);
    let result = zone_message_for_event(&event, &FsmState::Idle);
    match result {
        Some((crate::fsm::ZoneId::Headlamp, ZoneMessage::Headlamp(HeadlampMessage::AmbientLux(10)))) => {}
        other => panic!(
            "zone_message_for_event must return Some((Headlamp, Headlamp(AmbientLux(10)))) when Idle; got {other:?}"
        ),
    }
}

// --- Phase 7: RainsStarted self-loop in Idle ---

#[test]
fn test_rains_started_is_self_loop_in_idle() {
    let result = step(&FsmState::Idle, &ctx(), &FsmEvent::RainsStarted, Instant::now());
    assert_eq!(
        result.next_state,
        FsmState::Idle,
        "RainsStarted must be a self-loop in Idle (zone handles it, FSM state unchanged)"
    );
    assert!(
        result.actions.is_empty(),
        "RainsStarted in Idle must produce no FSM domain actions; got: {:?}",
        result.actions
    );
}

// --- Ledger record exclusion tests ---
//
// `StartAssemblies` / `StopAssemblies` are internal coordination signals,
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
