//! Gateway wiring: controller install, background loops, and CAN read loop. Keeps `main` thin.

use anyhow::Result;
use common::{
    ACK_OFF, ACK_ON, MSG_ACK_OFF, MSG_ACK_ON, MSG_NACK_OFF, MSG_NACK_ON, NACK_OFF, NACK_ON,
    ActuationCommand, PhysicalCarVocabulary, VehicleController,
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

pub struct GatewayLaunchConfig<'a> {
    pub car_identity: &'a str,
    pub print_timer_tick: bool,
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

    let runtime_options = VehicleControllerRuntimeOptions {
        log_timer_tick: launch.print_timer_tick,
        actuation_command_tx: Some(actuation_cmd_tx),
        diagnostic_tx: Some(diag_tx),
        ..VehicleControllerRuntimeOptions::default()
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
        "⚡ Gateway on {} — CAN → VehicleEvent → PhysicalCarVocabulary → DigitalTwinCarVocabulary → VirtualCarActor",
        launch.can_interface
    );
    println!(
        "[gateway] front-headlamp CMD egress on CAN; run `cargo run -p front_headlamp_actuator` for ACK/NACK"
    );
    if launch.print_timer_tick {
        println!("[gateway] TimerTick heartbeat logging enabled (--print-timer-tick)");
    }

    let (can_tx, can_rx) = mpsc::unbounded_channel();
    let _can_reader = spawn_can_reader_thread(
        launch.can_interface.to_string(),
        front_headlamp_policy,
        can_tx,
    )?;
    run_can_ingress_dispatch_loop(controller, can_rx).await
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
                            eprintln!(
                                "[actuation-can-ingress ignored]: reason={reason} session={} seq={}",
                                payload.session_id, payload.sequence_no
                            );
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

fn log_front_headlamp_ingress(session: u16, sequence: u32, physical: &PhysicalCarVocabulary) {
    match physical {
        PhysicalCarVocabulary::FrontHeadlampCommandConfirmed { on_command: true } => {
            println!(
                "[actuation-can-ingress session={session} seq={sequence}]: {ACK_ON} {MSG_ACK_ON}"
            );
        }
        PhysicalCarVocabulary::FrontHeadlampCommandConfirmed { on_command: false } => {
            println!(
                "[actuation-can-ingress session={session} seq={sequence}]: {ACK_OFF} {MSG_ACK_OFF}"
            );
        }
        PhysicalCarVocabulary::FrontHeadlampCommandRejected { on_command: true } => {
            println!(
                "[actuation-can-ingress session={session} seq={sequence}]: {NACK_ON} {MSG_NACK_ON}"
            );
        }
        PhysicalCarVocabulary::FrontHeadlampCommandRejected { on_command: false } => {
            println!(
                "[actuation-can-ingress session={session} seq={sequence}]: {NACK_OFF} {MSG_NACK_OFF}"
            );
        }
        _ => {}
    }
}

async fn run_can_ingress_dispatch_loop(
    controller: VehicleController,
    mut rx: mpsc::UnboundedReceiver<CanIngressEnvelope>,
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
                log_front_headlamp_ingress(session, sequence, &physical);
                controller
                    .submit_physical_car_event(physical)
                    .await
                    .map_err(|e| anyhow::anyhow!("submit physical car event: {e:?}"))?;
            }
        }
    }
    Err(anyhow::anyhow!(
        "CAN ingress channel closed: reader thread exited"
    ))
}
