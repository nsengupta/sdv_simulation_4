//! L4: map L1 zone outcomes to L2 [`DomainAction`] for the actuation path (shim until ADR-6).

use crate::fsm::DomainAction;
use crate::twin_runtime::zone_turn::ZoneOutcome;
use crate::vehicle_state::HeadlampOutcome;

pub fn zone_outcomes_to_domain_actions(
    outcomes: impl IntoIterator<Item = ZoneOutcome>,
) -> Vec<DomainAction> {
    outcomes
        .into_iter()
        .filter_map(|o| match o {
            ZoneOutcome::Headlamp(ho) => headlamp_outcome_to_domain_action(ho),
            ZoneOutcome::Wiper(_) => None, // Wiper outcomes have no actuation path in Phase 7
        })
        .collect()
}

fn headlamp_outcome_to_domain_action(outcome: HeadlampOutcome) -> Option<DomainAction> {
    match outcome {
        HeadlampOutcome::RequestOn => Some(DomainAction::RequestFrontHeadlampOn),
        HeadlampOutcome::RequestOff => Some(DomainAction::RequestFrontHeadlampOff),
        HeadlampOutcome::LogWarning(msg) => Some(DomainAction::LogWarning(msg)),
    }
}
