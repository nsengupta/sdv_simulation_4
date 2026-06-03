//! Visibility zone (L1): alphabet + context.
//!
//! Dumb lux store; headlamp owns policy. **ADR-5:** [`VisibilityState`],
//! [`VisibilityMessage`], [`VisibilityOutcome`].

/// L1 visibility snapshot.
pub type VisibilityState = VisibilityContext;

/// Inputs — ambient telemetry (L4 may fan-out to headlamp as well).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityMessage {
    AmbientLux(u16),
}

/// No zone-local egress in milestone 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityOutcome {
    #[doc(hidden)]
    __NoEgress,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VisibilityContext {
    pub ambient_lux: u16,
}

impl Default for VisibilityContext {
    fn default() -> Self {
        Self { ambient_lux: 100 }
    }
}

impl VisibilityContext {
    pub fn apply_lux(&mut self, lux: u16) {
        self.ambient_lux = lux;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_lux_stores_value() {
        let mut v = VisibilityContext::default();
        v.apply_lux(42);
        assert_eq!(v.ambient_lux, 42);
    }
}
