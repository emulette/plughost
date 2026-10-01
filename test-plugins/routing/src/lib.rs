//! Independent routing fixture: auxiliary buses precede main buses in both native formats. The
//! main buses take any portable layout CLAP and VST3 express, on the CLAP side through port
//! configurations or configurable ports. Three
//! event inputs feed one event output: notes from input port `p` of the first two leave
//! transposed up by 12 × (p + 1) semitones with controller `KEY_CONTROLLER` carrying the input
//! key, system exclusive messages are echoed, and `FLOOD` makes the fixture exceed any block's
//! event budget. A note off on `LATE_KEY` leaves at the block's length, past its last frame, and
//! the CLAP side also reports the note's end. Notes and their expressions on `EXPRESSION_PORT`
//! leave unchanged: CLAP note and note expression events, and VST3 notes, poly pressure and note
//! expression values.
mod clap;
mod errors;
mod vst;

const KEY_CONTROLLER: u8 = 20;
const LATE_KEY: u8 = 0;
const FLOOD: [u8; 4] = [0xF0, 0x7D, 0x7F, 0xF7];
const FLOOD_EVENTS: usize = 5000;
const EVENT_INPUTS: u32 = 3;
const EXPRESSION_PORT: usize = 2;

fn transpose(port: usize, key: u8) -> u8 {
    (usize::from(key) + 12 * (port + 1)).min(127) as u8
}

/// Per-frame values of the fixture's two parameters, starting at `initial`. Each point sets its
/// parameter from its frame on; later points at one frame win. One pass over the block.
fn frame_values(
    initial: [f64; 2],
    frames: usize,
    points: impl IntoIterator<Item = (usize, usize, f64)>,
) -> Vec<[f64; 2]> {
    let mut changes = vec![[None; 2]; frames];
    for (frame, id, value) in points {
        if let Some(change) = changes.get_mut(frame) {
            change[id] = Some(value);
        }
    }
    let mut current = initial;
    changes
        .into_iter()
        .map(|change| {
            for (value, change) in current.iter_mut().zip(change) {
                if let Some(change) = change {
                    *value = change;
                }
            }
            current
        })
        .collect()
}
