//! L4 ingress demux: [`FsmEvent`] → per-zone messages (and in-process L1 where no twinlet yet).

use std::time::Instant;

use crate::digital_twin::{ZoneMessage, ZoneReply};
use crate::fsm::{FsmEvent, FsmState, ZoneId};
use crate::twin_runtime::zone_replies::ZoneReplies;
use crate::vehicle_state::{
    HeadlampMessage, HeadlampOutcome, HeadlampZoneReply,
    WiperMessage, WiperOutcome, WiperZoneReply,
    VehicleContext,
};

/// Tagged zone egress for one `zone_turn` call — replaces the per-zone
/// `headlamp_outcomes` / `wiper_outcomes` fields so `ZoneTurnResult` stays
/// homogeneous as the number of assemblies grows.
#[derive(Debug, Clone, PartialEq)]
pub enum ZoneOutcome {
    Headlamp(HeadlampOutcome),
    Wiper(WiperOutcome),
}

/// Zone layer output for one mailbox event.
///
/// `outcomes` replaces the former per-zone `headlamp_outcomes` / `wiper_outcomes` fields.
/// The `headlamp_before` / `wiper_before` snapshots are captured at the *call site* from
/// the input `ctx` before calling `zone_turn` — they are not embedded here.
#[derive(Debug)]
pub struct ZoneTurnResult {
    pub ctx: VehicleContext,
    pub outcomes: Vec<ZoneOutcome>,
}

/// State-aware zone routing for a *user-originated* [`FsmEvent`].
///
/// Returns `None` when the FSM is in a lifecycle transition state (`PreparingToStart` or
/// `PreparingToStop`): no zone tell is emitted for user events during assembly startup or
/// shutdown.  For all other states, delegates to [`user_event_to_zone_tell`].
///
/// Used by `begin_fsm_turn` to decide between a zone-directed [`TurnBarrier`] and a
/// [`PassthroughBarrier`].
pub(crate) fn zone_message_for_event(
    event: &FsmEvent,
    state: &FsmState,
) -> Option<(ZoneId, ZoneMessage)> {
    match state {
        FsmState::PreparingToStart | FsmState::PreparingToStop => None,
        _ => user_event_to_zone_tell(event),
    }
}

/// Map a *user-originated* [`FsmEvent`] to the `(ZoneId, ZoneMessage)` pair that must be
/// told to the relevant zone twinlet.  Returns `None` for events that do not require a zone
/// tell (e.g. `PowerOn`, `UpdateRpm`) or for assembly lifecycle events (`AssemblyZoneReady`),
/// which carry their reply embedded in the barrier.
fn user_event_to_zone_tell(event: &FsmEvent) -> Option<(ZoneId, ZoneMessage)> {
    match event {
        FsmEvent::UpdateAmbientLux(lux) => Some((
            ZoneId::Headlamp,
            ZoneMessage::Headlamp(HeadlampMessage::AmbientLux(*lux)),
        )),
        FsmEvent::FrontHeadlampOnAck => Some((
            ZoneId::Headlamp,
            ZoneMessage::Headlamp(HeadlampMessage::AckOn),
        )),
        FsmEvent::FrontHeadlampOffAck => Some((
            ZoneId::Headlamp,
            ZoneMessage::Headlamp(HeadlampMessage::AckOff),
        )),
        FsmEvent::FrontHeadlampActuationIncomplete { direction, cause } => Some((
            ZoneId::Headlamp,
            ZoneMessage::Headlamp(HeadlampMessage::ActuationIncomplete {
                direction: *direction,
                cause: *cause,
            }),
        )),
        FsmEvent::RainsStarted => Some((
            ZoneId::Wiper,
            ZoneMessage::Wiper(WiperMessage::Start),
        )),
        FsmEvent::RainsStopped => Some((
            ZoneId::Wiper,
            ZoneMessage::Wiper(WiperMessage::Stop),
        )),
        FsmEvent::UpdateRpm(_)
        | FsmEvent::PowerOn
        | FsmEvent::PowerOff
        | FsmEvent::TimerTick
        | FsmEvent::Internal(_)
        | FsmEvent::AssemblyZoneReady(_) => None,
    }
}

fn merge_headlamp_for_message(
    ctx: &VehicleContext,
    message: HeadlampMessage,
    now: Instant,
    tell_back: Option<&HeadlampZoneReply>,
) -> HeadlampZoneReply {
    tell_back.cloned().unwrap_or_else(|| ctx.headlamp.on_receiving_message(message, now))
}

fn merge_wiper_for_message(
    ctx: &VehicleContext,
    message: WiperMessage,
    tell_back: Option<&WiperZoneReply>,
) -> WiperZoneReply {
    tell_back.cloned().unwrap_or_else(|| ctx.wiper.on_receiving_message(message))
}

/// Apply ingress to L1 zones. Does not run the operational FSM (L2).
pub fn zone_turn(
    ctx: &VehicleContext,
    event: &FsmEvent,
    current_state: &FsmState,
    now: Instant,
    zone_replies: &ZoneReplies,
) -> ZoneTurnResult {
    let mut next = ctx.clone();
    let mut outcomes: Vec<ZoneOutcome> = Vec::new();

    let headlamp_ingress = zone_replies.get(&ZoneId::Headlamp).and_then(ZoneReply::as_headlamp);
    let wiper_ingress = zone_replies.get(&ZoneId::Wiper).and_then(ZoneReply::as_wiper);

    match event {
        FsmEvent::UpdateRpm(rpm) => {
            next.powertrain.apply_rpm(*rpm);
            next.powertrain.refresh_speed();
            if *current_state == FsmState::Off {
                next.powertrain.freeze_standstill();
            }
        }
        FsmEvent::UpdateAmbientLux(lux) => {
            next.visibility.apply_lux(*lux);
            let zone_reply = merge_headlamp_for_message(
                ctx,
                HeadlampMessage::AmbientLux(*lux),
                now,
                headlamp_ingress,
            );
            next.headlamp = zone_reply.ctx;
            outcomes.extend(zone_reply.outcomes.into_iter().map(ZoneOutcome::Headlamp));
        }
        FsmEvent::FrontHeadlampOnAck => {
            let zone_reply =
                merge_headlamp_for_message(ctx, HeadlampMessage::AckOn, now, headlamp_ingress);
            next.headlamp = zone_reply.ctx;
            outcomes.extend(zone_reply.outcomes.into_iter().map(ZoneOutcome::Headlamp));
        }
        FsmEvent::FrontHeadlampOffAck => {
            let zone_reply =
                merge_headlamp_for_message(ctx, HeadlampMessage::AckOff, now, headlamp_ingress);
            next.headlamp = zone_reply.ctx;
            outcomes.extend(zone_reply.outcomes.into_iter().map(ZoneOutcome::Headlamp));
        }
        FsmEvent::FrontHeadlampActuationIncomplete { direction, cause } => {
            let zone_reply = merge_headlamp_for_message(
                ctx,
                HeadlampMessage::ActuationIncomplete {
                    direction: *direction,
                    cause: *cause,
                },
                now,
                headlamp_ingress,
            );
            next.headlamp = zone_reply.ctx;
            outcomes.extend(zone_reply.outcomes.into_iter().map(ZoneOutcome::Headlamp));
        }
        FsmEvent::TimerTick => {
            let zone_reply =
                merge_headlamp_for_message(ctx, HeadlampMessage::TimerTick, now, headlamp_ingress);
            next.headlamp = zone_reply.ctx;
            outcomes.extend(zone_reply.outcomes.into_iter().map(ZoneOutcome::Headlamp));
        }
        FsmEvent::RainsStarted => {
            let zone_reply = merge_wiper_for_message(ctx, WiperMessage::Start, wiper_ingress);
            next.wiper = zone_reply.ctx;
            outcomes.extend(zone_reply.outcomes.into_iter().map(ZoneOutcome::Wiper));
        }
        FsmEvent::RainsStopped => {
            let zone_reply = merge_wiper_for_message(ctx, WiperMessage::Stop, wiper_ingress);
            next.wiper = zone_reply.ctx;
            outcomes.extend(zone_reply.outcomes.into_iter().map(ZoneOutcome::Wiper));
        }
        FsmEvent::PowerOn | FsmEvent::PowerOff | FsmEvent::Internal(_) => {}
        FsmEvent::AssemblyZoneReady(zone_id) => {
            match zone_id {
                ZoneId::Headlamp => {
                    if let Some(reply) = headlamp_ingress {
                        next.headlamp = reply.ctx.clone();
                        outcomes.extend(reply.outcomes.iter().cloned().map(ZoneOutcome::Headlamp));
                    }
                }
                ZoneId::Wiper => {
                    if let Some(reply) = wiper_ingress {
                        next.wiper = reply.ctx.clone();
                        outcomes.extend(reply.outcomes.iter().cloned().map(ZoneOutcome::Wiper));
                    }
                }
            }
        }
    }

    ZoneTurnResult { ctx: next, outcomes }
}
