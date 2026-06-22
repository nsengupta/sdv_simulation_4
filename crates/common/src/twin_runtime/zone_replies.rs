//! Collected zone tell-back embeds for one external hop (L4 orchestration).
//!
//! Pure tests use [`ZoneReplies::simulate_locally`].
//! Add a field per actorified zone — do not add top-level `headlamp_reply` on [`ResolvedTurn`].

use crate::vehicle_state::HeadlampZoneReply;

/// Tell-back embeds for the headlamp zone on one ingress hop.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HeadlampReplies {
    /// Tell-back for the headlamp message demuxed from this hop's FSM ingress
    /// ([`super::zone_turn::user_event_to_headlamp_tell`]); `None` on pure/local path.
    pub ingress: Option<HeadlampZoneReply>,
}

/// All zone embeds collected before [`super::commit_resolved_turn`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ZoneReplies {
    pub headlamp: HeadlampReplies,
}

impl ZoneReplies {
    /// Pure tests / local [`super::zone_turn`] — no twinlet tell-back; L1 runs in-process.
    pub fn simulate_locally() -> Self {
        Self::default()
    }

    pub fn with_headlamp_ingress(ingress: HeadlampZoneReply) -> Self {
        Self {
            headlamp: HeadlampReplies {
                ingress: Some(ingress),
            },
        }
    }
}
