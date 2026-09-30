//! The operations every format provides, so the helper can host a chain of mixed formats. Audio
//! retains the explicitly selected sample precision across this interface.

use crate::errors::Error;
use std::ffi::c_void;

use plughost_core::Capabilities;
use plughost_core::PluginRef;
use plughost_core::render::Tail;
use plughost_core::{
    HostIdentity, MidiEvent, ParameterInfo, PluginInfo, PluginState, ProcessConfig,
};

#[cfg(all(feature = "au", target_os = "macos"))]
use crate::au;
#[cfg(feature = "clap")]
use crate::clap;
#[cfg(feature = "vst3")]
use crate::vst3;

/// Called when a plugin asks for a new editor size; returns whether the host window now has that
/// size.
pub type ResizeRequest = Box<dyn Fn(u32, u32) -> bool>;

/// Where an opened editor draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorView {
    /// In the host's parent view, at this size.
    Embedded { width: u32, height: u32 },
    /// In a window of the plugin's own; the host's parent view stays unused.
    Floating,
}

/// What a plugin asked of its editor window. A later request replaces one not yet served.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorRequest {
    /// Open the editor, or bring it to the front when it is open.
    Open,
    /// Show the open editor again.
    Show,
    /// Hide the open editor, keeping it open.
    Hide,
    /// The editor closed its own window; the host detaches it.
    Closed,
}

/// A plugin instance, used on the thread that created it.
pub trait HostedPlugin {
    /// Drains plugin-authored log messages; formats without a native log callback return empty.
    fn take_diagnostics(&self) -> plughost_core::DiagnosticBatch {
        plughost_core::DiagnosticBatch::default()
    }
    fn take_parameter_events(&mut self) -> plughost_core::ParameterEventBatch;
    fn factory_presets(&mut self) -> Result<Vec<plughost_core::FactoryPreset>, Error> {
        Err(Error::FactoryPresetsUnsupported)
    }
    fn select_factory_preset(
        &mut self,
        _preset: &plughost_core::FactoryPresetId,
    ) -> Result<(), Error> {
        Err(Error::FactoryPresetsUnsupported)
    }
    fn load_discovered_preset(
        &mut self,
        _location: &plughost_core::PresetLocation,
        _load_key: Option<&str>,
    ) -> Result<(), Error> {
        Err(Error::PresetDiscoveryUnsupported)
    }
    fn audio_buses(&mut self) -> Result<Vec<plughost_core::AudioBusInfo>, Error>;
    /// Native event ports in both directions, inputs first.
    fn event_ports(&mut self) -> Result<Vec<plughost_core::EventPortInfo>, Error>;
    fn audio_configurations(&mut self) -> Result<Vec<plughost_core::AudioConfiguration>, Error>;
    fn prepare_audio(
        &mut self,
        config: &plughost_core::AudioConfig,
    ) -> Result<Vec<plughost_core::AudioBusInfo>, Error>;
    fn bypass(&mut self) -> Result<plughost_core::BypassState, Error>;
    fn set_bypass(&mut self, enabled: bool) -> Result<(), Error>;
    fn info(&self) -> &PluginInfo;
    fn capabilities(&mut self) -> Result<Capabilities, Error>;
    fn prepare(&mut self, config: &ProcessConfig) -> Result<(), Error>;
    /// A handle that processes this plugin from another thread.
    fn processor(&self) -> Box<dyn BlockProcessor>;
    fn latency(&self) -> Result<u32, Error>;
    fn tail(&self) -> Result<Tail, Error>;
    /// Each parameter with its current normalized value.
    fn parameters(&mut self) -> Vec<(ParameterInfo, f64)>;
    fn parameter_details(&mut self, id: u64) -> Result<plughost_core::ParameterDetails, Error>;
    /// Reads a bounded page of native labels and values; does not edit the parameter.
    fn parameter_choices(
        &mut self,
        id: u64,
        start: u64,
        count: u32,
    ) -> Result<plughost_core::ParameterChoicePage, Error> {
        crate::parameter_choices::read(self, id, start, count)
    }
    /// Changes a parameter the way an edit in the plugin's editor does.
    fn set_parameter(&mut self, id: u64, value: f64) -> Result<(), Error>;
    fn parameter_text(&mut self, id: u64, value: f64) -> Result<String, Error>;
    fn parameter_from_text(&mut self, id: u64, text: &str) -> Result<f64, Error>;
    fn parameter_to_plain(&mut self, id: u64, value: f64) -> Result<f64, Error>;
    fn parameter_to_normalized(&mut self, id: u64, plain: f64) -> Result<f64, Error>;
    /// Saves the state, after pending edits reach the processor.
    fn save_state(&mut self, purpose: plughost_core::StatePurpose) -> Result<PluginState, Error>;
    fn restore_state(
        &mut self,
        state: &PluginState,
        purpose: plughost_core::StatePurpose,
    ) -> Result<(), Error>;
    /// Clears internal audio state (tails, delay lines), keeping settings.
    fn reset(&mut self) -> Result<(), Error>;
    /// Opens the editor in `parent` (`NSView` on macOS, `HWND` on Windows), or in its own window
    /// when the plugin draws only there.
    ///
    /// # Safety
    ///
    /// `parent` must be a valid native view that outlives the editor.
    unsafe fn open_editor(
        &mut self,
        parent: *mut c_void,
        resize: ResizeRequest,
    ) -> Result<EditorView, Error>;
    fn close_editor(&mut self);
    /// Tells the open editor that its window was shown or hidden.
    fn set_editor_visible(&mut self, _visible: bool) {}
    /// The plugin's latest editor request since the last call.
    fn take_editor_request(&mut self) -> Option<EditorRequest> {
        None
    }
    fn editor_can_resize(&mut self) -> bool {
        false
    }
    /// Offers the size the user dragged the editor window to; returns the size the editor took,
    /// or `None` when the editor keeps its own size.
    fn resize_editor(&mut self, _width: u32, _height: u32) -> Option<(u32, u32)> {
        None
    }
    /// Sends a Windows monitor scale to an editor that supports explicit scaling, returning
    /// its resulting size in physical pixels. Other editors keep their own size.
    #[cfg(target_os = "windows")]
    fn set_editor_scale(&mut self, scale: f64) -> Option<(u32, u32)>;
    /// Serves requests the plugin made of the owning thread (callbacks, timers). Call it
    /// regularly from that thread's event loop.
    fn idle(&mut self) {}
}

/// Processes a plugin from a processing thread.
pub trait BlockProcessor: Send {
    /// Processes one block in the prepared precision. `events` are in offset order, inside the
    /// block, on active native event input ports by index. `produced` is replaced with the
    /// plugin's output events in offset order, on native event output ports by index; give it
    /// capacity for `MAX_BLOCK_EVENTS` to process without allocating for them. A plugin that
    /// produces more events or system exclusive bytes than one block allows fails the block,
    /// which the caller resolves with a reset.
    fn process_audio_f32(
        &self,
        context: &plughost_core::BlockContext,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        automation: &[plughost_core::ParameterChange],
        events: &[MidiEvent],
        produced: &mut Vec<MidiEvent>,
    ) -> Result<(), Error>;
    fn process_audio_f64(
        &self,
        context: &plughost_core::BlockContext,
        input: &[&[f64]],
        output: &mut [&mut [f64]],
        automation: &[plughost_core::ParameterChange],
        events: &[MidiEvent],
        produced: &mut Vec<MidiEvent>,
    ) -> Result<(), Error>;
    fn latency(&self) -> Result<u32, Error>;
    fn tail(&self) -> Result<Tail, Error>;
    fn restart_required(&self) -> bool {
        false
    }
}

/// Loads the plugin class `plugin` names, in this process.
pub fn load(plugin: &PluginRef, host: &HostIdentity) -> Result<Box<dyn HostedPlugin>, Error> {
    host.validate().map_err(Error::Input)?;
    match plugin.format {
        #[cfg(feature = "vst3")]
        plughost_core::PluginFormat::Vst3 => {
            let bundle = plugin.bundle.as_deref().ok_or(Error::NoBundle)?;
            let module = vst3::Module::load_with_host(bundle, host)?;
            Ok(Box::new(vst3::Plugin::new(
                &module,
                &plugin.class_id,
                host,
            )?))
        }
        #[cfg(all(feature = "au", target_os = "macos"))]
        plughost_core::PluginFormat::AudioUnit => Ok(Box::new(au::Plugin::new(&plugin.class_id)?)),
        #[cfg(feature = "clap")]
        plughost_core::PluginFormat::Clap => {
            let bundle = plugin.bundle.as_deref().ok_or(Error::NoBundle)?;
            Ok(Box::new(clap::Plugin::new(bundle, &plugin.class_id, host)?))
        }
        #[allow(unreachable_patterns)]
        format => Err(Error::UnsupportedFormat(format)),
    }
}

#[cfg(feature = "vst3")]
impl HostedPlugin for vst3::Plugin {
    fn take_diagnostics(&self) -> plughost_core::DiagnosticBatch {
        vst3::Plugin::take_diagnostics(self)
    }
    fn audio_buses(&mut self) -> Result<Vec<plughost_core::AudioBusInfo>, Error> {
        Ok(vst3::Plugin::audio_buses(self)?)
    }
    fn event_ports(&mut self) -> Result<Vec<plughost_core::EventPortInfo>, Error> {
        Ok(vst3::Plugin::event_ports(self)?)
    }
    fn audio_configurations(&mut self) -> Result<Vec<plughost_core::AudioConfiguration>, Error> {
        Ok(vst3::Plugin::audio_configurations(self)?)
    }
    fn prepare_audio(
        &mut self,
        config: &plughost_core::AudioConfig,
    ) -> Result<Vec<plughost_core::AudioBusInfo>, Error> {
        Ok(vst3::Plugin::prepare_audio(self, config)?)
    }
    fn bypass(&mut self) -> Result<plughost_core::BypassState, Error> {
        Ok(vst3::Plugin::bypass(self)?)
    }
    fn set_bypass(&mut self, enabled: bool) -> Result<(), Error> {
        Ok(vst3::Plugin::set_bypass(self, enabled)?)
    }

    fn take_parameter_events(&mut self) -> plughost_core::ParameterEventBatch {
        vst3::Plugin::take_parameter_events(self)
    }
    fn factory_presets(&mut self) -> Result<Vec<plughost_core::FactoryPreset>, Error> {
        Ok(vst3::Plugin::factory_presets(self)?)
    }
    fn select_factory_preset(
        &mut self,
        preset: &plughost_core::FactoryPresetId,
    ) -> Result<(), Error> {
        Ok(vst3::Plugin::select_factory_preset(self, preset)?)
    }

    fn capabilities(&mut self) -> Result<Capabilities, Error> {
        Ok(vst3::Plugin::capabilities(self)?)
    }
    fn info(&self) -> &PluginInfo {
        vst3::Plugin::info(self)
    }

    fn prepare(&mut self, config: &ProcessConfig) -> Result<(), Error> {
        Ok(vst3::Plugin::prepare(self, config)?)
    }

    fn processor(&self) -> Box<dyn BlockProcessor> {
        Box::new(vst3::Plugin::processor(self))
    }

    fn latency(&self) -> Result<u32, Error> {
        Ok(vst3::Plugin::latency(self)?)
    }

    fn tail(&self) -> Result<Tail, Error> {
        Ok(vst3::Plugin::tail(self)?)
    }

    fn parameters(&mut self) -> Vec<(ParameterInfo, f64)> {
        vst3::Plugin::parameters(self)
            .into_iter()
            .map(|info| {
                let value = self.parameter_value(info.id);
                (info, value)
            })
            .collect()
    }

    fn parameter_details(&mut self, id: u64) -> Result<plughost_core::ParameterDetails, Error> {
        Ok(vst3::Plugin::parameter_details(self, id)?)
    }

    fn set_parameter(&mut self, id: u64, value: f64) -> Result<(), Error> {
        vst3::Plugin::set_parameter(self, id, value).map_err(Error::from)
    }

    fn parameter_text(&mut self, id: u64, value: f64) -> Result<String, Error> {
        Ok(vst3::Plugin::parameter_text(self, id, value)?)
    }

    fn parameter_from_text(&mut self, id: u64, text: &str) -> Result<f64, Error> {
        Ok(vst3::Plugin::parameter_from_text(self, id, text)?)
    }

    fn parameter_to_plain(&mut self, id: u64, value: f64) -> Result<f64, Error> {
        Ok(vst3::Plugin::parameter_to_plain(self, id, value)?)
    }

    fn parameter_to_normalized(&mut self, id: u64, plain: f64) -> Result<f64, Error> {
        Ok(vst3::Plugin::parameter_to_normalized(self, id, plain)?)
    }

    fn save_state(&mut self, purpose: plughost_core::StatePurpose) -> Result<PluginState, Error> {
        Ok(vst3::Plugin::save_state(self, purpose)?)
    }
    fn restore_state(
        &mut self,
        state: &PluginState,
        purpose: plughost_core::StatePurpose,
    ) -> Result<(), Error> {
        Ok(vst3::Plugin::restore_state(self, state, purpose)?)
    }

    fn reset(&mut self) -> Result<(), Error> {
        Ok(vst3::Plugin::reset(self)?)
    }

    unsafe fn open_editor(
        &mut self,
        parent: *mut c_void,
        resize: ResizeRequest,
    ) -> Result<EditorView, Error> {
        let (width, height) = unsafe { vst3::Plugin::open_editor(self, parent, resize)? };
        Ok(EditorView::Embedded { width, height })
    }

    fn close_editor(&mut self) {
        vst3::Plugin::close_editor(self);
    }

    fn take_editor_request(&mut self) -> Option<EditorRequest> {
        vst3::Plugin::take_editor_request(self)
    }

    fn editor_can_resize(&mut self) -> bool {
        vst3::Plugin::editor_can_resize(self)
    }

    fn resize_editor(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        vst3::Plugin::resize_editor(self, width, height)
    }

    #[cfg(target_os = "windows")]
    fn set_editor_scale(&mut self, scale: f64) -> Option<(u32, u32)> {
        vst3::Plugin::set_editor_scale(self, scale)
    }

    fn idle(&mut self) {
        vst3::Plugin::idle(self);
    }
}

#[cfg(feature = "vst3")]
impl BlockProcessor for vst3::Processor {
    fn process_audio_f32(
        &self,
        context: &plughost_core::BlockContext,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        automation: &[plughost_core::ParameterChange],
        events: &[MidiEvent],
        produced: &mut Vec<MidiEvent>,
    ) -> Result<(), Error> {
        Ok(vst3::Processor::process(
            self, context, input, output, automation, events, produced,
        )?)
    }

    fn process_audio_f64(
        &self,
        context: &plughost_core::BlockContext,
        input: &[&[f64]],
        output: &mut [&mut [f64]],
        automation: &[plughost_core::ParameterChange],
        events: &[MidiEvent],
        produced: &mut Vec<MidiEvent>,
    ) -> Result<(), Error> {
        Ok(vst3::Processor::process(
            self, context, input, output, automation, events, produced,
        )?)
    }

    fn latency(&self) -> Result<u32, Error> {
        Ok(vst3::Processor::latency(self)?)
    }
    fn tail(&self) -> Result<Tail, Error> {
        Ok(vst3::Processor::tail(self)?)
    }
    fn restart_required(&self) -> bool {
        vst3::Processor::restart_required(self)
    }
}

#[cfg(all(feature = "au", target_os = "macos"))]
impl HostedPlugin for au::Plugin {
    fn take_diagnostics(&self) -> plughost_core::DiagnosticBatch {
        au::Plugin::take_diagnostics(self)
    }
    fn audio_buses(&mut self) -> Result<Vec<plughost_core::AudioBusInfo>, Error> {
        Ok(au::Plugin::audio_buses(self)?)
    }
    fn event_ports(&mut self) -> Result<Vec<plughost_core::EventPortInfo>, Error> {
        Ok(au::Plugin::event_ports(self)?)
    }
    fn audio_configurations(&mut self) -> Result<Vec<plughost_core::AudioConfiguration>, Error> {
        Ok(au::Plugin::audio_configurations(self)?)
    }
    fn prepare_audio(
        &mut self,
        config: &plughost_core::AudioConfig,
    ) -> Result<Vec<plughost_core::AudioBusInfo>, Error> {
        Ok(au::Plugin::prepare_audio(self, config)?)
    }
    fn bypass(&mut self) -> Result<plughost_core::BypassState, Error> {
        Ok(au::Plugin::bypass(self)?)
    }
    fn set_bypass(&mut self, enabled: bool) -> Result<(), Error> {
        Ok(au::Plugin::set_bypass(self, enabled)?)
    }

    fn take_parameter_events(&mut self) -> plughost_core::ParameterEventBatch {
        au::Plugin::take_parameter_events(self)
    }
    fn factory_presets(&mut self) -> Result<Vec<plughost_core::FactoryPreset>, Error> {
        Ok(au::Plugin::factory_presets(self)?)
    }
    fn select_factory_preset(
        &mut self,
        preset: &plughost_core::FactoryPresetId,
    ) -> Result<(), Error> {
        Ok(au::Plugin::select_factory_preset(self, preset)?)
    }

    fn capabilities(&mut self) -> Result<Capabilities, Error> {
        Ok(au::Plugin::capabilities(self)?)
    }
    fn info(&self) -> &PluginInfo {
        au::Plugin::info(self)
    }

    fn prepare(&mut self, config: &ProcessConfig) -> Result<(), Error> {
        Ok(au::Plugin::prepare(self, config)?)
    }

    fn processor(&self) -> Box<dyn BlockProcessor> {
        Box::new(au::Plugin::processor(self))
    }

    fn latency(&self) -> Result<u32, Error> {
        Ok(au::Plugin::latency(self)?)
    }

    fn tail(&self) -> Result<Tail, Error> {
        Ok(au::Plugin::tail(self)?)
    }

    fn parameters(&mut self) -> Vec<(ParameterInfo, f64)> {
        au::Plugin::parameters(self)
    }

    fn parameter_details(&mut self, id: u64) -> Result<plughost_core::ParameterDetails, Error> {
        Ok(au::Plugin::parameter_details(self, id)?)
    }

    fn set_parameter(&mut self, id: u64, value: f64) -> Result<(), Error> {
        au::Plugin::set_parameter(self, id, value).map_err(Error::from)
    }

    fn parameter_text(&mut self, id: u64, value: f64) -> Result<String, Error> {
        Ok(au::Plugin::parameter_text(self, id, value)?)
    }

    fn parameter_from_text(&mut self, id: u64, text: &str) -> Result<f64, Error> {
        Ok(au::Plugin::parameter_from_text(self, id, text)?)
    }

    fn parameter_to_plain(&mut self, id: u64, value: f64) -> Result<f64, Error> {
        Ok(au::Plugin::parameter_to_plain(self, id, value)?)
    }

    fn parameter_to_normalized(&mut self, id: u64, plain: f64) -> Result<f64, Error> {
        Ok(au::Plugin::parameter_to_normalized(self, id, plain)?)
    }

    fn save_state(&mut self, purpose: plughost_core::StatePurpose) -> Result<PluginState, Error> {
        Ok(au::Plugin::save_state(self, purpose)?)
    }
    fn restore_state(
        &mut self,
        state: &PluginState,
        purpose: plughost_core::StatePurpose,
    ) -> Result<(), Error> {
        Ok(au::Plugin::restore_state(self, state, purpose)?)
    }

    fn reset(&mut self) -> Result<(), Error> {
        Ok(au::Plugin::reset(self)?)
    }

    unsafe fn open_editor(
        &mut self,
        parent: *mut c_void,
        _resize: ResizeRequest,
    ) -> Result<EditorView, Error> {
        let (width, height) = unsafe { au::Plugin::open_editor(self, parent)? };
        Ok(EditorView::Embedded { width, height })
    }

    fn close_editor(&mut self) {
        au::Plugin::close_editor(self);
    }
}

#[cfg(all(feature = "au", target_os = "macos"))]
impl BlockProcessor for au::Processor {
    fn restart_required(&self) -> bool {
        au::Processor::restart_required(self)
    }
    fn process_audio_f32(
        &self,
        context: &plughost_core::BlockContext,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        automation: &[plughost_core::ParameterChange],
        events: &[MidiEvent],
        produced: &mut Vec<MidiEvent>,
    ) -> Result<(), Error> {
        Ok(au::Processor::process(
            self, context, input, output, automation, events, produced,
        )?)
    }

    fn process_audio_f64(
        &self,
        context: &plughost_core::BlockContext,
        input: &[&[f64]],
        output: &mut [&mut [f64]],
        automation: &[plughost_core::ParameterChange],
        events: &[MidiEvent],
        produced: &mut Vec<MidiEvent>,
    ) -> Result<(), Error> {
        let _ = (context, input, output, automation, events, produced);
        Err(Error::Au(au::AuError::SampleFormatUnsupported(
            plughost_core::SampleFormat::F64,
        )))
    }

    fn latency(&self) -> Result<u32, Error> {
        Ok(au::Processor::latency(self)?)
    }
    fn tail(&self) -> Result<Tail, Error> {
        Ok(au::Processor::tail(self)?)
    }
}

#[cfg(feature = "clap")]
impl HostedPlugin for clap::Plugin {
    fn audio_buses(&mut self) -> Result<Vec<plughost_core::AudioBusInfo>, Error> {
        Ok(clap::Plugin::audio_buses(self)?)
    }
    fn event_ports(&mut self) -> Result<Vec<plughost_core::EventPortInfo>, Error> {
        Ok(clap::Plugin::event_ports(self)?)
    }
    fn audio_configurations(&mut self) -> Result<Vec<plughost_core::AudioConfiguration>, Error> {
        Ok(clap::Plugin::audio_configurations(self)?)
    }
    fn prepare_audio(
        &mut self,
        config: &plughost_core::AudioConfig,
    ) -> Result<Vec<plughost_core::AudioBusInfo>, Error> {
        Ok(clap::Plugin::prepare_audio(self, config)?)
    }
    fn bypass(&mut self) -> Result<plughost_core::BypassState, Error> {
        Ok(clap::Plugin::bypass(self)?)
    }
    fn set_bypass(&mut self, enabled: bool) -> Result<(), Error> {
        Ok(clap::Plugin::set_bypass(self, enabled)?)
    }

    fn take_parameter_events(&mut self) -> plughost_core::ParameterEventBatch {
        clap::Plugin::take_parameter_events(self)
    }
    fn load_discovered_preset(
        &mut self,
        location: &plughost_core::PresetLocation,
        load_key: Option<&str>,
    ) -> Result<(), Error> {
        Ok(clap::Plugin::load_discovered_preset(
            self, location, load_key,
        )?)
    }

    fn take_diagnostics(&self) -> plughost_core::DiagnosticBatch {
        clap::Plugin::take_diagnostics(self)
    }
    fn capabilities(&mut self) -> Result<Capabilities, Error> {
        Ok(clap::Plugin::capabilities(self))
    }
    fn info(&self) -> &PluginInfo {
        clap::Plugin::info(self)
    }

    fn prepare(&mut self, config: &ProcessConfig) -> Result<(), Error> {
        Ok(clap::Plugin::prepare(self, config)?)
    }

    fn processor(&self) -> Box<dyn BlockProcessor> {
        Box::new(clap::Plugin::processor(self))
    }

    fn latency(&self) -> Result<u32, Error> {
        Ok(clap::Plugin::latency(self)?)
    }

    fn tail(&self) -> Result<Tail, Error> {
        Ok(clap::Plugin::tail(self)?)
    }

    fn parameters(&mut self) -> Vec<(ParameterInfo, f64)> {
        clap::Plugin::parameters(self)
    }

    fn parameter_choices(
        &mut self,
        id: u64,
        start: u64,
        count: u32,
    ) -> Result<plughost_core::ParameterChoicePage, Error> {
        clap::Plugin::parameter_choices(self, id, start, count)
    }

    fn parameter_details(&mut self, id: u64) -> Result<plughost_core::ParameterDetails, Error> {
        Ok(clap::Plugin::parameter_details(self, id)?)
    }

    fn set_parameter(&mut self, id: u64, value: f64) -> Result<(), Error> {
        clap::Plugin::set_parameter(self, id, value).map_err(Error::from)
    }

    fn parameter_text(&mut self, id: u64, value: f64) -> Result<String, Error> {
        Ok(clap::Plugin::parameter_text(self, id, value)?)
    }

    fn parameter_from_text(&mut self, id: u64, text: &str) -> Result<f64, Error> {
        Ok(clap::Plugin::parameter_from_text(self, id, text)?)
    }

    fn parameter_to_plain(&mut self, id: u64, value: f64) -> Result<f64, Error> {
        Ok(clap::Plugin::parameter_to_plain(self, id, value)?)
    }

    fn parameter_to_normalized(&mut self, id: u64, plain: f64) -> Result<f64, Error> {
        Ok(clap::Plugin::parameter_to_normalized(self, id, plain)?)
    }

    fn save_state(&mut self, purpose: plughost_core::StatePurpose) -> Result<PluginState, Error> {
        Ok(clap::Plugin::save_state(self, purpose)?)
    }
    fn restore_state(
        &mut self,
        state: &PluginState,
        purpose: plughost_core::StatePurpose,
    ) -> Result<(), Error> {
        Ok(clap::Plugin::restore_state(self, state, purpose)?)
    }

    fn reset(&mut self) -> Result<(), Error> {
        Ok(clap::Plugin::reset(self)?)
    }

    unsafe fn open_editor(
        &mut self,
        parent: *mut c_void,
        resize: ResizeRequest,
    ) -> Result<EditorView, Error> {
        Ok(unsafe { clap::Plugin::open_editor(self, parent, resize)? })
    }

    fn close_editor(&mut self) {
        clap::Plugin::close_editor(self);
    }

    fn set_editor_visible(&mut self, visible: bool) {
        clap::Plugin::set_editor_visible(self, visible);
    }

    fn take_editor_request(&mut self) -> Option<EditorRequest> {
        clap::Plugin::take_editor_request(self)
    }

    fn editor_can_resize(&mut self) -> bool {
        clap::Plugin::editor_can_resize(self)
    }

    fn resize_editor(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        clap::Plugin::resize_editor(self, width, height)
    }

    #[cfg(target_os = "windows")]
    fn set_editor_scale(&mut self, scale: f64) -> Option<(u32, u32)> {
        clap::Plugin::set_editor_scale(self, scale)
    }

    fn idle(&mut self) {
        clap::Plugin::idle(self);
    }
}

#[cfg(feature = "clap")]
impl BlockProcessor for clap::Processor {
    fn process_audio_f32(
        &self,
        context: &plughost_core::BlockContext,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        automation: &[plughost_core::ParameterChange],
        events: &[MidiEvent],
        produced: &mut Vec<MidiEvent>,
    ) -> Result<(), Error> {
        Ok(clap::Processor::process(
            self, context, input, output, automation, events, produced,
        )?)
    }

    fn process_audio_f64(
        &self,
        context: &plughost_core::BlockContext,
        input: &[&[f64]],
        output: &mut [&mut [f64]],
        automation: &[plughost_core::ParameterChange],
        events: &[MidiEvent],
        produced: &mut Vec<MidiEvent>,
    ) -> Result<(), Error> {
        Ok(clap::Processor::process(
            self, context, input, output, automation, events, produced,
        )?)
    }

    fn latency(&self) -> Result<u32, Error> {
        Ok(clap::Processor::latency(self)?)
    }
    fn tail(&self) -> Result<Tail, Error> {
        Ok(clap::Processor::tail(self)?)
    }
    fn restart_required(&self) -> bool {
        clap::Processor::restart_required(self)
    }
}
