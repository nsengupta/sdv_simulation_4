//! Step 7 (TDD): brain operational policy after tell-back merge.
//!
//! Spec: [`docs/milestone-actor-headlamp-scope.md`](../../docs/milestone-actor-headlamp-scope.md)
//! § Brain operational policy. Pure [`twin_turn`] stands in for post-tell-back
//! `commit_resolved_turn` + [`ZoneReplies`] on the actor path; pure tests use [`ZoneReplies::simulate_locally`].
//!
//! **Expected (not implemented yet):** when **Driving** + dark + headlamp failed to stay ON
//! (timeout → `Off` + zone `LogWarning`), brain enters **`DrivingDangerously`** and
//! **`StartBuzzer`**, latched until corrective action (lamp ON, bright lux, or stationary → Idle).

use crate::fsm::{DomainAction, FsmEvent, FsmState, HeadlampState, Operational};
use crate::twin_runtime::{run_to_quiescence, twin_turn, ZoneReplies};
use crate::vehicle_state::VehicleContext;
use crate::vehicle_physics::{
    FRONT_HEADLAMP_ON_ACK_WAIT, LUX_ON_THRESHOLD, RPM_DRIVING_THRESHOLD,
};
use std::time::Instant;

fn ctx_driving_in_dark() -> VehicleContext {
    let mut ctx = VehicleContext::default();
    ctx.visibility.ambient_lux = 20;
    ctx.powertrain.apply_rpm(RPM_DRIVING_THRESHOLD + 200);
    ctx.powertrain.refresh_speed();
    ctx
}

#[test]
fn given_driving_in_dark_when_internal_lighting_unsafe_then_l1_unchanged_and_enters_danger() {
    let t0 = Instant::now();
    let mut ctx = ctx_driving_in_dark();
    ctx.headlamp.state = HeadlampState::Off;
    let before = ctx.clone();

    let result = run_to_quiescence(
        &FsmState::Driving,
        &before,
        &FsmEvent::Internal(Operational::LightingUnsafe),
        t0,
        &ZoneReplies::simulate_locally(),
    );

    assert_eq!(result.hops.len(), 1, "single internal hop only");
    assert!(matches!(
        result.hops[0].event,
        FsmEvent::Internal(Operational::LightingUnsafe)
    ));
    assert_eq!(
        result.final_step().modified_ctx, before,
        "internal hop must not run zone_turn / mutate L1"
    );
    assert_eq!(result.final_step().next_state, FsmState::DrivingDangerously);
    assert!(
        result.final_step().actions.contains(&DomainAction::StartBuzzer),
        "table edge Driving → DrivingDangerously arms buzzer via output()"
    );
}

fn assert_no_lighting_unsafe_internal_hop(result: &crate::twin_runtime::QuiescentResult) {
    assert!(
        !result.hops.iter().any(|h| {
            matches!(h.event, FsmEvent::Internal(Operational::LightingUnsafe))
        }),
        "detector must not synthesize LightingUnsafe on this cut"
    );
}

/// ADR-7 confirmation #1: pending ON (ACK not yet settled) is not "unsafe" — lamp actuation in flight.
#[test]
fn given_driving_in_dark_when_on_requested_then_no_lighting_unsafe_internal_hop() {
    let t0 = Instant::now();
    let mut ctx = ctx_driving_in_dark();
    ctx.headlamp.state = HeadlampState::OnRequested;
    ctx.headlamp.ack_pending_since = Some(t0);

    let result = run_to_quiescence(
        &FsmState::Driving,
        &ctx,
        &FsmEvent::TimerTick,
        t0,
        &ZoneReplies::simulate_locally(),
    );

    assert_eq!(result.hops.len(), 1, "external TimerTick only — no internal hop");
    assert_eq!(result.hops[0].event, FsmEvent::TimerTick);
    assert_no_lighting_unsafe_internal_hop(&result);
    assert_eq!(result.final_step().next_state, FsmState::Driving);
    assert_eq!(
        result.final_step().modified_ctx.headlamp.state,
        HeadlampState::OnRequested,
        "zone tick before ACK timeout must not settle to Off"
    );
    assert!(
        !result.final_step().actions.contains(&DomainAction::StartBuzzer),
        "no danger mode while lamp request is pending"
    );
}

fn ctx_driving_dangerous_after_failed_on() -> VehicleContext {
    let mut ctx = ctx_driving_in_dark();
    // After a failed ON attempt (Phase 2): assembly still active but lamp dark → Ready.
    ctx.headlamp.state = HeadlampState::Ready;
    ctx.headlamp.ack_pending_since = None;
    ctx
}

#[test]
fn given_driving_in_dark_when_on_request_times_out_then_two_hop_quiescence_enters_danger() {
    let t0 = Instant::now();
    let mut ctx = ctx_driving_in_dark();
    ctx.headlamp.state = HeadlampState::OnRequested;
    ctx.headlamp.ack_pending_since = Some(t0);

    let result = run_to_quiescence(
        &FsmState::Driving,
        &ctx,
        &FsmEvent::TimerTick,
        t0 + FRONT_HEADLAMP_ON_ACK_WAIT,
        &ZoneReplies::simulate_locally(),
    );

    assert_eq!(result.hops.len(), 2, "external zone hop then internal synthesis");
    assert_eq!(result.hops[0].event, FsmEvent::TimerTick);
    assert!(matches!(
        result.hops[1].event,
        FsmEvent::Internal(Operational::LightingUnsafe)
    ));
    assert_eq!(
        result.hops[0].result.modified_ctx.headlamp.state,
        HeadlampState::Ready
    );
    assert!(
        result.hops[0]
            .result
            .actions
            .iter()
            .any(|a| matches!(a, DomainAction::LogWarning(_))),
        "zone should emit lighting timeout warning on hop 1"
    );
    assert_eq!(
        result.hops[1].result.modified_ctx,
        result.hops[0].result.modified_ctx,
        "internal hop must not mutate L1"
    );
    assert_eq!(result.final_step().next_state, FsmState::DrivingDangerously);
    assert!(
        result.merged_actions().contains(&DomainAction::StartBuzzer),
        "danger mode should alarm after quiescence"
    );
}

#[test]
fn given_idle_in_dark_when_on_request_times_out_then_stays_idle_not_dangerous() {
    let t0 = Instant::now();
    let mut ctx = VehicleContext::default();
    ctx.visibility.ambient_lux = 20;
    ctx.headlamp.state = HeadlampState::OnRequested;
    ctx.headlamp.ack_pending_since = Some(t0);

    let result = twin_turn(
        &FsmState::Idle,
        &ctx,
        &FsmEvent::TimerTick,
        t0 + FRONT_HEADLAMP_ON_ACK_WAIT,
    );

    assert_eq!(result.next_state, FsmState::Idle);
    assert_ne!(result.next_state, FsmState::DrivingDangerously);
    assert!(
        !result.actions.contains(&DomainAction::StartBuzzer),
        "danger policy applies only while operationally driving"
    );
}

#[test]
fn given_driving_dangerously_when_timer_tick_without_recovery_then_stays_dangerous_no_duplicate_buzzer(
) {
    let ctx = ctx_driving_dangerous_after_failed_on();
    let result = twin_turn(
        &FsmState::DrivingDangerously,
        &ctx,
        &FsmEvent::TimerTick,
        Instant::now(),
    );

    assert_eq!(result.next_state, FsmState::DrivingDangerously);
    assert!(
        !result.actions.contains(&DomainAction::StartBuzzer),
        "latched danger must not re-arm buzzer every tick"
    );
}

#[test]
fn given_driving_dangerously_when_lamp_on_in_dark_then_returns_to_driving_and_stops_buzzer() {
    let mut ctx = ctx_driving_dangerous_after_failed_on();
    ctx.headlamp.state = HeadlampState::On;

    let result = twin_turn(
        &FsmState::DrivingDangerously,
        &ctx,
        &FsmEvent::TimerTick,
        Instant::now(),
    );

    assert_eq!(result.next_state, FsmState::Driving);
    assert!(result.actions.contains(&DomainAction::StopBuzzer));
}

#[test]
fn given_driving_dangerously_when_lux_above_on_threshold_then_returns_to_driving() {
    let mut ctx = ctx_driving_dangerous_after_failed_on();
    ctx.visibility.ambient_lux = LUX_ON_THRESHOLD + 50;

    let result = twin_turn(
        &FsmState::DrivingDangerously,
        &ctx,
        &FsmEvent::UpdateAmbientLux(ctx.visibility.ambient_lux),
        Instant::now(),
    );

    assert_eq!(result.next_state, FsmState::Driving);
    assert!(result.actions.contains(&DomainAction::StopBuzzer));
}

#[test]
fn given_driving_dangerously_when_becomes_stationary_then_enters_idle_and_stops_buzzer() {
    let mut ctx = ctx_driving_dangerous_after_failed_on();
    ctx.powertrain.apply_rpm(0);
    ctx.powertrain.refresh_speed();
    ctx.powertrain.freeze_standstill();

    let result = twin_turn(
        &FsmState::DrivingDangerously,
        &ctx,
        &FsmEvent::TimerTick,
        Instant::now(),
    );

    assert_eq!(result.next_state, FsmState::Idle);
    assert!(result.actions.contains(&DomainAction::StopBuzzer));
}
