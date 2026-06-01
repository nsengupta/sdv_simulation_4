//! Visibility assembly: ambient light reading.
//!
//! Self-sufficient store of the latest ambient lux. The headlamp assembly owns
//! the lux→request hysteresis (it pairs the reading with lighting state), so
//! this assembly only ingests and holds the value. Step 2: feeds the headlamp
//! actor.

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
