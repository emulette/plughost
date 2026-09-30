use clack_extensions::audio_ports::{AudioPortFlags, AudioPortInfoBuffer};
use clack_extensions::gui::{GuiApiType, GuiConfiguration};
use clack_extensions::note_ports::{NoteDialect, NotePortInfoBuffer};
use clack_extensions::params::{ParamInfoBuffer, ParamInfoFlags};
use plughost_core::{Capabilities, Support};

use super::Plugin;

impl Plugin {
    /// Queries advertised extensions and the current ports/parameters, without opening a GUI.
    pub(crate) fn capabilities(&mut self) -> Capabilities {
        let extensions = self.shared().extensions();
        let (audio, notes, params, gui, state) = (
            extensions.audio_ports,
            extensions.note_ports,
            extensions.params,
            extensions.gui,
            extensions.state,
        );
        let handle = self.instance.plugin_handle();
        let embedded_editor = gui
            .is_some_and(|gui| {
                GuiApiType::default_for_current_platform().is_some_and(|api_type| {
                    gui.is_api_supported(
                        &handle,
                        GuiConfiguration {
                            api_type,
                            is_floating: false,
                        },
                    )
                })
            })
            .into();
        let note_support = |is_input| {
            let Some(notes) = notes else {
                return Support::Unsupported;
            };
            let mut buffer = NotePortInfoBuffer::new();
            let mut support = Support::Unsupported;
            for index in 0..notes.count(&handle, is_input) {
                match notes.get(&handle, index, is_input, &mut buffer) {
                    Some(port)
                        if [NoteDialect::Clap, NoteDialect::Midi]
                            .iter()
                            .any(|&dialect| port.supported_dialects.contains(dialect.into())) =>
                    {
                        return Support::Supported;
                    }
                    Some(_) => {}
                    None => support = Support::Unknown,
                }
            }
            support
        };
        let mut automation = Support::Unsupported;
        if let Some(params) = params {
            let mut buffer = ParamInfoBuffer::new();
            for index in 0..params.count(&handle) {
                match params.get_info(&handle, index, &mut buffer) {
                    Some(info) if info.flags.contains(ParamInfoFlags::IS_AUTOMATABLE) => {
                        automation = Support::Supported;
                        break;
                    }
                    Some(_) => {}
                    None => automation = Support::Unknown,
                }
            }
        }
        let (mut f32, mut f64) = (Support::Unsupported, Support::Unsupported);
        if let Some(audio) = audio {
            let mut buffer = AudioPortInfoBuffer::new();
            let mut seen = false;
            let mut missing = false;
            let mut all_f64 = true;
            for is_input in [true, false] {
                for index in 0..audio.count(&handle, is_input) {
                    seen = true;
                    match audio.get(&handle, index, is_input, &mut buffer) {
                        Some(port) => {
                            all_f64 &= port.flags.contains(AudioPortFlags::SUPPORTS_64BITS)
                        }
                        None => missing = true,
                    }
                }
            }
            if seen {
                // CLAP requires 32-bit support on audio ports; 64-bit is explicitly advertised.
                f32 = if missing {
                    Support::Unknown
                } else {
                    Support::Supported
                };
                f64 = if !all_f64 {
                    Support::Unsupported
                } else if missing {
                    Support::Unknown
                } else {
                    Support::Supported
                };
            }
        }
        Capabilities {
            embedded_editor,
            bus_discovery: audio.is_some().into(),
            note_input: note_support(true),
            note_output: note_support(false),
            sample_accurate_automation: automation,
            state: state.is_some().into(),
            // Discovery uses a separate factory; the current instance query does not inspect it.
            factory_presets: Support::Unknown,
            f32,
            f64,
        }
    }
}
