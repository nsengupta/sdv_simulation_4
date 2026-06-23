//! Zone-agnostic commit inputs: [`ZoneReplies`] on [`ResolvedTurn`] (Phase 7: map-based).

use std::time::Instant;

use crate::digital_twin::ZoneReply;
use crate::fsm::{FsmEvent, FsmState, HeadlampState, ZoneId};
use crate::twin_runtime::{commit_resolved_turn, twin_turn, ResolvedTurn, ZoneReplies};
use crate::vehicle_state::{HeadlampContext, HeadlampZoneReply, VehicleContext};
use crate::vehicle_physics::{FRONT_HEADLAMP_ON_ACK_WAIT, RPM_DRIVING_THRESHOLD};

fn driving_ctx() -> VehicleContext {
    let mut ctx = VehicleContext::default();
    ctx.visibility.ambient_lux = 20;
    ctx.powertrain.apply_rpm(RPM_DRIVING_THRESHOLD + 200);
    ctx.powertrain.refresh_speed();
    ctx
}

// --- Phase 6 regression guards (updated for Phase 7 map API) ---

#[test]
fn test_zone_replies_simulate_locally_is_empty() {
    // Phase 7: simulate_locally returns an empty map (no zone replies at all).
    let r = ZoneReplies::simulate_locally();
    assert!(
        r.get(&ZoneId::Headlamp).is_none(),
        "simulate_locally must produce an empty map; get(Headlamp) must be None"
    );
    assert!(
        r.replies.is_empty(),
        "simulate_locally must produce an empty map"
    );
}

#[test]
fn test_power_off_does_not_speculatively_run_zone_turn() {
    let ctx = VehicleContext::default();
    let result = twin_turn(
        &FsmState::Idle,
        &ctx,
        &FsmEvent::PowerOff,
        Instant::now(),
    );
    assert_eq!(result.next_state, FsmState::PreparingToStop);
    assert_eq!(
        result.modified_ctx.headlamp.state,
        ctx.headlamp.state,
        "PowerOff must not mutate headlamp state (no speculative IgnitionOffReset)"
    );
}

#[test]
fn test_zone_replies_with_reply_is_non_default_constructor() {
    // Phase 7: `with_reply` replaces the deleted `with_headlamp_ingress`.
    let embed = HeadlampZoneReply {
        ctx: HeadlampContext { state: HeadlampState::On, ack_pending_since: None },
        outcomes: vec![],
    };
    let r = ZoneReplies::with_reply(ZoneId::Headlamp, ZoneReply::Headlamp(embed.clone()));
    let got = r.get(&ZoneId::Headlamp).expect("must be present");
    assert_eq!(got.as_headlamp(), Some(&embed));
}

// --- Phase 7 map-shape tests ---

#[test]
fn test_zone_replies_map_get_returns_none_for_absent_zone() {
    let r = ZoneReplies::simulate_locally();
    assert!(
        r.get(&crate::fsm::ZoneId::Headlamp).is_none(),
        "simulate_locally must return an empty map; get(Headlamp) must be None"
    );
}

#[test]
fn test_zone_replies_with_reply_stores_and_retrieves() {
    use crate::vehicle_state::{HeadlampContext, HeadlampState, HeadlampZoneReply};
    let embed = HeadlampZoneReply {
        ctx: HeadlampContext { state: HeadlampState::On, ack_pending_since: None },
        outcomes: vec![],
    };
    let r = ZoneReplies::with_reply(
        crate::fsm::ZoneId::Headlamp,
        crate::digital_twin::ZoneReply::Headlamp(embed.clone()),
    );
    let got = r.get(&crate::fsm::ZoneId::Headlamp).expect("must be present");
    assert_eq!(got.as_headlamp(), Some(&embed));
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
            zone_replies: ZoneReplies::with_reply(ZoneId::Headlamp, ZoneReply::Headlamp(embed)),
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
