//! Collected zone tell-back embeds for one external hop (L4 orchestration).
//!
//! Phase 7 migrated from a field-per-zone layout (`headlamp: HeadlampReplies`) to a
//! homogeneous `HashMap<AssemblyId, ZoneReply>`.  `with_reply` and `get` replaced
//! `with_headlamp_ingress`.
//!
//! Pure tests use [`ZoneReplies::simulate_locally`].

use std::collections::HashMap;

use crate::digital_twin::ZoneReply;
use crate::fsm::AssemblyId;

/// All zone tell-back embeds collected before [`super::commit_resolved_turn`].
///
/// `replies` is a homogeneous map keyed by [`AssemblyId`]: `zone_turn` calls
/// `get(&assembly_id)` to find the relevant tell-back for each assembly, rather than
/// reaching into an assembly-specific field.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ZoneReplies {
    pub replies: HashMap<AssemblyId, ZoneReply>,
}

impl ZoneReplies {
    /// Pure tests / local path — no twinlet tell-back; L1 runs in-process.
    pub fn simulate_locally() -> Self {
        Self { replies: HashMap::new() }
    }

    /// Build a `ZoneReplies` carrying exactly one zone reply.
    pub fn with_reply(assembly_id: AssemblyId, reply: ZoneReply) -> Self {
        let mut map = HashMap::new();
        map.insert(assembly_id, reply);
        Self { replies: map }
    }

    /// Look up the reply for a given assembly (borrow).
    pub fn get(&self, id: &AssemblyId) -> Option<&ZoneReply> {
        self.replies.get(id)
    }
}
