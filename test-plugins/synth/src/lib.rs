//! CLAP test instrument whose output shows exactly when notes and MIDI arrive. One voice per note
//! port: from a note on, a voice is `velocity × volume × cos(phase)` at the key's frequency, so
//! the first sample of a note equals its velocity; the voice stops at the sample of its note off,
//! or, when the sustain pedal (control change 64) is down, at the sample the pedal is released.
//! Every output channel is the first port's voice minus the second's. MIDI control change 7 and
//! the system exclusive message `F0 7D 01 vv F7` set the volume to `vv / 127`. Both note ports
//! take CLAP and MIDI note events. Note ons for keys 0 to 2 drive the editor (see `gui.rs`)
//! instead of sounding.

mod errors;
mod gui;

use std::cell::RefCell;
use std::f64::consts::TAU;
use std::sync::atomic::{AtomicU8, Ordering};

use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::gui::{HostGui, PluginGui};
use clack_extensions::note_ports::{
    NoteDialect, NoteDialects, NotePortInfo, NotePortInfoWriter, PluginNotePorts,
    PluginNotePortsImpl,
};
use clack_plugin::events::Match;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::plugin::features::{INSTRUMENT, STEREO, SYNTHESIZER};
use clack_plugin::prelude::*;

const ID: &str = "com.studio.plughost.test-synth";
const NAME: &str = "plughost test synth";
const VOLUME_CONTROLLER: u8 = 7;
const SUSTAIN_CONTROLLER: u8 = 64;
const NOTE_PORTS: usize = 2;

pub struct TestSynth;

impl Plugin for TestSynth {
    type AudioProcessor<'a> = Voice<'a>;
    type Shared<'a> = Shared<'a>;
    type MainThread<'a> = MainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&Shared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginNotePorts>()
            .register::<PluginGui>();
    }
}

pub struct Shared<'a> {
    host: HostSharedHandle<'a>,
    gui: Option<HostGui>,
    /// An editor request from the audio thread, served on the main thread (see `gui.rs`).
    editor_request: AtomicU8,
}

impl<'a> PluginShared<'a> for Shared<'a> {}

impl DefaultPluginFactory for TestSynth {
    fn get_descriptor() -> PluginDescriptor {
        PluginDescriptor::new(ID, NAME).with_features([INSTRUMENT, SYNTHESIZER, STEREO])
    }

    fn new_shared(host: HostSharedHandle<'_>) -> Result<Shared<'_>, PluginError> {
        Ok(Shared {
            host,
            gui: host.get_extension(),
            editor_request: AtomicU8::new(0),
        })
    }

    fn new_main_thread<'a>(
        _host: HostMainThreadHandle<'a>,
        shared: &'a Shared<'a>,
    ) -> Result<MainThread<'a>, PluginError> {
        Ok(MainThread {
            shared,
            window: RefCell::new(None),
        })
    }
}

pub struct MainThread<'a> {
    shared: &'a Shared<'a>,
    window: RefCell<Option<gui::window::Window>>,
}

impl<'a> PluginMainThread<'a, Shared<'a>> for MainThread<'a> {
    fn on_main_thread(&self) {
        gui::serve(self.shared);
    }
}

impl PluginAudioPortsImpl for MainThread<'_> {
    fn count(&self, is_input: bool) -> u32 {
        u32::from(!is_input)
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        if index == 0 && !is_input {
            writer.set(&AudioPortInfo {
                id: ClapId::new(0),
                name: b"Output",
                channel_count: 2,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: None,
            });
        }
    }
}

impl PluginNotePortsImpl for MainThread<'_> {
    fn count(&self, is_input: bool) -> u32 {
        if is_input { NOTE_PORTS as u32 } else { 0 }
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut NotePortInfoWriter) {
        if (index as usize) < NOTE_PORTS && is_input {
            writer.set(&NotePortInfo {
                id: ClapId::new(index),
                name: if index == 0 {
                    b"Notes"
                } else {
                    b"Inverted notes"
                },
                supported_dialects: NoteDialects::CLAP | NoteDialects::MIDI,
                preferred_dialect: Some(NoteDialect::Clap),
            });
        }
    }
}

enum Action {
    On {
        port: usize,
        key: u16,
        velocity: f64,
    },
    Off {
        port: usize,
        key: u16,
    },
    Volume(f64),
    Sustain(bool),
    Editor(u8),
}

#[derive(Clone, Copy)]
struct Note {
    key: u16,
    velocity: f64,
    /// Phase in cycles.
    phase: f64,
    /// Released while the sustain pedal was down.
    released: bool,
}

pub struct Voice<'a> {
    shared: &'a Shared<'a>,
    sample_rate: f64,
    volume: f64,
    sustain: bool,
    notes: [Option<Note>; NOTE_PORTS],
}

impl Voice<'_> {
    fn apply(&mut self, action: Action) {
        match action {
            Action::Editor(request) => {
                self.shared.editor_request.store(request, Ordering::Relaxed);
                self.shared.host.request_callback();
            }
            Action::On {
                port,
                key,
                velocity,
            } => {
                self.notes[port] = Some(Note {
                    key,
                    velocity,
                    phase: 0.0,
                    released: false,
                });
            }
            Action::Off { port, key } => {
                if let Some(note) = &mut self.notes[port]
                    && note.key == key
                {
                    if self.sustain {
                        note.released = true;
                    } else {
                        self.notes[port] = None;
                    }
                }
            }
            Action::Volume(volume) => self.volume = volume,
            Action::Sustain(down) => {
                self.sustain = down;
                if !down {
                    for note in &mut self.notes {
                        if note.is_some_and(|note| note.released) {
                            *note = None;
                        }
                    }
                }
            }
        }
    }

    fn next_sample(&mut self) -> f32 {
        let mut sample = 0.0;
        for (port, note) in self.notes.iter_mut().enumerate() {
            let Some(note) = note else {
                continue;
            };
            let voice = note.velocity * self.volume * (note.phase * TAU).cos();
            sample += if port == 0 { voice } else { -voice };
            let frequency = 440.0 * 2f64.powf((f64::from(note.key) - 69.0) / 12.0);
            note.phase = (note.phase + frequency / self.sample_rate).fract();
        }
        sample as f32
    }
}

fn action(event: &UnknownEvent) -> Option<Action> {
    let specific = |value: Match<u16>| match value {
        Match::Specific(value) => Some(value),
        Match::All => None,
    };
    let port = |port: u16| Some(usize::from(port)).filter(|port| *port < NOTE_PORTS);
    let editor = |key: u16| Some(Action::Editor(key as u8 + gui::HIDE));
    match event.as_core_event()? {
        CoreEventSpace::NoteOn(on) => {
            let key = specific(on.pckn().key)?;
            if key < 3 {
                return editor(key);
            }
            Some(Action::On {
                port: port(specific(on.pckn().port_index)?)?,
                key,
                velocity: on.velocity(),
            })
        }
        CoreEventSpace::NoteOff(off) => Some(Action::Off {
            port: port(specific(off.pckn().port_index)?)?,
            key: specific(off.pckn().key)?,
        }),
        CoreEventSpace::Midi(midi) => {
            let port = port(midi.port_index())?;
            match midi.data() {
                [status, key, velocity] if status & 0xF0 == 0x90 && velocity > 0 && key < 3 => {
                    editor(u16::from(key))
                }
                [status, key, velocity] if status & 0xF0 == 0x90 && velocity > 0 => {
                    Some(Action::On {
                        port,
                        key: u16::from(key),
                        velocity: f64::from(velocity) / 127.0,
                    })
                }
                [status, key, _] if status & 0xF0 == 0x80 || status & 0xF0 == 0x90 => {
                    Some(Action::Off {
                        port,
                        key: u16::from(key),
                    })
                }
                [status, VOLUME_CONTROLLER, value] if status & 0xF0 == 0xB0 => {
                    Some(Action::Volume(f64::from(value) / 127.0))
                }
                [status, SUSTAIN_CONTROLLER, value] if status & 0xF0 == 0xB0 => {
                    Some(Action::Sustain(value >= 64))
                }
                _ => None,
            }
        }
        // SAFETY: the host's buffer is valid during this process call.
        CoreEventSpace::MidiSysEx(sysex) => match unsafe { sysex.data() } {
            [0xF0, 0x7D, 0x01, value, 0xF7] if *value < 0x80 => {
                Some(Action::Volume(f64::from(*value) / 127.0))
            }
            _ => None,
        },
        _ => None,
    }
}

impl<'a> PluginAudioProcessor<'a, Shared<'a>, MainThread<'a>> for Voice<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &MainThread<'a>,
        shared: &'a Shared<'a>,
        config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        Ok(Voice {
            shared,
            sample_rate: config.sample_rate,
            volume: 1.0,
            sustain: false,
            notes: [None; NOTE_PORTS],
        })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        let frames = audio.frames_count() as usize;
        let mut rendered = vec![0.0f32; frames];
        let mut pending = events.input.iter().peekable();
        for (frame, sample) in rendered.iter_mut().enumerate() {
            while let Some(event) = pending.next_if(|e| e.header().time() as usize <= frame) {
                if let Some(action) = action(event) {
                    self.apply(action);
                }
            }
            *sample = self.next_sample();
        }
        for mut port in &mut audio {
            let Some(channels) = port.channels()?.into_f32() else {
                continue;
            };
            for pair in channels {
                if let ChannelPair::OutputOnly(output) = pair {
                    output.copy_from_slice(&rendered);
                }
            }
        }
        Ok(ProcessStatus::Continue)
    }

    fn reset(&mut self) {
        self.sustain = false;
        self.notes = [None; NOTE_PORTS];
    }
}

clack_export_entry!(SinglePluginEntry<TestSynth>);
