//! Item 1 — brain commit hook: `commit_resolved_turn` + quiescence on the actor path (ADR-7).
//!
//! TDD: given-when-then names; pure tests first, then actor wiring.

use std::time::{Duration, Instant};

use crate::fsm::{DomainAction, FsmEvent, FsmState, HeadlampState, Operational};
use crate::published::{PublishedDomainAction, PublishedFsmEvent, PublishedFsmState, PublishedOperational};
use crate::test::power_on_to_idle;
use crate::test::ActorGuard;
use crate::twin_runtime::controller::vehicle_controller::VehicleControllerRuntimeOptions;
use crate::twin_runtime::{commit_resolved_turn, ResolvedTurn, ZoneReplies};
use crate::vehicle_state::VehicleContext;
use crate::vehicle_physics::{
    FRONT_HEADLAMP_ON_ACK_WAIT, RPM_DRIVING_THRESHOLD,
};
use crate::{PhysicalCarVocabulary, VehicleController, VssSignal};
use tokio::sync::mpsc;

fn ctx_driving_in_dark() -> VehicleContext {
    let mut ctx = VehicleContext::default();
    ctx.visibility.ambient_lux = 20;
    ctx.powertrain.apply_rpm(RPM_DRIVING_THRESHOLD + 200);
    ctx.powertrain.refresh_speed();
    ctx
}

#[test]
fn given_driving_on_requested_in_dark_when_commit_resolved_turn_after_ack_wait_then_two_hops_enter_danger(
) {
    let t0 = Instant::now();
    let mut ctx = ctx_driving_in_dark();
    ctx.headlamp.state = HeadlampState::OnRequested;
    ctx.headlamp.ack_pending_since = Some(t0);

    let quiescent = commit_resolved_turn(
        &FsmState::Driving,
        &ctx,
        ResolvedTurn {
            ingress: FsmEvent::TimerTick,
            now: t0 + FRONT_HEADLAMP_ON_ACK_WAIT,
            zone_replies: ZoneReplies::simulate_locally(),
        },
    );

    assert_eq!(quiescent.hops.len(), 2, "zone hop then internal synthesis");
    assert_eq!(quiescent.hops[0].event, FsmEvent::TimerTick);
    assert!(matches!(
        quiescent.hops[1].event,
        FsmEvent::Internal(Operational::LightingUnsafe)
    ));
    assert_eq!(
        quiescent.final_step().next_state,
        FsmState::DrivingDangerously
    );
    assert!(
        quiescent.merged_actions().contains(&DomainAction::StartBuzzer),
        "quiescence must merge buzzer from internal hop"
    );
}

#[test]
fn given_driving_in_dark_when_commit_resolved_turn_without_zone_reply_then_single_hop_stays_driving(
) {
    let t0 = Instant::now();
    let mut ctx = ctx_driving_in_dark();
    ctx.headlamp.state = HeadlampState::OnRequested;
    ctx.headlamp.ack_pending_since = Some(t0);

    let quiescent = commit_resolved_turn(
        &FsmState::Driving,
        &ctx,
        ResolvedTurn {
            ingress: FsmEvent::TimerTick,
            now: t0,
            zone_replies: ZoneReplies::simulate_locally(),
        },
    );

    assert_eq!(quiescent.hops.len(), 1);
    assert_eq!(quiescent.final_step().next_state, FsmState::Driving);
    assert!(
        !quiescent
            .hops
            .iter()
            .any(|h| matches!(h.event, FsmEvent::Internal(Operational::LightingUnsafe))),
        "no internal hop before ACK timeout"
    );
}

#[tokio::test]
async fn given_actor_idle_when_power_on_then_single_ledger_row_and_idle_state() {
    // Phase 1: PowerOn → PreparingToStart (seq 1), then AssembliesReady → Idle (seq 2).
    let (transition_tx, mut rx) = mpsc::channel(8);
    let runtime_options = VehicleControllerRuntimeOptions {
        transition_tx: Some(transition_tx),
        ..VehicleControllerRuntimeOptions::default()
    };

    let (controller, handle) = VehicleController::install_and_start_with_options(
        "QUI-ACTOR-01".to_string(),
        runtime_options,
    )
    .await
    .expect("start actor");
    let _guard = ActorGuard {
        addr: controller.get_actor_ref().clone(),
        handle,
    };

    controller.send_power_on().await.expect("power on");

    let record_start = rx.recv().await.expect("power on → preparing ledger row");
    assert_eq!(record_start.record_seq, 1);
    assert_eq!(record_start.event, PublishedFsmEvent::PowerOn);
    assert_eq!(record_start.next_state, PublishedFsmState::PreparingToStart);

    // Phase 7: TWO startup barriers drain:
    //   row 2 = AssemblyZoneReady(Headlamp) → PreparingToStart (Wiper still pending)
    //   row 3 = AssemblyZoneReady(Wiper) → Idle
    let record_headlamp = rx.recv().await.expect("headlamp zone ready row");
    assert_eq!(record_headlamp.record_seq, 2);
    assert_eq!(record_headlamp.next_state, PublishedFsmState::PreparingToStart);

    let record_idle = rx.recv().await.expect("wiper zone ready → idle ledger row");
    assert_eq!(record_idle.record_seq, 3);
    assert_eq!(record_idle.next_state, PublishedFsmState::Idle);

    let snapshot = controller
        .get_snapshot(Some(ractor::concurrency::Duration::from_millis(250)))
        .await
        .expect("snapshot");
    assert_eq!(*snapshot.current_state(), FsmState::Idle);
    assert_eq!(snapshot.as_of_seq(), 3);
}

#[tokio::test]
async fn given_actor_driving_in_dark_when_ack_wait_elapses_then_two_ledger_rows_and_driving_dangerously(
) {
    let (transition_tx, mut rx) = mpsc::channel(16);
    let runtime_options = VehicleControllerRuntimeOptions {
        transition_tx: Some(transition_tx),
        ..VehicleControllerRuntimeOptions::default()
    };

    let (controller, handle) = VehicleController::install_and_start_with_options(
        "QUI-ACTOR-02".to_string(),
        runtime_options,
    )
    .await
    .expect("start actor");
    let _guard = ActorGuard {
        addr: controller.get_actor_ref().clone(),
        handle,
    };

    // Phase 7: drain THREE boot rows:
    //   row 1 = PowerOn → PreparingToStart
    //   row 2 = AssemblyZoneReady(Headlamp) → PreparingToStart
    //   row 3 = AssemblyZoneReady(Wiper) → Idle
    power_on_to_idle(&controller).await;
    let _ = rx.recv().await.expect("power on → preparing row");
    let _ = rx.recv().await.expect("headlamp zone ready → preparing row");
    let _ = rx.recv().await.expect("wiper zone ready → idle row");

    crate::test::submit_daylight_ambient(&controller).await;
    let _ = rx.recv().await.expect("bright lux row");

    controller
        .submit_fsm_event(FsmEvent::UpdateRpm(RPM_DRIVING_THRESHOLD + 200))
        .await
        .expect("rpm");
    let _ = rx.recv().await.expect("rpm row");

    controller
        .submit_physical_car_event(PhysicalCarVocabulary::TelemetryUpdate(VssSignal::AmbientLux(
            20,
        )))
        .await
        .expect("low lux");
    let lux_row = rx.recv().await.expect("lux row");
    assert_eq!(lux_row.next_state, PublishedFsmState::Driving);

    tokio::time::sleep(FRONT_HEADLAMP_ON_ACK_WAIT + Duration::from_millis(25)).await;

    let hop1 = rx.recv().await.expect("spontaneous incomplete hop ledger row");
    let hop2 = rx.recv().await.expect("internal hop ledger row");

    assert!(
        matches!(
            hop1.event,
            PublishedFsmEvent::FrontHeadlampActuationIncomplete { .. }
        ),
        "ACK timeout must not depend on TimerTick, got {:?}",
        hop1.event
    );
    assert_eq!(hop1.next_state, PublishedFsmState::Driving);
    assert!(matches!(
        hop2.event,
        PublishedFsmEvent::Internal(PublishedOperational::LightingUnsafe)
    ));
    assert_eq!(hop2.next_state, PublishedFsmState::DrivingDangerously);
    assert!(
        hop2
            .actions
            .iter()
            .any(|a| matches!(a, PublishedDomainAction::StartBuzzer)),
        "internal hop row must carry StartBuzzer, got {:?}",
        hop2.actions
    );
    assert_eq!(hop2.record_seq, hop1.record_seq + 1);

    crate::test::wait_fsm_state(
        &controller,
        FsmState::DrivingDangerously,
        Duration::from_secs(1),
    )
    .await;

    let snapshot = controller
        .get_snapshot(Some(ractor::concurrency::Duration::from_millis(250)))
        .await
        .expect("snapshot");
    assert_eq!(*snapshot.current_state(), FsmState::DrivingDangerously);
    assert_eq!(snapshot.as_of_seq(), hop2.record_seq);
    assert_eq!(
        snapshot.context().headlamp.state,
        HeadlampState::Ready,
        "zone hop should settle failed ON request to Ready (assembly active) before danger synthesis"
    );
}
