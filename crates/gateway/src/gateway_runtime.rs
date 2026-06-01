//! Gateway wiring: controller install, background loops, and CAN read loop. Keeps `main` thin.

use anyhow::Result;
use common::facade::{
    ACK_OFF, ACK_ON, ActuationCommand, MSG_ACK_OFF, MSG_ACK_ON, MSG_NACK_OFF, MSG_NACK_ON,
    NACK_OFF, NACK_ON, PhysicalCarVocabulary, PublishedTransitionRecord, VehicleController,
    VehicleControllerRuntimeOptions, VehicleEvent, VssSignal, spawn_stdout_diagnostic_observer,
};
use socketcan::{CanSocket, Socket};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::mpsc;
use vehicle_device_bus::devices::front_headlamp::can::{decode_payload_from_can_frame, encode_command_frame};
use vehicle_device_bus::devices::front_headlamp::policy::{FrontHeadlampPolicy, FrontHeadlampPolicyDecision};

use crate::ingress;

/// Default SocketCAN interface (matches emulator and front_headlamp_actuator).
pub const DEFAULT_CAN_INTERFACE: &str = "vcan0";

const TIMER_TICK_MS: u64 = 100;
const ACTUATION_COMMAND_CHANNEL_CAPACITY: usize = 64;
/// Bound on the off-task ingress-log queue. Logging is best-effort: a frozen console
/// (Ctrl-S / XOFF) must not stall the CAN ingress dispatch loop (which also delivers ACKs to the
/// twin), so lines are dropped once this fills.
const INGRESS_LOG_CHANNEL_CAPACITY: usize = 512;

pub struct GatewayLaunchConfig<'a> {
    pub car_identity: &'a str,
    pub print_timer_tick: bool,
    pub print_transitions: bool,
    /// Log ignored headlamp ingress (e.g. command-frame echo on shared CAN). Off by default for demos.
    pub trace_actuation_ingress: bool,
    pub can_interface: &'a str,
}

/// Messages forwarded from the dedicated CAN reader thread into async gateway flow.
enum CanIngressEnvelope {
    Physical(PhysicalCarVocabulary),
    ActuationResponse {
        physical: PhysicalCarVocabulary,
        session: u16,
        sequence: u32,
    },
}

pub async fn run(launch: GatewayLaunchConfig<'_>) -> Result<()> {
    let front_headlamp_policy = Arc::new(Mutex::new(FrontHeadlampPolicy::default()));
    let (actuation_cmd_tx, actuation_cmd_rx) = mpsc::channel(ACTUATION_COMMAND_CHANNEL_CAPACITY);

    // Diagnostic channel: twin emits DiagnosticMessage, runtime observes.
    let (diag_tx, diag_rx) = mpsc::unbounded_channel();
    let _diag_observer = spawn_stdout_diagnostic_observer(diag_rx);

    // Transition channel: twin emits PublishedTransitionRecord, optionally printed to stdout.
    let transition_tx = if launch.print_transitions {
        let (tx, mut rx) = mpsc::channel::<PublishedTransitionRecord>(256);
        tokio::spawn(async move {
            while let Some(record) = rx.recv().await {
                println!(
                    "[transition] car={} seq={} at_unix={:?} {:?} {:?} -> {:?} actions={:?}",
                    record.car_identity,
                    record.record_seq,
                    record.at_unix,
                    record.event,
                    record.old_state,
                    record.next_state,
                    record.actions,
                );
            }
        });
        Some(tx)
    } else {
        None
    };

    let runtime_options = VehicleControllerRuntimeOptions {
        log_timer_tick: launch.print_timer_tick,
        actuation_command_tx: Some(actuation_cmd_tx),
        diagnostic_tx: Some(diag_tx),
        transition_tx,
    };

    let (controller, _join) = VehicleController::install_and_start_with_options(
        launch.car_identity.to_string(),
        runtime_options,
    )
    .await
    .map_err(|e| anyhow::anyhow!("spawn actor: {e}"))?;

    spawn_front_headlamp_command_publisher(
        actuation_cmd_rx,
        launch.can_interface.to_string(),
        front_headlamp_policy.clone(),
    );

    controller
        .send_power_on()
        .await
        .map_err(|e| anyhow::anyhow!("PowerOn: {e:?}"))?;

    spawn_timer_tick_loop(controller.clone());

    println!(
        "⚡ Gateway on {} — CAN → VehicleEvent → PhysicalCarVocabulary → VehicleController",
        launch.can_interface
    );
    println!(
        "[gateway] front-headlamp CMD egress on CAN; run `cargo run -p front_headlamp_actuator` for ACK/NACK"
    );
    if launch.print_timer_tick {
        println!("[gateway] TimerTick heartbeat logging enabled (--print-timer-tick)");
    }
    if launch.print_transitions {
        println!("[gateway] FSM transition logging enabled (--print-transitions)");
    }
    if launch.trace_actuation_ingress {
        println!(
            "[gateway] actuation ingress trace enabled (--trace-actuation-ingress; ignored CMD/correlation lines)"
        );
    }

    // Off-hot-path ingress logger: a frozen console must not block ACK delivery to the twin.
    let (ingress_log_tx, mut ingress_log_rx) =
        mpsc::channel::<String>(INGRESS_LOG_CHANNEL_CAPACITY);
    tokio::spawn(async move {
        while let Some(line) = ingress_log_rx.recv().await {
            println!("{line}");
        }
    });

    let (can_tx, can_rx) = mpsc::unbounded_channel();
    let _can_reader = spawn_can_reader_thread(
        launch.can_interface.to_string(),
        front_headlamp_policy,
        launch.trace_actuation_ingress,
        can_tx,
    )?;
    run_can_ingress_dispatch_loop(controller, can_rx, ingress_log_tx).await
}

fn spawn_timer_tick_loop(controller: VehicleController) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(TIMER_TICK_MS)).await;
            let physical = ingress::vehicle_event_to_physical_vocabulary(VehicleEvent::TimerTick);
            let _ = controller.submit_physical_car_event(physical).await;
        }
    });
}

/// Dedicated OS thread for blocking `read_frame()` loop.
fn spawn_can_reader_thread(
    can_interface: String,
    front_headlamp_policy: Arc<Mutex<FrontHeadlampPolicy>>,
    trace_actuation_ingress: bool,
    tx: mpsc::UnboundedSender<CanIngressEnvelope>,
) -> Result<JoinHandle<()>> {
    let socket = CanSocket::open(&can_interface)?;
    let thread_name = format!("gateway-can-reader-{can_interface}");
    let handle = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            loop {
                let frame = match socket.read_frame() {
                    Ok(frame) => frame,
                    Err(e) => {
                        eprintln!("[gateway-can-reader]: read_frame failed: {e:?}");
                        continue;
                    }
                };
                if let Some(sig) = VssSignal::from_can_frame(&frame) {
                    if matches!(sig, VssSignal::VehicleSpeed(_)) {
                        // Observed speed ECU path (slip/clutch/gear) — future milestone.
                        continue;
                    }
                    let ev = VehicleEvent::TelemetryUpdate(sig);
                    let physical = ingress::vehicle_event_to_physical_vocabulary(ev);
                    if tx.send(CanIngressEnvelope::Physical(physical)).is_err() {
                        break;
                    }
                    continue;
                }
                if let Some(payload) = decode_payload_from_can_frame(&frame) {
                    let decision = {
                        let mut policy = front_headlamp_policy
                            .lock()
                            .expect("front-headlamp policy lock");
                        policy.on_response(payload)
                    };
                    match decision {
                        FrontHeadlampPolicyDecision::Accept {
                            physical,
                            session,
                            sequence,
                        } => {
                            if tx
                                .send(CanIngressEnvelope::ActuationResponse {
                                    physical,
                                    session,
                                    sequence,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                        FrontHeadlampPolicyDecision::Ignore(reason) => {
                            if trace_actuation_ingress {
                                eprintln!(
                                    "[actuation-can-ingress trace ignored]: reason={reason} session={} seq={}",
                                    payload.session_id, payload.sequence_no
                                );
                            }
                        }
                    }
                }
            }
        })?;
    Ok(handle)
}

/// Egress: twin actuation intent → policy pending state → CMD frame on CAN.
fn spawn_front_headlamp_command_publisher(
    mut actuation_cmd_rx: mpsc::Receiver<ActuationCommand>,
    can_interface: String,
    front_headlamp_policy: Arc<Mutex<FrontHeadlampPolicy>>,
) {
    tokio::spawn(async move {
        let socket = match CanSocket::open(&can_interface) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[gateway]: cannot open CAN {can_interface} for front-headlamp CMD TX: {e}"
                );
                return;
            }
        };
        while let Some(cmd) = actuation_cmd_rx.recv().await {
            {
                let mut policy = front_headlamp_policy
                    .lock()
                    .expect("front-headlamp policy lock");
                policy.on_command_sent(&cmd);
            }
            match encode_command_frame(&cmd) {
                Ok(frame) => {
                    if let Err(e) = socket.write_frame(&frame) {
                        eprintln!("[gateway]: front-headlamp CMD write_frame failed: {e:?}");
                    }
                }
                Err(e) => eprintln!("[gateway]: encode front-headlamp CMD failed: {e:?}"),
            }
        }
    });
}

/// Format the wire-level ingress line for a headlamp ACK/NACK, or `None` for other events.
/// Formatting only — emitting is the caller's job (off the hot path, via a bounded log channel).
fn format_front_headlamp_ingress(
    session: u16,
    sequence: u32,
    physical: &PhysicalCarVocabulary,
) -> Option<String> {
    let (icon, msg) = match physical {
        PhysicalCarVocabulary::FrontHeadlampCommandConfirmed { on_command: true } => {
            (ACK_ON, MSG_ACK_ON)
        }
        PhysicalCarVocabulary::FrontHeadlampCommandConfirmed { on_command: false } => {
            (ACK_OFF, MSG_ACK_OFF)
        }
        PhysicalCarVocabulary::FrontHeadlampCommandRejected { on_command: true } => {
            (NACK_ON, MSG_NACK_ON)
        }
        PhysicalCarVocabulary::FrontHeadlampCommandRejected { on_command: false } => {
            (NACK_OFF, MSG_NACK_OFF)
        }
        _ => return None,
    };
    Some(format!(
        "[actuation-can-ingress session={session} seq={sequence}]: {icon} {msg}"
    ))
}

async fn run_can_ingress_dispatch_loop(
    controller: VehicleController,
    mut rx: mpsc::UnboundedReceiver<CanIngressEnvelope>,
    ingress_log_tx: mpsc::Sender<String>,
) -> Result<()> {
    while let Some(msg) = rx.recv().await {
        match msg {
            CanIngressEnvelope::Physical(physical) => {
                controller
                    .submit_physical_car_event(physical)
                    .await
                    .map_err(|e| anyhow::anyhow!("submit physical car event: {e:?}"))?;
            }
            CanIngressEnvelope::ActuationResponse {
                physical,
                session,
                sequence,
            } => {
                // Deliver the ACK/NACK to the twin first; logging is best-effort and off the hot
                // path (non-blocking `try_send`, dropped on console back-pressure).
                let line = format_front_headlamp_ingress(session, sequence, &physical);
                controller
                    .submit_physical_car_event(physical)
                    .await
                    .map_err(|e| anyhow::anyhow!("submit physical car event: {e:?}"))?;
                if let Some(line) = line {
                    let _ = ingress_log_tx.try_send(line);
                }
            }
        }
    }
    Err(anyhow::anyhow!(
        "CAN ingress channel closed: reader thread exited"
    ))
}
