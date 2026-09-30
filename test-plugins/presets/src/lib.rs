//! Independent CLAP provider/load fixture; every fault is selected by explicit preset key/path.
mod errors;
mod provider;
use clack_extensions::audio_ports::*;
use clack_extensions::preset_discovery::prelude::*;
use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_plugin::entry::prelude::*;
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use std::ffi::CStr;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
pub const ID: &str = "com.studio.plughost.test-presets";
pub struct Fixture;
pub struct Shared(AtomicU32);
impl PluginShared<'_> for Shared {}
pub struct MainThread<'a> {
    shared: &'a Shared,
    host: HostMainThreadHandle<'a>,
}
impl<'a> PluginMainThread<'a, Shared> for MainThread<'a> {}
impl Plugin for Fixture {
    type Shared<'a> = Shared;
    type MainThread<'a> = MainThread<'a>;
    type AudioProcessor<'a> = Processor<'a>;
    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&Shared>) {
        builder
            .register::<PluginPresetLoad>()
            .register::<PluginAudioPorts>()
            .register::<PluginState>();
    }
}
impl DefaultPluginFactory for Fixture {
    fn get_descriptor() -> PluginDescriptor {
        PluginDescriptor::new(ID, "plughost test presets")
            .with_features([clack_plugin::plugin::features::INSTRUMENT])
    }
    fn new_shared(_host: HostSharedHandle<'_>) -> Result<Shared, PluginError> {
        Ok(Shared(AtomicU32::new(1.0f32.to_bits())))
    }
    fn new_main_thread<'a>(
        host: HostMainThreadHandle<'a>,
        shared: &'a Shared,
    ) -> Result<MainThread<'a>, PluginError> {
        Ok(MainThread { shared, host })
    }
}
impl PluginAudioPortsImpl for MainThread<'_> {
    fn count(&self, input: bool) -> u32 {
        u32::from(!input)
    }
    fn get(&self, index: u32, input: bool, writer: &mut AudioPortInfoWriter) {
        if index == 0 && !input {
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
impl PluginPresetLoadImpl for MainThread<'_> {
    fn load_from_location(
        &self,
        location: Location,
        key: Option<&CStr>,
    ) -> Result<(), PluginError> {
        let key = key.unwrap_or(c"half");
        if key == c"crash" {
            std::process::abort();
        }
        if key == c"hang" {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
        self.shared.0.store(0.5f32.to_bits(), Ordering::Relaxed);
        if key == c"fail" {
            return Err(PluginError::Message(errors::REFUSED));
        }
        let callback = self.host.get_extension::<HostPresetLoad>();
        if key == c"callback-error" {
            if let Some(callback) = callback {
                callback.on_error(&self.host, location, Some(key), 0, Some(errors::CALLBACK));
            }
            return Ok(());
        }
        if key == c"no-completion" {
            return Ok(());
        }
        if key != c"half" && key != c"quarter" {
            return Err(PluginError::Message(errors::UNKNOWN));
        }
        if key == c"quarter" {
            self.shared.0.store(0.25f32.to_bits(), Ordering::Relaxed);
        }
        if let Some(callback) = callback {
            callback.loaded(&self.host, location, Some(key));
        }
        Ok(())
    }
}
impl PluginStateImpl for MainThread<'_> {
    fn save(&self, output: &mut OutputStream) -> Result<(), PluginError> {
        output.write_all(&self.shared.0.load(Ordering::Relaxed).to_le_bytes())?;
        Ok(())
    }
    fn load(&self, input: &mut InputStream) -> Result<(), PluginError> {
        let mut bytes = [0; 4];
        input.read_exact(&mut bytes)?;
        self.shared
            .0
            .store(u32::from_le_bytes(bytes), Ordering::Relaxed);
        Ok(())
    }
}
pub struct Processor<'a>(&'a Shared);
impl<'a> PluginAudioProcessor<'a, Shared, MainThread<'a>> for Processor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main: &MainThread<'a>,
        shared: &'a Shared,
        _config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        Ok(Self(shared))
    }
    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        _events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        let gain = f32::from_bits(self.0.0.load(Ordering::Relaxed));
        for mut port in &mut audio {
            if let Some(channels) = port.channels()?.into_f32() {
                for pair in channels {
                    if let ChannelPair::OutputOnly(output) = pair {
                        output.fill(gain);
                    }
                }
            }
        }
        Ok(ProcessStatus::Continue)
    }
}
struct FixtureEntry {
    plugin: SinglePluginEntry<Fixture>,
    presets: PresetDiscoveryFactoryWrapper<provider::Factory>,
}
impl Entry for FixtureEntry {
    fn new(path: Option<&CStr>) -> Result<Self, EntryLoadError> {
        Ok(Self {
            plugin: SinglePluginEntry::new(path)?,
            presets: PresetDiscoveryFactoryWrapper::new(provider::Factory::new()),
        })
    }
    fn declare_factories<'a>(&'a self, builder: &mut EntryFactories<'a>) {
        self.plugin.declare_factories(builder);
        builder.register_factory(&self.presets);
    }
}
clack_export_entry!(FixtureEntry);
