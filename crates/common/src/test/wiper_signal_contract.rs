//! Step 7 contract: `VssSignal::RainDetected` CAN encode/decode round-trip.

use crate::signals::VssSignal;
use socketcan::EmbeddedFrame;

#[test]
fn given_rain_detected_true_when_can_round_trip_then_preserves_true() {
    let signal = VssSignal::RainDetected(true);
    let frame = signal.to_can_frame().expect("encode RainDetected(true)");
    let decoded = VssSignal::from_can_frame(&frame);
    assert_eq!(decoded, Some(VssSignal::RainDetected(true)));
}

#[test]
fn given_rain_detected_false_when_can_round_trip_then_preserves_false() {
    let signal = VssSignal::RainDetected(false);
    let frame = signal.to_can_frame().expect("encode RainDetected(false)");
    let decoded = VssSignal::from_can_frame(&frame);
    assert_eq!(decoded, Some(VssSignal::RainDetected(false)));
}

#[test]
fn given_rain_detected_when_encoded_then_can_id_distinct_from_rpm_and_lux() {
    let rain_frame = VssSignal::RainDetected(true)
        .to_can_frame()
        .expect("encode rain");
    let lux_frame = VssSignal::AmbientLux(50)
        .to_can_frame()
        .expect("encode lux");
    let rpm_frame = VssSignal::EngineRpm(1000)
        .to_can_frame()
        .expect("encode rpm");
    assert_ne!(rain_frame.id(), lux_frame.id(), "rain vs lux CAN ID collision");
    assert_ne!(rain_frame.id(), rpm_frame.id(), "rain vs rpm CAN ID collision");
}

#[test]
fn given_rain_detected_true_and_false_when_encoded_then_same_can_id_and_distinct_data() {
    let frame_true = VssSignal::RainDetected(true)
        .to_can_frame()
        .expect("encode true");
    let frame_false = VssSignal::RainDetected(false)
        .to_can_frame()
        .expect("encode false");
    assert_eq!(
        frame_true.id(),
        frame_false.id(),
        "true/false variants must share the same CAN ID"
    );
    assert_ne!(
        frame_true.data()[0],
        frame_false.data()[0],
        "data byte must differ between true and false"
    );
}
