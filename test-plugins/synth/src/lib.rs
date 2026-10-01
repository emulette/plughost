//! CLAP and VST3 test instrument whose output shows exactly when notes, their tuning and pressure,
//! and MIDI arrive (see `voices.rs`). Each note is a voice, told apart from other notes on its key
//! by its note ID; a voice stops at the sample of its note off, or, when the sustain pedal (control
//! change 64) is down, at the sample the pedal is released. Every output channel is the sum of the
//! first note port's voices minus the second's. MIDI control change 7 and the system exclusive
//! message `F0 7D 01 vv F7` set the volume to `vv / 127`. Both CLAP note ports take CLAP and MIDI
//! note events, with tuning and pressure note expressions. Note ons for keys 0 to 2 drive the CLAP
//! editor (see `gui.rs`) instead of sounding. The VST3 side (`vst.rs`) plays notes, tuning and
//! poly pressure on two event buses.

mod errors;
mod gui;
mod voices;
mod vst;

use voices::{Expression, PORTS, Target, Voices};

use std::cell::RefCell;
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
use clack_plugin::events::Pckn;
use clack_plugin::events::event_types::NoteExpressionType;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::plugin::features::{INSTRUMENT, STEREO, SYNTHESIZER};
use clack_plugin::prelude::*;

const ID: &str = "com.studio.plughost.test-synth";
const NAME: &str = "plughost test synth";
const VOLUME_CONTROLLER: u8 = 7;
const SUSTAIN_CONTROLLER: u8 = 64;

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
        if is_input { PORTS as u32 } else { 0 }
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut NotePortInfoWriter) {
        if (index as usize) < PORTS && is_input {
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
    On(Target, f64),
    Off(Target),
    Expression(Target, Expression),
    Volume(f64),
    Sustain(bool),
    Editor(u8),
}

pub struct Voice<'a> {
    shared: &'a Shared<'a>,
    voices: Voices,
}

impl Voice<'_> {
    fn apply(&mut self, action: Action) {
        match action {
            Action::Editor(request) => {
                self.shared.editor_request.store(request, Ordering::Relaxed);
                self.shared.host.request_callback();
            }
            Action::On(target, velocity) => self.voices.note_on(target, velocity),
            Action::Off(target) => self.voices.note_off(target),
            Action::Expression(target, expression) => self.voices.expression(target, &expression),
            Action::Volume(volume) => self.voices.set_volume(volume),
            Action::Sustain(down) => self.voices.set_sustain(down),
        }
    }
}

/// The voices a note event addresses, on one of the synth's ports.
fn target(pckn: Pckn) -> Option<Target> {
    let port = usize::from(*pckn.port_index.as_specific()?);
    (port < PORTS).then_some(Target {
        port,
        channel: pckn.channel.into_specific(),
        key: pckn.key.into_specific(),
        id: pckn.note_id.into_specific(),
    })
}

/// The voices a MIDI note message on `port` addresses.
fn midi_target(port: usize, status: u8, key: u8) -> Target {
    Target {
        port,
        channel: Some(u16::from(status & 0x0F)),
        key: Some(u16::from(key)),
        id: None,
    }
}

fn action(event: &UnknownEvent) -> Option<Action> {
    let editor = |key: u16| Some(Action::Editor(key as u8 + gui::HIDE));
    match event.as_core_event()? {
        CoreEventSpace::NoteOn(on) => {
            let target = target(on.pckn())?;
            if target.key.is_some_and(|key| key < 3) {
                return editor(target.key?);
            }
            Some(Action::On(target, on.velocity()))
        }
        CoreEventSpace::NoteOff(off) => Some(Action::Off(target(off.pckn())?)),
        CoreEventSpace::NoteExpression(expression) => {
            let target = target(expression.pckn())?;
            match expression.expression_type()? {
                NoteExpressionType::Tuning => Some(Action::Expression(
                    target,
                    Expression::Tuning(expression.value()),
                )),
                NoteExpressionType::Pressure => Some(Action::Expression(
                    target,
                    Expression::Pressure(expression.value()),
                )),
                _ => None,
            }
        }
        CoreEventSpace::Midi(midi) => {
            let port = Some(usize::from(midi.port_index())).filter(|port| *port < PORTS)?;
            match midi.data() {
                [status, key, velocity] if status & 0xF0 == 0x90 && velocity > 0 && key < 3 => {
                    editor(u16::from(key))
                }
                [status, key, velocity] if status & 0xF0 == 0x90 && velocity > 0 => Some(
                    Action::On(midi_target(port, status, key), f64::from(velocity) / 127.0),
                ),
                [status, key, _] if status & 0xF0 == 0x80 || status & 0xF0 == 0x90 => {
                    Some(Action::Off(midi_target(port, status, key)))
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
            voices: Voices::new(config.sample_rate),
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
            *sample = self.voices.next_sample();
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
        self.voices.reset();
    }
}

clack_export_entry!(SinglePluginEntry<TestSynth>);
