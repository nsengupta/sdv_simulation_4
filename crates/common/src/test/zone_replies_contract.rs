//! Zone-agnostic commit inputs: [`ZoneReplies`] on [`ResolvedTurn`] (clone template for zone #2).

use std::time::Instant;

use crate::fsm::{FsmEvent, FsmState, HeadlampState};
use crate::twin_runtime::{commit_resolved_turn, twin_turn, ResolvedTurn, ZoneReplies};
use crate::twin_runtime::zone_replies::HeadlampReplies;
use crate::vehicle_state::{HeadlampContext, HeadlampZoneReply, VehicleContext};
use crate::vehicle_physics::{FRONT_HEADLAMP_ON_ACK_WAIT, RPM_DRIVING_THRESHOLD};

fn driving_ctx() -> VehicleContext {
    let mut ctx = VehicleContext::default();
    ctx.visibility.ambient_lux = 20;
    ctx.powertrain.apply_rpm(RPM_DRIVING_THRESHOLD + 200);
    ctx.powertrain.refresh_speed();
    ctx
}

// --- Phase 6 structural tests ---

#[test]
fn test_zone_replies_simulate_locally_has_no_ignition_off_reset() {
    let r = ZoneReplies::simulate_locally();
    assert_eq!(
        r.headlamp.ingress, None,
        "simulate_locally must produce ingress=None"
    );
    // Structural guard: HeadlampReplies has exactly one field.
    // If ignition_off_reset were re-added, this destructuring would fail to compile.
    let HeadlampReplies { ingress: _ } = r.headlamp;
}

#[test]
fn test_power_off_does_not_speculatively_run_zone_turn() {
    // PowerOff → PreparingToStop (not Off), so the old IgnitionOffReset block must not fire.
    // This is a regression guard: if IgnitionOffReset logic is re-introduced, this test
    // would detect an unexpected context mutation.
    let ctx = VehicleContext::default();
    let result = twin_turn(
        &FsmState::Idle,
        &ctx,
        &FsmEvent::PowerOff,
        Instant::now(),
    );
    assert_eq!(result.next_state, FsmState::PreparingToStop);
    // Headlamp context must be unchanged (no ResetForIgnitionOff applied).
    assert_eq!(
        result.modified_ctx.headlamp.state,
        ctx.headlamp.state,
        "PowerOff must not mutate headlamp state (no speculative IgnitionOffReset)"
    );
}

#[test]
fn test_headlamp_replies_with_headlamp_ingress_is_the_only_non_default_constructor() {
    let embed = HeadlampZoneReply {
        ctx: HeadlampContext {
            state: HeadlampState::On,
            ack_pending_since: None,
        },
        outcomes: vec![],
    };
    let r = ZoneReplies::with_headlamp_ingress(embed.clone());
    assert_eq!(r.headlamp.ingress, Some(embed));
}

// --- Original tests ---

#[test]
fn given_headlamp_ingress_embed_when_commit_resolved_turn_then_uses_tell_back_not_local_zone() {
    let t0 = Instant::now();
    let ctx = driving_ctx();
    let embed = HeadlampZoneReply {
        ctx: HeadlampContext {
            state: HeadlampState::On,
            ack_pending_since: None,
        },
        outcomes: vec![],
    };

    let quiescent = commit_resolved_turn(
        &FsmState::Driving,
        &ctx,
        ResolvedTurn {
            ingress: FsmEvent::TimerTick,
            now: t0,
            zone_replies: ZoneReplies::with_headlamp_ingress(embed),
        },
    );

    assert_eq!(
        quiescent.final_step().modified_ctx.headlamp.state,
        HeadlampState::On,
        "ingress tell-back embed must win over local on_receiving_message"
    );
}

#[test]
fn given_simulated_replies_when_twin_turn_after_ack_wait_then_matches_quiescence_two_hop_journey() {
    let t0 = Instant::now();
    let mut ctx = driving_ctx();
    ctx.headlamp.state = HeadlampState::OnRequested;
    ctx.headlamp.ack_pending_since = Some(t0);

    let single = twin_turn(
        &FsmState::Driving,
        &ctx,
        &FsmEvent::TimerTick,
        t0 + FRONT_HEADLAMP_ON_ACK_WAIT,
    );
    let quiescent = commit_resolved_turn(
        &FsmState::Driving,
        &ctx,
        ResolvedTurn {
            ingress: FsmEvent::TimerTick,
            now: t0 + FRONT_HEADLAMP_ON_ACK_WAIT,
            zone_replies: ZoneReplies::simulate_locally(),
        },
    );

    assert_eq!(quiescent.hops.len(), 2);
    assert_eq!(
        quiescent.hops[0].result.modified_ctx.headlamp.state,
        single.modified_ctx.headlamp.state
    );
    assert_eq!(
        quiescent.final_step().next_state,
        crate::fsm::FsmState::DrivingDangerously
    );
}
