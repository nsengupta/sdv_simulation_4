//! Ledger-shaped stdout for `--print-transitions-only` (ANSI when stdout is a TTY).

use common::facade::PublishedTransitionRecord;
use common::published::{PublishedFsmEvent, PublishedFsmState};

pub fn spawn_transition_log_task(
    rx: tokio::sync::mpsc::Receiver<PublishedTransitionRecord>,
    color: bool,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut rx = rx;
        while let Some(record) = rx.recv().await {
            println!("{}", format_transition_record(&record, color));
        }
    })
}

fn format_transition_record(record: &PublishedTransitionRecord, color: bool) -> String {
    let seq = format!("seq={:<4}", record.record_seq);
    let event = format!("{:?}", record.event);
    let transition = format_state_transition(&record.old_state, &record.next_state, color);
    let actions = if record.actions.is_empty() {
        "actions=[]".to_string()
    } else {
        format!("actions={:?}", record.actions)
    };
    let headlamp = format!("headlamp={:?}", record.current_ctx.headlamp.state);

    if !color {
        return format!("{seq}  {event}  {transition}  {actions}  {headlamp}");
    }

    format!(
        "{DIM}{seq}{RESET}  {event_color}{event}{RESET}  {transition}  {action_color}{actions}{RESET}  {DIM}{headlamp}{RESET}",
        DIM = ansi::DIM,
        RESET = ansi::RESET,
        event_color = event_color(&record.event),
        action_color = if record.actions.is_empty() {
            ansi::DIM
        } else {
            ansi::MAGENTA
        },
    )
}

fn format_state_transition(
    old: &PublishedFsmState,
    new: &PublishedFsmState,
    color: bool,
) -> String {
    if !color {
        return format!("{old:?} → {new:?}");
    }

    format!(
        "{DIM}{old}{RESET}{DIM} → {RESET}{next}{RESET}",
        DIM = ansi::DIM,
        RESET = ansi::RESET,
        old = format!("{old:?}"),
        next = format_next_state(new),
    )
}

fn format_next_state(state: &PublishedFsmState) -> String {
    let label = format!("{state:?}");
    format!(
        "{paint}{BOLD}{label}{RESET}",
        paint = next_state_color(state),
        BOLD = ansi::BOLD,
        RESET = ansi::RESET,
    )
}

fn event_color(event: &PublishedFsmEvent) -> &'static str {
    match event {
        PublishedFsmEvent::Internal(_) => ansi::MAGENTA,
        PublishedFsmEvent::TimerTick => ansi::DIM,
        PublishedFsmEvent::FrontHeadlampActuationIncomplete { .. } => ansi::YELLOW,
        _ => ansi::CYAN,
    }
}

/// Highlight the **exit** operational mode (no red — reserved for errors elsewhere).
fn next_state_color(state: &PublishedFsmState) -> &'static str {
    match state {
        PublishedFsmState::Off => ansi::DIM,
        PublishedFsmState::PreparingToStart | PublishedFsmState::PreparingToStop => ansi::CYAN,
        PublishedFsmState::Idle => ansi::YELLOW,
        PublishedFsmState::Driving => ansi::GREEN,
        PublishedFsmState::DrivingDangerously => ansi::MAGENTA,
        PublishedFsmState::ExtremeOperationWarning { .. } => ansi::BRIGHT_YELLOW,
    }
}

mod ansi {
    pub const RESET: &str = "\x1b[0m";
    pub const BOLD: &str = "\x1b[1m";
    pub const DIM: &str = "\x1b[2m";
    pub const CYAN: &str = "\x1b[36m";
    pub const YELLOW: &str = "\x1b[33m";
    pub const BRIGHT_YELLOW: &str = "\x1b[93m";
    pub const GREEN: &str = "\x1b[32m";
    pub const MAGENTA: &str = "\x1b[35m";
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::published::{
        PublishedDomainAction, PublishedFsmEvent, PublishedFsmState, PublishedHeadlampContext,
        PublishedHeadlampState, PublishedHealthContext, PublishedPowertrainContext,
        PublishedTransitionRecord, PublishedVehicleContext, PublishedVisibilityContext,
        PublishedWheelRpm,
    };

    fn sample_record() -> PublishedTransitionRecord {
        PublishedTransitionRecord {
            car_identity: "test-car".to_string(),
            session_epoch_unix_nanos: 1,
            record_seq: 3,
            at_unix: std::time::Duration::from_secs(100),
            event: PublishedFsmEvent::UpdateAmbientLux(20),
            old_state: PublishedFsmState::Idle,
            next_state: PublishedFsmState::Driving,
            old_ctx: empty_ctx(),
            current_ctx: empty_ctx(),
            actions: vec![PublishedDomainAction::RequestFrontHeadlampOn],
        }
    }

    fn empty_ctx() -> PublishedVehicleContext {
        PublishedVehicleContext {
            powertrain: PublishedPowertrainContext {
                wheel_rpm: PublishedWheelRpm {
                    front_left: 0,
                    front_right: 0,
                    rear_left: 0,
                    rear_right: 0,
                },
                speed_kph: 0,
            },
            health: PublishedHealthContext {
                fuel_level_pct: 100,
                oil_pressure_kpa: 100,
                tyre_pressure_ok: true,
            },
            visibility: PublishedVisibilityContext { ambient_lux: 0 },
            headlamp: PublishedHeadlampContext {
                state: PublishedHeadlampState::Off,
                ack_pending_since_unix: None,
            },
        }
    }

    #[test]
    fn plain_format_includes_seq_event_and_state_arrow() {
        let line = format_transition_record(&sample_record(), false);
        assert!(line.contains("seq=3"));
        assert!(line.contains("UpdateAmbientLux(20)"));
        assert!(line.contains("Idle → Driving"));
        assert!(line.contains("RequestFrontHeadlampOn"));
    }

    #[test]
    fn colored_transition_highlights_next_state_separately_from_old() {
        let transition = format_state_transition(
            &PublishedFsmState::Driving,
            &PublishedFsmState::DrivingDangerously,
            true,
        );
        assert!(transition.contains("Driving"));
        assert!(transition.contains("DrivingDangerously"));
        assert!(transition.contains("→"));
        // old dimmed, next bold+magenta — distinct escape sequences around the target state.
        assert!(transition.contains(ansi::MAGENTA));
        assert!(transition.contains(ansi::BOLD));
    }
}
