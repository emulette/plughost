//! The voices both formats play. A voice is `velocity × volume × (1 + pressure) × cos(phase)` at
//! the frequency of its key plus its tuning in semitones, so the first sample of a note equals its
//! velocity, and a tuning or pressure change shows from the sample it arrives at. Voices on the
//! second note port are inverted. Notes on one key overlap; a note off or expression reaches the
//! voices it addresses by port, channel, key and note ID, any of which may match all.

use std::f64::consts::TAU;

pub const PORTS: usize = 2;
const VOICES: usize = 16;

/// The voices an event addresses; `None` matches any.
#[derive(Clone, Copy)]
pub struct Target {
    pub port: usize,
    pub channel: Option<u16>,
    pub key: Option<u16>,
    pub id: Option<u32>,
}

pub enum Expression {
    /// Semitones.
    Tuning(f64),
    /// 0..=1.
    Pressure(f64),
}

#[derive(Clone, Copy)]
struct Voice {
    port: usize,
    channel: u16,
    key: u16,
    id: Option<u32>,
    velocity: f64,
    /// Phase in cycles.
    phase: f64,
    tuning: f64,
    pressure: f64,
    /// Released while the sustain pedal was down.
    released: bool,
}

impl Voice {
    fn matches(&self, target: &Target) -> bool {
        self.port == target.port
            && target.channel.is_none_or(|channel| channel == self.channel)
            && target.key.is_none_or(|key| key == self.key)
            && target.id.is_none_or(|id| Some(id) == self.id)
    }
}

pub struct Voices {
    voices: [Option<Voice>; VOICES],
    sample_rate: f64,
    volume: f64,
    sustain: bool,
}

impl Voices {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            voices: [None; VOICES],
            sample_rate,
            volume: 1.0,
            sustain: false,
        }
    }

    /// Starts a voice; a note with a key and channel the target leaves open is not played.
    pub fn note_on(&mut self, target: Target, velocity: f64) {
        let (Some(channel), Some(key)) = (target.channel, target.key) else {
            return;
        };
        if let Some(free) = self.voices.iter_mut().find(|voice| voice.is_none()) {
            *free = Some(Voice {
                port: target.port,
                channel,
                key,
                id: target.id,
                velocity,
                phase: 0.0,
                tuning: 0.0,
                pressure: 0.0,
                released: false,
            });
        }
    }

    pub fn note_off(&mut self, target: Target) {
        for slot in &mut self.voices {
            if let Some(voice) = slot
                && voice.matches(&target)
            {
                if self.sustain {
                    voice.released = true;
                } else {
                    *slot = None;
                }
            }
        }
    }

    pub fn expression(&mut self, target: Target, expression: &Expression) {
        for voice in self.voices.iter_mut().flatten() {
            if voice.matches(&target) {
                match *expression {
                    Expression::Tuning(semitones) => voice.tuning = semitones,
                    Expression::Pressure(pressure) => voice.pressure = pressure,
                }
            }
        }
    }

    pub fn set_volume(&mut self, volume: f64) {
        self.volume = volume;
    }

    /// Releasing the pedal ends the notes released while it was down.
    pub fn set_sustain(&mut self, down: bool) {
        self.sustain = down;
        if !down {
            for slot in &mut self.voices {
                if slot.is_some_and(|voice| voice.released) {
                    *slot = None;
                }
            }
        }
    }

    pub fn reset(&mut self) {
        self.sustain = false;
        self.voices = [None; VOICES];
    }

    pub fn next_sample(&mut self) -> f32 {
        let mut sample = 0.0;
        for voice in self.voices.iter_mut().flatten() {
            let level = voice.velocity * self.volume * (1.0 + voice.pressure);
            let value = level * (voice.phase * TAU).cos();
            sample += if voice.port == 0 { value } else { -value };
            let pitch = f64::from(voice.key) + voice.tuning;
            let frequency = 440.0 * 2f64.powf((pitch - 69.0) / 12.0);
            voice.phase = (voice.phase + frequency / self.sample_rate).fract();
        }
        sample as f32
    }
}
