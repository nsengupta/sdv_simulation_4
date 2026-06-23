//! Contract tests for `PreparingToStart` / `PreparingToStop` FSM states and their
//! associated vocabulary (`StartAssemblies`, `StopAssemblies`).
//!
//! Phase 9 renames the variants to tuple style and moves the countdown into the state
//! itself (`BTreeSet<AssemblyId>`), eliminating `VehicleContext::remaining_assemblies`.

use std::collections::BTreeSet;
use std::time::Instant;

use crate::fsm::{step, transition, DomainAction, FsmEvent, FsmState, AssemblyId};
use crate::twin_runtime::zone_turn::zone_message_for_event;
use crate::vehicle_state::{HeadlampMessage, VehicleContext};

fn ctx() -> VehicleContext {
    VehicleContext::default()
}

/// A state that has only `AssemblyId::Headlamp` remaining — equivalent to
/// "one assembly left to ack before the transition fires."
fn preparing_to_start_headlamp_only() -> FsmState {
    FsmState::PreparingToStart(BTreeSet::from([AssemblyId::Headlamp]))
}

fn preparing_to_stop_headlamp_only() -> FsmState {
    FsmState::PreparingToStop(BTreeSet::from([AssemblyId::Headlamp]))
}

// --- Transition table tests ---

#[test]
fn test_power_on_transitions_to_preparing_to_start() {
    let result = transition(&FsmState::Off, &FsmEvent::PowerOn, &ctx(), Instant::now());
    assert!(matches!(result.next_state, FsmState::PreparingToStart(_)));
}

#[test]
fn test_assemblies_ready_from_preparing_to_start_transitions_to_idle() {
    // State has only Headlamp remaining; AssemblyZoneReady(Headlamp) drains it → Idle.
    let result = transition(
        &preparing_to_start_headlamp_only(),
        &FsmEvent::AssemblyZoneReady(AssemblyId::Headlamp),
        &ctx(),
        Instant::now(),
    );
    assert_eq!(result.next_state, FsmState::Idle);
}

#[test]
fn test_power_off_from_idle_transitions_to_preparing_to_stop() {
    let result = transition(&FsmState::Idle, &FsmEvent::PowerOff, &ctx(), Instant::now());
    assert!(matches!(result.next_state, FsmState::PreparingToStop(_)));
}

#[test]
fn test_assemblies_stopped_from_preparing_to_stop_transitions_to_off() {
    // State has only Headlamp remaining; AssemblyZoneReady(Headlamp) drains it → Off.
    let result = transition(
        &preparing_to_stop_headlamp_only(),
        &FsmEvent::AssemblyZoneReady(AssemblyId::Headlamp),
        &ctx(),
        Instant::now(),
    );
    assert_eq!(result.next_state, FsmState::Off);
}

#[test]
fn test_external_rpm_event_is_no_op_during_preparing_to_start() {
    let result = transition(
        &FsmState::PreparingToStart(BTreeSet::new()),
        &FsmEvent::UpdateRpm(3000),
        &ctx(),
        Instant::now(),
    );
    assert!(matches!(result.next_state, FsmState::PreparingToStart(_)));
    assert!(result.note.is_none(), "no RejectedPowerOff note expected");
}

#[test]
fn test_external_lux_event_is_no_op_during_preparing_to_stop() {
    let result = transition(
        &FsmState::PreparingToStop(BTreeSet::new()),
        &FsmEvent::UpdateAmbientLux(10),
        &ctx(),
        Instant::now(),
    );
    assert!(matches!(result.next_state, FsmState::PreparingToStop(_)));
    assert!(result.note.is_none(), "no RejectedPowerOff note expected");
}

// --- DomainAction emission tests (via `step`, which maps FsmAction → DomainAction) ---

#[test]
fn test_start_assemblies_action_emitted_on_power_on() {
    let result = step(&FsmState::Off, &ctx(), &FsmEvent::PowerOn, Instant::now());
    assert!(matches!(result.next_state, FsmState::PreparingToStart(_)));
    assert!(
        result.actions.iter().any(|a| matches!(a, DomainAction::StartAssemblies(_))),
        "StartAssemblies must be in the action feed; got: {:?}",
        result.actions
    );
}

#[test]
fn test_stop_assemblies_action_emitted_on_power_off_from_idle() {
    let result = step(&FsmState::Idle, &ctx(), &FsmEvent::PowerOff, Instant::now());
    assert!(matches!(result.next_state, FsmState::PreparingToStop(_)));
    assert!(
        result.actions.iter().any(|a| matches!(a, DomainAction::StopAssemblies(_))),
        "StopAssemblies must be in the action feed; got: {:?}",
        result.actions
    );
}

// --- zone_message_for_event state-aware routing tests ---

#[test]
fn test_zone_message_for_event_returns_none_during_preparing_to_start() {
    let event = FsmEvent::UpdateAmbientLux(10);
    let result = zone_message_for_event(&event, &FsmState::PreparingToStart(BTreeSet::new()));
    assert!(
        result.is_none(),
        "zone_message_for_event must return None during PreparingToStart; got {result:?}"
    );
}

#[test]
fn test_zone_message_for_event_returns_none_during_preparing_to_stop() {
    let event = FsmEvent::UpdateAmbientLux(10);
    let result = zone_message_for_event(&event, &FsmState::PreparingToStop(BTreeSet::new()));
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
        Some((AssemblyId::Headlamp, ZoneMessage::Headlamp(HeadlampMessage::AmbientLux(10)))) => {}
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
            .all(|a| !matches!(a, DomainAction::StartAssemblies(_))),
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
            .all(|a| !matches!(a, DomainAction::StopAssemblies(_))),
        "StopAssemblies must NOT appear in the ledger record; got: {:?}",
        result.transition_record.actions
    );
}

// ── Phase 9 tests — shrinking BTreeSet countdown ──────────────────────────────────────

#[test]
fn test_preparing_to_start_carries_assembly_ids() {
    let result = step(&FsmState::Off, &ctx(), &FsmEvent::PowerOn, Instant::now());
    let FsmState::PreparingToStart(remaining) = &result.next_state else {
        panic!("expected PreparingToStart, got {:?}", result.next_state);
    };
    assert!(remaining.contains(&AssemblyId::Headlamp), "Headlamp must be in the remaining set");
    assert!(remaining.contains(&AssemblyId::Wiper), "Wiper must be in the remaining set");
}

#[test]
fn test_preparing_to_stop_carries_assembly_ids() {
    let result = step(&FsmState::Idle, &ctx(), &FsmEvent::PowerOff, Instant::now());
    let FsmState::PreparingToStop(remaining) = &result.next_state else {
        panic!("expected PreparingToStop, got {:?}", result.next_state);
    };
    assert!(remaining.contains(&AssemblyId::Headlamp));
    assert!(remaining.contains(&AssemblyId::Wiper));
}

#[test]
fn test_state_and_action_agree_on_assembly_set() {
    // The FSM state's inner set and the StartAssemblies action payload must name
    // the same assemblies — both derived from ALL_ASSEMBLIES.
    let result = step(&FsmState::Off, &ctx(), &FsmEvent::PowerOn, Instant::now());
    let FsmState::PreparingToStart(state_set) = &result.next_state else {
        panic!("expected PreparingToStart");
    };
    let action_list = result
        .actions
        .iter()
        .find_map(|a| if let DomainAction::StartAssemblies(list) = a { Some(list.clone()) } else { None })
        .expect("StartAssemblies action must be present after PowerOn");
    let action_set: BTreeSet<AssemblyId> = action_list.into_iter().collect();
    assert_eq!(state_set, &action_set, "state set and action payload must agree");
}

#[test]
fn test_assembly_zone_ready_shrinks_state_not_context() {
    // After PowerOn: PreparingToStart({Headlamp, Wiper}).
    // After AssemblyZoneReady(Headlamp): PreparingToStart({Wiper}).
    // The countdown lives in the state; VehicleContext carries no separate field.
    let power_on = step(&FsmState::Off, &ctx(), &FsmEvent::PowerOn, Instant::now());
    let FsmState::PreparingToStart(after_power_on) = &power_on.next_state else {
        panic!("expected PreparingToStart after PowerOn");
    };
    assert_eq!(after_power_on.len(), 2, "two assemblies pending after PowerOn");

    let headlamp_ready = step(
        &power_on.next_state,
        &power_on.modified_ctx,
        &FsmEvent::AssemblyZoneReady(AssemblyId::Headlamp),
        Instant::now(),
    );
    let FsmState::PreparingToStart(after_headlamp) = &headlamp_ready.next_state else {
        panic!("expected PreparingToStart after first AssemblyZoneReady");
    };
    assert_eq!(
        after_headlamp,
        &BTreeSet::from([AssemblyId::Wiper]),
        "only Wiper remains after Headlamp clears"
    );
}

#[test]
fn test_start_assemblies_action_carries_assembly_list() {
    let result = step(&FsmState::Off, &ctx(), &FsmEvent::PowerOn, Instant::now());
    let list = result
        .actions
        .iter()
        .find_map(|a| if let DomainAction::StartAssemblies(list) = a { Some(list.clone()) } else { None })
        .expect("StartAssemblies must be in actions after PowerOn");
    assert!(list.contains(&AssemblyId::Headlamp));
    assert!(list.contains(&AssemblyId::Wiper));
}
