use super::*;
use plughost_core::{BarPosition, TimeSignature, Transport};

fn transport() -> Transport {
    Transport {
        sample_position: 0,
        beat_position: None,
        bar_position: None,
        tempo: None,
        time_signature: None,
        playing: true,
        loop_region: None,
    }
}

fn nothing() -> Outputs {
    Outputs {
        tempo: std::ptr::null_mut(),
        numerator: std::ptr::null_mut(),
        denominator: std::ptr::null_mut(),
        beat: std::ptr::null_mut(),
        to_next_beat: std::ptr::null_mut(),
        downbeat: std::ptr::null_mut(),
    }
}

#[test]
fn a_tempo_alone_reaches_a_unit_that_asks_only_for_the_tempo() {
    let tempo_only = Transport {
        tempo: Some(90.0),
        ..transport()
    };
    let mut tempo = 0.0;
    let result = fill(
        Some(tempo_only),
        48_000.0,
        Outputs {
            tempo: &mut tempo,
            ..nothing()
        },
    );
    assert_eq!((result, tempo), (Bool::YES, 90.0));

    let mut beat = -1.0;
    let result = fill(
        Some(tempo_only),
        48_000.0,
        Outputs {
            tempo: &mut tempo,
            beat: &mut beat,
            ..nothing()
        },
    );
    assert_eq!((result, beat), (Bool::NO, -1.0));
    assert_eq!(fill(None, 48_000.0, nothing()), Bool::NO);
}

#[test]
fn a_full_transport_fills_every_value() {
    let full = Transport {
        tempo: Some(120.0),
        beat_position: Some(4.5),
        bar_position: Some(BarPosition {
            start: 4.0,
            number: 1,
        }),
        time_signature: Some(TimeSignature {
            numerator: 3,
            denominator: 4,
        }),
        ..transport()
    };
    let (mut tempo, mut numerator, mut denominator) = (0.0, 0.0, 0);
    let (mut beat, mut to_next_beat, mut downbeat) = (0.0, 0, 0.0);
    let result = fill(
        Some(full),
        48_000.0,
        Outputs {
            tempo: &mut tempo,
            numerator: &mut numerator,
            denominator: &mut denominator,
            beat: &mut beat,
            to_next_beat: &mut to_next_beat,
            downbeat: &mut downbeat,
        },
    );
    assert_eq!(result, Bool::YES);
    assert_eq!(
        (tempo, numerator, denominator, beat, to_next_beat, downbeat),
        (120.0, 3.0, 4, 4.5, 12_000, 4.0)
    );
}
