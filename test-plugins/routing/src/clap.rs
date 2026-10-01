use clack_extensions::ambisonic::*;
use clack_extensions::audio_ports::*;
use clack_extensions::audio_ports_activation::*;
use clack_extensions::audio_ports_config::*;
use clack_extensions::configurable_audio_ports::*;
use clack_extensions::note_ports::PluginNotePorts;
use clack_extensions::params::*;
use clack_extensions::state::*;
use clack_extensions::surround::*;
use clack_plugin::entry::prelude::*;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use clack_plugin::utils::Cookie;
use std::ffi::CStr;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::sync::{Mutex, MutexGuard};
#[path = "clap_audio.rs"]
mod audio;
#[path = "clap_events.rs"]
mod events;
#[path = "clap_ports.rs"]
mod ports;
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
pub struct Fixture;
/// A main port layout the host applied through configurable ports.
#[derive(Clone, PartialEq)]
enum Main {
    Surround(Vec<SurroundChannel>),
    /// Channels of ACN-ordered, SN3D-normalized ambisonics.
    Ambisonic(u32),
}
pub struct Shared {
    values: Mutex<[f64; 2]>,
    configuration: Mutex<u32>,
    /// Replaces the selected configuration's main ports until another is selected.
    main: Mutex<Option<Main>>,
    active: Mutex<[[bool; 2]; 2]>,
}
impl Shared {
    fn apply(&self, input: &InputEvents) {
        for event in input {
            if let Some(CoreEventSpace::ParamValue(value)) = event.as_core_event()
                && let Some(id) = value.param_id()
                && id.get() < 2
            {
                lock(&self.values)[id.get() as usize] = value.value();
            }
        }
    }
}
impl PluginShared<'_> for Shared {}
pub struct MainThread<'a> {
    shared: &'a Shared,
}
impl<'a> PluginMainThread<'a, Shared> for MainThread<'a> {}
impl Plugin for Fixture {
    type Shared<'a> = Shared;
    type MainThread<'a> = MainThread<'a>;
    type AudioProcessor<'a> = audio::Processor<'a>;
    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&Shared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginAudioPortsConfig>()
            .register::<PluginAudioPortsConfigInfo>()
            .register::<PluginAudioPortsActivation>()
            .register::<PluginNotePorts>()
            .register::<PluginSurround>()
            .register::<PluginAmbisonic>()
            .register::<PluginConfigurableAudioPorts>()
            .register::<PluginParams>()
            .register::<PluginState>();
    }
}
impl DefaultPluginFactory for Fixture {
    fn get_descriptor() -> PluginDescriptor {
        PluginDescriptor::new("com.studio.plughost.test-routing", "plughost test routing")
    }
    fn new_shared(_host: HostSharedHandle<'_>) -> Result<Shared, PluginError> {
        Ok(Shared {
            values: Mutex::new([1.0, 0.0]),
            configuration: Mutex::new(101),
            main: Mutex::new(None),
            active: Mutex::new([[true; 2]; 2]),
        })
    }
    fn new_main_thread<'a>(
        _host: HostMainThreadHandle<'a>,
        shared: &'a Shared,
    ) -> Result<MainThread<'a>, PluginError> {
        Ok(MainThread { shared })
    }
}
impl PluginMainThreadParams for MainThread<'_> {
    fn count(&self) -> u32 {
        2
    }
    fn get_info(&self, index: u32, writer: &mut ParamInfoWriter) {
        if index < 2 {
            writer.set(&ParamInfo {
                id: ClapId::new(index),
                flags: ParamInfoFlags::IS_AUTOMATABLE
                    | if index == 1 {
                        ParamInfoFlags::IS_BYPASS | ParamInfoFlags::IS_STEPPED
                    } else {
                        ParamInfoFlags::empty()
                    },
                cookie: Cookie::empty(),
                name: if index == 0 { b"Gain" } else { b"Bypass" },
                module: b"",
                min_value: 0.0,
                max_value: 1.0,
                default_value: if index == 0 { 1.0 } else { 0.0 },
            });
        }
    }
    fn get_value(&self, id: ClapId) -> Option<f64> {
        lock(&self.shared.values).get(id.get() as usize).copied()
    }
    fn value_to_text(
        &self,
        _id: ClapId,
        value: f64,
        writer: &mut ParamDisplayWriter,
    ) -> std::fmt::Result {
        write!(writer, "{value}")
    }
    fn text_to_value(&self, _id: ClapId, text: &CStr) -> Option<f64> {
        text.to_str().ok()?.parse().ok()
    }
    fn flush(&self, input: &InputEvents, _output: &mut OutputEvents) {
        self.shared.apply(input);
    }
}
impl PluginStateImpl for MainThread<'_> {
    fn save(&self, output: &mut OutputStream) -> Result<(), PluginError> {
        for value in *lock(&self.shared.values) {
            output.write_all(&value.to_le_bytes())?;
        }
        Ok(())
    }
    fn load(&self, input: &mut InputStream) -> Result<(), PluginError> {
        let mut values = [0.0; 2];
        for value in &mut values {
            let mut bytes = [0; 8];
            input.read_exact(&mut bytes)?;
            *value = f64::from_le_bytes(bytes);
        }
        *lock(&self.shared.values) = values;
        Ok(())
    }
}
clack_export_entry!(SinglePluginEntry<Fixture>);
