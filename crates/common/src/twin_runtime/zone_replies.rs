//! Collected zone tell-back embeds for one external hop (L4 orchestration).
//!
//! Phase 7 migrates from a field-per-zone layout (`headlamp: HeadlampReplies`) to a
//! homogeneous `HashMap<ZoneId, ZoneReply>`.  `HeadlampReplies` is deleted; `with_reply`
//! and `get` replace `with_headlamp_ingress`.
//!
//! Pure tests use [`ZoneReplies::simulate_locally`].

use std::collections::HashMap;

use crate::digital_twin::ZoneReply;
use crate::fsm::ZoneId;

/// All zone tell-back embeds collected before [`super::commit_resolved_turn`].
///
/// `replies` is a homogeneous map keyed by [`ZoneId`]: `zone_turn` calls
/// `get(&zone_id)` to find the relevant tell-back for each assembly, rather than
/// reaching into a zone-specific field.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ZoneReplies {
    pub replies: HashMap<ZoneId, ZoneReply>,
}

impl ZoneReplies {
    /// Pure tests / local path — no twinlet tell-back; L1 runs in-process.
    pub fn simulate_locally() -> Self {
        Self { replies: HashMap::new() }
    }

    /// Build a `ZoneReplies` carrying exactly one zone reply.
    ///
    /// Call-site translation:
    /// - Old: `ZoneReplies::with_headlamp_ingress(r)` →
    /// - New: `ZoneReplies::with_reply(ZoneId::Headlamp, ZoneReply::Headlamp(r))`
    pub fn with_reply(zone_id: ZoneId, reply: ZoneReply) -> Self {
        let mut map = HashMap::new();
        map.insert(zone_id, reply);
        Self { replies: map }
    }

    /// Look up the reply for a given zone (borrow).
    pub fn get(&self, id: &ZoneId) -> Option<&ZoneReply> {
        self.replies.get(id)
    }
}
