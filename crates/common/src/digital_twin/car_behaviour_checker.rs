//! Consistency rules between reported FSM state and [`VehicleContext`].
//!
//! Defined only in terms of [`FsmState`] and [`VehicleContext`] — no dependency on [`super::DigitalTwinCar`].
//!
//! The individual laws are pure predicates on `(&FsmState, &VehicleContext)`, gathered into
//! [`STATE_LAWS`], a named catalog, and exposed through the pure public entry point
//! [`verify_state_laws`]. These are the **building blocks**: an external/offline verifier (or
//! a test) reconstructs each `(state, ctx)` cut from a captured `PublishedTransitionRecord`
//! stream and folds `verify_state_laws` over it. The library deliberately does **not** ship a
//! journey-fold helper — that consumer-side concern lives outside the twin. The pure laws are
//! an *oracle* (tests / CI / offline / async-sampled), never a PROD hot-path gate; invariants
//! are *enforced* in the FSM transition (clamp/reject) and *announced* via diagnostics. See
//! ADR-2 / ADR-3 / Q6 in `docs/design-notes-runtime-observation.md`.

use crate::fsm::FsmState;
use crate::vehicle_state::VehicleContext;
use crate::vehicle_physics::RPM_DRIVING_THRESHOLD;

/// A single state law: a stable name plus a pure predicate on `(state, ctx)`.
///
/// The `name` is the contract a harness reports against ("which law failed"); keep it stable.
#[derive(Clone, Copy)]
pub struct StateLaw {
    pub name: &'static str,
    pub check: fn(&FsmState, &VehicleContext) -> Result<(), String>,
}

/// One failed law, tagged with the law name so callers can report *which* law failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LawViolation {
    pub law: &'static str,
    pub detail: String,
}

/// The catalog of state laws. Add new laws here as the project grows; an external verifier
/// folds these over each `(state, ctx)` cut of a captured journey.
pub const STATE_LAWS: &[StateLaw] = &[
    StateLaw {
        name: "kinetic_locking",
        check: law_kinetic_locking_holds,
    },
    StateLaw {
        name: "rpm_above_threshold",
        check: law_rpm_above_threshold_holds,
    },
];

/// Pure, public state-law entry point: evaluate the whole [`STATE_LAWS`] catalog at one
/// `(state, ctx)` cut and collect *all* violations (not just the first).
///
/// This takes no identity/health context, so it is exactly the subset that holds at any
/// single cut. [`super::DigitalTwinCar::verify_all_invariants`] wraps it with the
/// snapshot-level identity/health checks.
pub fn verify_state_laws(state: &FsmState, ctx: &VehicleContext) -> Result<(), Vec<LawViolation>> {
    let violations: Vec<LawViolation> = STATE_LAWS
        .iter()
        .filter_map(|law| {
            (law.check)(state, ctx).err().map(|detail| LawViolation {
                law: law.name,
                detail,
            })
        })
        .collect();

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

/// 1. Kinetic locking: must not report motion while logically off.
fn law_kinetic_locking_holds(state: &FsmState, ctx: &VehicleContext) -> Result<(), String> {
    if *state == FsmState::Off && ctx.powertrain.speed_kph > 0 {
        return Err(format!(
            "Safety Breach: Car is Off but moving at {} km/h",
            ctx.powertrain.speed_kph
        ));
    }
    Ok(())
}

/// 2. Dual of Idle→Driving: [`FsmState::Driving`] implies RPM strictly above
/// [`RPM_DRIVING_THRESHOLD`] (same constant as `transition_map`).
fn law_rpm_above_threshold_holds(state: &FsmState, ctx: &VehicleContext) -> Result<(), String> {
    if *state == FsmState::Driving && ctx.powertrain.wheel_rpm.front_left <= RPM_DRIVING_THRESHOLD
    {
        return Err(format!(
            "Logic Breach: State is Driving but RPM is <= {RPM_DRIVING_THRESHOLD}"
        ));
    }
    Ok(())
}
