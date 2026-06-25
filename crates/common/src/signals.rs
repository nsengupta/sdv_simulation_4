use socketcan::{CanFrame, EmbeddedFrame, StandardId};

pub const ID_SPEED: u16 = 0x101;
pub const ID_RPM: u16 = 0x102;
pub const ID_AMBIENT_LUX: u16 = 0x103;
/// Binary rain-presence signal from the windshield rain sensor.
/// `true` = rain detected; `false` = no rain.
pub const ID_RAIN_DETECTED: u16 = 0x104;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VssSignal {
    /// Vehicle.Speed (Unit: km/h, Scaling: 0.01). Decoded for future observed-speed ECUs; twin derives speed from RPM today.
    VehicleSpeed(f64),
    /// Vehicle.Powertrain.CombustionEngine.Speed (Unit: rpm, Scaling: 1.0)
    EngineRpm(u16),
    /// Vehicle.Cabin or exterior ambient light sensor (Unit: lux, Scaling: 1.0)
    AmbientLux(u16),
    /// Vehicle.Body.Windshield.Front.WipingSystem.RainSensor — binary detection.
    /// `true` = rain present; `false` = rain absent.
    RainDetected(bool),
}

impl VssSignal {
    /// Decode a raw CAN Frame into a VSS Signal
    pub fn from_can_frame(frame: &CanFrame) -> Option<Self> {
        // Only standard (11-bit) IDs are supported here; extended / FD-only shapes are rejected.
        // For a standard frame, `as_raw()` is the numeric ID (0..=0x7FF).
        let id = match frame.id() {
            socketcan::Id::Standard(s) => s.as_raw(),
            _ => return None,
        };

        let data = frame.data();
        if data.len() < 2 { return None; }

        match id {
            ID_SPEED => {
                let raw = u16::from_be_bytes([data[0], data[1]]);
                Some(Self::VehicleSpeed(raw as f64 / 100.0))
            }
            ID_RPM => {
                let raw = u16::from_be_bytes([data[0], data[1]]);
                Some(Self::EngineRpm(raw))
            }
            ID_AMBIENT_LUX => {
                let raw = u16::from_be_bytes([data[0], data[1]]);
                Some(Self::AmbientLux(raw))
            }
            ID_RAIN_DETECTED => {
                Some(Self::RainDetected(data[0] != 0))
            }
            _ => None,
        }
    }

    /// Encode a VSS Signal into a raw CAN Frame
    pub fn to_can_frame(&self) -> Result<CanFrame, socketcan::Error> {
        // 1. Helper for safe ID creation (`socketcan::StandardId`).
        // `StandardId::new` accepts only values valid for an 11-bit standard CAN ID; otherwise `None`.
        let can_standard_id = |id: u16| {
            StandardId::new(id).ok_or_else(|| {
                socketcan::Error::from(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("Invalid CAN ID: {:#X}", id),
                ))
            })
        };

        // 2. Helper for safe Frame creation
        let build_frame = |id: u16, data: &[u8]| {
            let cid = can_standard_id(id)?;
            CanFrame::new(cid, data).ok_or_else(|| {
                socketcan::Error::from(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Data length exceeds CAN standard (8 bytes)",
                ))
            })
        };

        match self {
            Self::VehicleSpeed(val) => {
                let scaled = (val * 100.0) as u16;
                build_frame(ID_SPEED, &scaled.to_be_bytes())
            }
            Self::EngineRpm(val) => {
                build_frame(ID_RPM, &val.to_be_bytes())
            }
            Self::AmbientLux(val) => {
                build_frame(ID_AMBIENT_LUX, &val.to_be_bytes())
            }
            Self::RainDetected(val) => {
                // Byte 0: 0x01 = rain, 0x00 = no rain. Byte 1: reserved zero.
                build_frame(ID_RAIN_DETECTED, &[*val as u8, 0])
            }
        }
    }
}