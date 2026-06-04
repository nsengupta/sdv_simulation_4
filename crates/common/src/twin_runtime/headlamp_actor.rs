//! L4 headlamp twinlet — brain **tell**s [`HeadlampActorVocabulary::Apply`]; twinlet **tell**s
//! [`DigitalTwinCarVocabulary::HeadlampZoneReady`] back.

use async_trait::async_trait;
use ractor::{Actor, ActorProcessingErr, ActorRef};
use std::time::Instant;

use crate::digital_twin::DigitalTwinCarVocabulary;
use crate::vehicle_state::{HeadlampContext, HeadlampMessage};

/// Tell payload: one [`HeadlampMessage`] for this brain [`turn_id`](Self::turn_id).
#[derive(Debug)]
pub struct HeadlampActorVocabulary {
    pub message: HeadlampMessage,
    pub now: Instant,
    pub turn_id: u64,
    pub brain: ActorRef<DigitalTwinCarVocabulary>,
}

#[derive(Default)]
pub struct HeadlampActor;

#[async_trait]
impl Actor for HeadlampActor {
    type Msg = HeadlampActorVocabulary;
    type State = HeadlampContext;
    type Arguments = HeadlampContext;

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
        HeadlampActorVocabulary {
            message,
            now,
            turn_id,
            brain,
        }: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        let zone_reply = state.on_receiving_message(message, now);
        *state = zone_reply.ctx.clone();
        brain
            .send_message(DigitalTwinCarVocabulary::HeadlampZoneReady {
                turn_id,
                reply: zone_reply,
            })
            .map_err(|e| {
                ActorProcessingErr::from(std::io::Error::other(format!(
                    "HeadlampZoneReady tell-back: {e:?}"
                )))
            })?;
        Ok(())
    }
}

/// Fire-and-forget tell to the headlamp twinlet (no reply port on this hop).
pub fn tell_headlamp_zone(
    headlamp: &ActorRef<HeadlampActorVocabulary>,
    brain: &ActorRef<DigitalTwinCarVocabulary>,
    turn_id: u64,
    message: HeadlampMessage,
    now: Instant,
) -> Result<(), ActorProcessingErr> {
    headlamp
        .send_message(HeadlampActorVocabulary {
            message,
            now,
            turn_id,
            brain: brain.clone(),
        })
        .map_err(|e| {
            ActorProcessingErr::from(std::io::Error::other(format!(
                "tell_headlamp_zone: {e:?}"
            )))
        })
}
