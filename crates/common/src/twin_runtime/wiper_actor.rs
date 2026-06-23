//! L4 wiper twinlet — brain **tell**s [`WiperActorMsg::Apply`]; twinlet **tell**s
//! [`DigitalTwinCarVocabulary::ZoneReady`] immediately (no ACK protocol).
//!
//! Phase 7: all wiper transitions are direct — no `OffRequested`/`OnRequested` intermediate
//! states, no ACK timer.  `post_stop` is a no-op.

use async_trait::async_trait;
use ractor::{Actor, ActorProcessingErr, ActorRef};
use std::time::Instant;

use crate::digital_twin::{DigitalTwinCarVocabulary, ZoneReply};
use crate::vehicle_state::{WiperContext, WiperMessage};

/// Tell payload: one [`WiperMessage`] for this brain [`turn_id`](Self::turn_id).
#[derive(Debug)]
pub struct WiperActorVocabulary {
    pub message: WiperMessage,
    pub now: Instant,
    pub turn_id: u64,
    /// Matches brain tell-back wait attempt (retries use incrementing ids).
    pub tell_attempt: u32,
    pub brain: ActorRef<DigitalTwinCarVocabulary>,
}

/// Wiper twinlet mailbox — brain tells only (no ACK deadline variant).
#[derive(Debug)]
pub enum WiperActorMsg {
    Apply(WiperActorVocabulary),
}

#[derive(Debug)]
pub struct WiperActorState {
    pub ctx: WiperContext,
    /// When true, swallow tells without tell-back (contract tests only).
    pub silent: bool,
}

impl WiperActorState {
    pub fn new(ctx: WiperContext, silent: bool) -> Self {
        Self { ctx, silent }
    }
}

#[derive(Default)]
pub struct WiperActor;

#[async_trait]
impl Actor for WiperActor {
    type Msg = WiperActorMsg;
    type State = WiperActorState;
    type Arguments = WiperActorState;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(args)
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        let WiperActorMsg::Apply(vocab) = message;
        Self::handle_apply(state, vocab).await
    }

    async fn post_stop(
        &self,
        _myself: ActorRef<Self::Msg>,
        _state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        // No timers to abort — no-op.
        Ok(())
    }
}

impl WiperActor {
    async fn handle_apply(
        state: &mut WiperActorState,
        WiperActorVocabulary {
            message,
            now: _now,
            turn_id,
            tell_attempt,
            brain,
        }: WiperActorVocabulary,
    ) -> Result<(), ActorProcessingErr> {
        if state.silent {
            return Ok(());
        }

        let zone_reply = state.ctx.on_receiving_message(message);
        state.ctx = zone_reply.ctx.clone();
        brain
            .send_message(DigitalTwinCarVocabulary::ZoneReady {
                zone_id: crate::fsm::ZoneId::Wiper,
                turn_id,
                tell_attempt,
                reply: ZoneReply::Wiper(zone_reply),
            })
            .map_err(|e| {
                ActorProcessingErr::from(std::io::Error::other(format!(
                    "WiperActor ZoneReady tell-back: {e:?}"
                )))
            })?;
        Ok(())
    }
}

/// Fire-and-forget tell to the wiper twinlet (no reply port on this hop).
pub fn tell_wiper_zone(
    wiper: &ActorRef<WiperActorMsg>,
    brain: &ActorRef<DigitalTwinCarVocabulary>,
    turn_id: u64,
    tell_attempt: u32,
    message: WiperMessage,
    now: Instant,
) -> Result<(), ActorProcessingErr> {
    wiper
        .send_message(WiperActorMsg::Apply(WiperActorVocabulary {
            message,
            now,
            turn_id,
            tell_attempt,
            brain: brain.clone(),
        }))
        .map_err(|e| {
            ActorProcessingErr::from(std::io::Error::other(format!(
                "tell_wiper_zone: {e:?}"
            )))
        })
}
