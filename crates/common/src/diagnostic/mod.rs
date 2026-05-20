//! Diagnostic event types for streamed observability from the digital twin.
//!
//! The twin emits [`DiagnosticMessage`] values through an injected channel.
//! The runtime decides who reads the RX side and how to display them.

use crate::fsm::{FsmState};

/// Severity classification for diagnostic messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticLevel {
    Info,
    Action,
    Alert,
    Warning,
    Error,
}

/// A single diagnostic event emitted by the digital twin actor or its components.
#[derive(Debug, Clone)]
pub struct DiagnosticMessage {
    pub level: DiagnosticLevel,
    pub source: &'static str,
    pub message: String,
    pub timestamp_utc_nanos: u128,
}

impl DiagnosticMessage {
    pub fn new(level: DiagnosticLevel, source: &'static str, message: impl Into<String>) -> Self {
        Self {
            level,
            source,
            message: message.into(),
            timestamp_utc_nanos: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        }
    }
}

/// Shorthand constructors for common twin diagnostics.
impl DiagnosticMessage {
    pub fn info(source: &'static str, msg: impl Into<String>) -> Self {
        Self::new(DiagnosticLevel::Info, source, msg)
    }

    pub fn action(source: &'static str, msg: impl Into<String>) -> Self {
        Self::new(DiagnosticLevel::Action, source, msg)
    }

    pub fn alert(source: &'static str, msg: impl Into<String>) -> Self {
        Self::new(DiagnosticLevel::Alert, source, msg)
    }

    pub fn warning(source: &'static str, msg: impl Into<String>) -> Self {
        Self::new(DiagnosticLevel::Warning, source, msg)
    }

    pub fn error(source: &'static str, msg: impl Into<String>) -> Self {
        Self::new(DiagnosticLevel::Error, source, msg)
    }
}

impl std::fmt::Display for DiagnosticMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let icon = match self.level {
            DiagnosticLevel::Info => "ℹ️",
            DiagnosticLevel::Action => "⚡",
            DiagnosticLevel::Alert => "🚨",
            DiagnosticLevel::Warning => "⚠️",
            DiagnosticLevel::Error => "❌",
        };
        write!(f, "[{icon}][{}] {}", self.source, self.message)
    }
}

// --- Sink trait (analogous to TransitionRecordSink) ---

/// Abstract sink for diagnostic messages emitted by the digital twin.
///
/// The twin is unconcerned with who reads the other end; the runtime injects the
/// appropriate implementation and decides on display / persistence.
pub trait DiagnosticSink: Send + Sync {
    fn try_emit(&self, msg: DiagnosticMessage) -> Result<(), DiagnosticSinkError>;
}

/// Errors that can occur when emitting a diagnostic message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSinkError {
    Full,
    Closed,
}

// --- Tokio mpsc implementation ---

use tokio::sync::mpsc;

/// Wraps a `tokio::sync::mpsc::Sender` as a [`DiagnosticSink`].
///
/// The twin calls `try_emit` (non-blocking); the receiver is on the runtime side.
pub struct TokioMpscDiagnosticSink {
    tx: mpsc::UnboundedSender<DiagnosticMessage>,
}

impl TokioMpscDiagnosticSink {
    pub fn new(tx: mpsc::UnboundedSender<DiagnosticMessage>) -> Self {
        Self { tx }
    }
}

impl DiagnosticSink for TokioMpscDiagnosticSink {
    fn try_emit(&self, msg: DiagnosticMessage) -> Result<(), DiagnosticSinkError> {
        self.tx.send(msg).map_err(|_| DiagnosticSinkError::Closed)
    }
}

// --- Domain-specific helpers (for cleaner call sites) ---

pub fn diag_state_transition(identity: &str, new_state: &FsmState) -> DiagnosticMessage {
    DiagnosticMessage::info("VirtualCarActor", format!("[{identity}]: Transitioned to {new_state:?}"))
}

pub fn diag_timer_tick(identity: &str) -> DiagnosticMessage {
    DiagnosticMessage::info("VirtualCarActor", format!("[{identity}]: received heartbeat TimerTick"))
}

pub fn diag_actuation_failure(identity: &str, action: &str, err: &str) -> DiagnosticMessage {
    DiagnosticMessage::error("VirtualCarActor", format!("[{identity}]: actuation failure for {action}: {err}"))
}

pub fn diag_transition_sink_full(identity: &str) -> DiagnosticMessage {
    DiagnosticMessage::warning("VirtualCarActor", format!("[{identity}]: dropping transition record: sink full"))
}

pub fn diag_transition_sink_closed(identity: &str) -> DiagnosticMessage {
    DiagnosticMessage::warning("VirtualCarActor", format!("[{identity}]: dropping transition record: sink closed"))
}

// --- Stdout observer (for the runtime side) ---

/// Spawns a task that reads [`DiagnosticMessage`] values from `rx` and prints each
/// to stdout (or stderr for error-level).
///
/// The runtime calls this when it wants to attach stdout to the twin's diagnostic stream.
pub fn spawn_stdout_diagnostic_observer(
    mut rx: mpsc::UnboundedReceiver<DiagnosticMessage>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        use std::io::Write;
        while let Some(msg) = rx.recv().await {
            match msg.level {
                DiagnosticLevel::Error | DiagnosticLevel::Alert => {
                    let _ = writeln!(std::io::stderr(), "{msg}");
                }
                _ => {
                    let _ = writeln!(std::io::stdout(), "{msg}");
                }
            }
        }
    })
}
