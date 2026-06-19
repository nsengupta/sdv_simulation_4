//! Step 4: [`HeadlampZoneReply`] embed must match ledger projection and [`GetStatus`] snapshot.
//!
//! When moving toward Q5-C, shrink `HeadlampZoneReply` or stop embedding fields — these tests
//! should fail with a clear boundary (ledger incomplete vs snapshot stale).

use std::time::Instant;

use crate::digital_twin::DigitalTwinCarVocabulary;
use crate::fsm::{FsmEvent, HeadlampState, Operational};
use crate::published::{PublishedHeadlampContext, PublishedHeadlampState};
use crate::test::{expect_actuation_command, inject_matching_ack, ActorGuard};
use crate::twin_runtime::controller::vehicle_controller::VehicleControllerRuntimeOptions;
use crate::vehicle_state::{HeadlampContext, HeadlampMessage};
use crate::{PhysicalCarVocabulary, PublishedFsmEvent, VehicleController, VssSignal};
use ractor::concurrency::Duration;
use tokio::sync::mpsc;

const ACTOR_TIMEOUT: Duration = Duration::from_millis(250);

fn assert_published_headlamp_matches_runtime(
    published: &PublishedHeadlampContext,
    runtime: &HeadlampContext,
) {
    let expected_state: PublishedHeadlampState = (&runtime.state).into();
    assert_eq!(
        published.state, expected_state,
        "ledger headlamp.state must match persisted runtime snapshot"
    );
    assert_eq!(
        published.ack_pending_since_unix.is_some(),
        runtime.ack_pending_since.is_some(),
        "ledger ACK-wait presence must match runtime (temporal anchor may differ in wall projection)"
    );
}

fn expected_headlamp_after_on_ack_journey(now: Instant) -> HeadlampContext {
    // Starting in Ready (assembly active, lamp dark) — the post-Phase-2 baseline.
    let ctx = HeadlampContext { state: HeadlampState::Ready, ack_pending_since: None };
    let after_lux = ctx.on_receiving_message(HeadlampMessage::AmbientLux(20), now).ctx;
    after_lux.on_receiving_message(HeadlampMessage::AckOn, now).ctx
}

#[tokio::test]
async fn given_low_lux_and_on_ack_when_get_status_then_ledger_headlamp_matches_embed() {
    let (transition_tx, mut rx) = mpsc::channel(16);
    let (actuation_tx, mut actuation_rx) = mpsc::channel(16);
    let runtime_options = VehicleControllerRuntimeOptions {
        transition_tx: Some(transition_tx),
        actuation_command_tx: Some(actuation_tx),
        // Phase 2: headlamp starts in Ready (assembly active, lamp dark).
        // Phase 5 will drive this transition automatically via StartAssemblies + BecomeOn.
        initial_headlamp_ctx: Some(HeadlampContext {
            state: HeadlampState::Ready,
            ack_pending_since: None,
        }),
        ..VehicleControllerRuntimeOptions::default()
    };

    let (controller, handle) = VehicleController::install_and_start_with_options(
        "HL-REPLY-01".to_string(),
        runtime_options,
    )
    .await
    .expect("start actor");
    let _guard = ActorGuard {
        addr: controller.get_actor_ref().clone(),
        handle,
    };

    // Phase 1: PowerOn → PreparingToStart; inject AssembliesReady to advance to Idle.
    controller.send_power_on().await.expect("power on");
    let _power_on_record = rx.recv().await.expect("ledger row for power on");
    controller
        .submit_fsm_event(FsmEvent::Internal(Operational::AssembliesReady))
        .await
        .expect("assemblies ready");
    let _assemblies_ready_record = rx.recv().await.expect("ledger row for assemblies ready");

    controller
        .submit_physical_car_event(PhysicalCarVocabulary::TelemetryUpdate(VssSignal::AmbientLux(
            20,
        )))
        .await
        .expect("low lux");

    let lux_record = rx.recv().await.expect("ledger row for lux");
    assert_eq!(lux_record.event, PublishedFsmEvent::UpdateAmbientLux(20));
    assert_eq!(
        lux_record.current_ctx.headlamp.state,
        PublishedHeadlampState::OnRequested,
    );
    assert!(
        lux_record.current_ctx.headlamp.ack_pending_since_unix.is_some(),
        "ON request should leave ACK-wait in ledger current_ctx"
    );

    let command =
        expect_actuation_command(&mut actuation_rx, Duration::from_millis(250)).await;
    inject_matching_ack(&controller, &command).await;

    let ack_record = rx.recv().await.expect("ledger row for ON ack");
    assert_eq!(ack_record.event, PublishedFsmEvent::FrontHeadlampOnAck);
    assert_eq!(
        ack_record.current_ctx.headlamp.state,
        PublishedHeadlampState::On,
    );
    assert!(
        ack_record.current_ctx.headlamp.ack_pending_since_unix.is_none(),
        "settled ON must clear ACK-wait in ledger"
    );

    let snapshot = controller
        .get_snapshot(Some(ACTOR_TIMEOUT))
        .await
        .expect("GetStatus");

    assert_published_headlamp_matches_runtime(
        &ack_record.current_ctx.headlamp,
        &snapshot.context().headlamp,
    );
    assert_eq!(snapshot.context().headlamp.state, HeadlampState::On);
    assert!(snapshot.context().headlamp.ack_pending_since.is_none());

    let expected = expected_headlamp_after_on_ack_journey(Instant::now());
    assert_eq!(snapshot.context().headlamp.state, expected.state);
    assert_eq!(
        snapshot.context().headlamp.ack_pending_since.is_some(),
        expected.ack_pending_since.is_some(),
    );
}

#[tokio::test]
async fn given_power_on_only_when_get_status_then_ledger_headlamp_matches_embed() {
    let (tx, mut rx) = mpsc::channel(8);
    let runtime_options = VehicleControllerRuntimeOptions {
        transition_tx: Some(tx),
        ..VehicleControllerRuntimeOptions::default()
    };

    let (controller, handle) = VehicleController::install_and_start_with_options(
        "HL-REPLY-02".to_string(),
        runtime_options,
    )
    .await
    .expect("start actor");
    let actor_ref = controller.get_actor_ref().clone();
    let _guard = ActorGuard {
        addr: actor_ref.clone(),
        handle,
    };

    controller.send_power_on().await.expect("power on");

    let record = rx.recv().await.expect("ledger row for power on");
    assert_eq!(record.event, PublishedFsmEvent::PowerOn);

    let snapshot = actor_ref
        .call(
            |port| DigitalTwinCarVocabulary::GetStatus(port),
            Some(ACTOR_TIMEOUT),
        )
        .await
        .expect("GetStatus call")
        .expect("GetStatus reply");

    assert_published_headlamp_matches_runtime(
        &record.current_ctx.headlamp,
        &snapshot.context().headlamp,
    );
}
