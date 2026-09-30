//! One CLAP plugin instance. The instance stays on the thread that created it; its audio
//! processor lives in an engine shared with a processing thread.

mod audio;
mod audio_process;
mod buffers;
mod event_ports;
mod events;
mod input_events;
mod output_events;
mod parameter_cache;
mod parameters;
mod presets;
mod render;
use std::ffi::{CString, c_void};
mod capabilities;
use std::io::Cursor;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use clack_extensions::audio_ports::{AudioPortFlags, AudioPortType};
use clack_extensions::gui::{GuiApiType, GuiConfiguration, GuiSize, Window};
use clack_extensions::note_ports::{NoteDialect, NoteDialects};
use clack_extensions::render::RenderMode;
use clack_extensions::surround::SurroundChannel;
use clack_host::events::event_types::{
    MidiEvent as ClapMidiEvent, NoteOffEvent, NoteOnEvent, ParamValueEvent,
};
use clack_host::events::{Match, Pckn};
use clack_host::plugin::PluginDescriptor;
use clack_host::prelude::*;
use plughost_core::PluginRef;
use plughost_core::render::Tail;
use plughost_core::{DiagnosticBatch, DiagnosticBuffer};
use plughost_core::{
    HostIdentity, Layout, Message, MidiEvent, ParameterInfo, PluginFormat, PluginInfo, PluginKind,
    PluginState, ProcessConfig, ProcessMode, SampleFormat, events_fit,
};

use super::errors::ClapError;
use super::host::{Host, MainThread, Shared, in_audio_context};
use crate::hosted::{EditorRequest, EditorView, ResizeRequest};
use parameter_cache::ParameterCache;

const INSTRUMENT: &str = "instrument";

/// The plugin classes in a `.clap` module.
pub fn classes(bundle: &Path) -> Result<Vec<PluginInfo>, ClapError> {
    let entry = load_entry(bundle)?;
    let factory = entry.get_plugin_factory().ok_or(ClapError::NoFactory)?;
    Ok(factory.plugin_descriptors().filter_map(info).collect())
}

fn load_entry(bundle: &Path) -> Result<PluginEntry, ClapError> {
    // SAFETY: running the module's code is the purpose of loading it.
    unsafe { PluginEntry::load(bundle) }.map_err(|error| ClapError::Load(error.to_string()))
}

fn text(value: Option<&std::ffi::CStr>) -> String {
    value
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn info(descriptor: &PluginDescriptor) -> Option<PluginInfo> {
    let categories: Vec<String> = descriptor
        .features()
        .map(|feature| feature.to_string_lossy().into_owned())
        .collect();
    Some(PluginInfo {
        format: PluginFormat::Clap,
        class_id: descriptor.id()?.to_str().ok()?.to_owned(),
        name: text(descriptor.name()),
        vendor: text(descriptor.vendor()),
        version: text(descriptor.version()),
        sdk_version: "CLAP".to_owned(),
        kind: if categories.iter().any(|c| c == INSTRUMENT) {
            PluginKind::Instrument
        } else {
            PluginKind::Effect
        },
        categories,
    })
}

struct Engine {
    processor: Option<PluginAudioProcessor<Host>>,
    prepared: Option<Prepared>,
}

struct Prepared {
    input_events: input_events::InputBuffer,
    buffers: buffers::AudioBuffers,
    config: ProcessConfig,
    audio_config: Option<plughost_core::AudioConfig>,
    buses: Vec<plughost_core::AudioBusInfo>,
    /// Channel count of every input and output port; the main port is at `main_input` or
    /// `main_output`.
    inputs: Vec<usize>,
    outputs: Vec<usize>,
    main_input: Option<usize>,
    main_output: Option<usize>,
    /// Per note input port index, the dialects of a port the routing uses; events reach only
    /// those ports.
    event_inputs: Vec<Option<NoteDialects>>,
    latency: u32,
    steady_time: u64,
    parameters: Arc<ParameterCache>,
}

fn lock(engine: &Mutex<Engine>) -> MutexGuard<'_, Engine> {
    engine
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct Plugin {
    diagnostics: DiagnosticBuffer,
    info: PluginInfo,
    instance: PluginInstance<Host>,
    engine: Arc<Mutex<Engine>>,
    parameters: Arc<ParameterCache>,
    editor: Option<ResizeRequest>,
    audio_active: std::collections::HashMap<(bool, u64), bool>,
}

impl Plugin {
    pub fn new(
        bundle: &Path,
        class_id: &str,
        identity: &HostIdentity,
    ) -> Result<Plugin, ClapError> {
        let diagnostics = DiagnosticBuffer::default();
        identity.validate().map_err(ClapError::Input)?;
        let entry = load_entry(bundle)?;
        let factory = entry.get_plugin_factory().ok_or(ClapError::NoFactory)?;
        let info = factory
            .plugin_descriptors()
            .filter_map(info)
            .find(|info| info.class_id == class_id)
            .ok_or_else(|| ClapError::ClassNotFound(class_id.to_owned()))?;
        let id =
            CString::new(class_id).map_err(|_| ClapError::ClassNotFound(class_id.to_owned()))?;
        let host_info = HostInfo::new(
            identity.name.as_str(),
            identity.vendor.as_str(),
            "",
            identity.version.as_str(),
        )
        .map_err(|error| ClapError::Instantiate(error.to_string()))?;
        let instance = PluginInstance::<Host>::new(
            |_| {
                Shared::new(
                    diagnostics.clone(),
                    PluginRef {
                        format: PluginFormat::Clap,
                        bundle: Some(bundle.to_owned()),
                        class_id: class_id.to_owned(),
                    },
                )
            },
            |shared| MainThread::new(shared),
            &entry,
            &id,
            &host_info,
        )
        .map_err(|error| ClapError::Instantiate(error.to_string()))?;
        Ok(Plugin {
            diagnostics,
            info,
            instance,
            engine: Arc::new(Mutex::new(Engine {
                processor: None,
                prepared: None,
            })),
            parameters: Arc::default(),
            editor: None,
            audio_active: std::collections::HashMap::new(),
        })
    }

    pub(crate) fn info(&self) -> &PluginInfo {
        &self.info
    }

    /// Drains messages from the CLAP host log extension, including initialization messages.
    pub(crate) fn take_diagnostics(&self) -> DiagnosticBatch {
        self.diagnostics.take()
    }

    fn shared(&self) -> &Shared {
        self.instance.access_shared_handler(|shared| {
            // SAFETY: the shared handler lives as long as the instance, which outlives `self`'s borrow.
            unsafe { &*(shared as *const Shared) }
        })
    }

    /// Stops processing as the plugin's audio thread, then deactivates on the main thread.
    fn deactivate(&mut self) {
        let processor = {
            let mut engine = lock(&self.engine);
            engine.prepared = None;
            engine.processor.take()
        };
        if let Some(processor) = processor {
            let stopped = in_audio_context(|| processor.into_stopped());
            self.instance.deactivate(stopped);
        }
    }

    pub fn config(&self) -> Option<ProcessConfig> {
        lock(&self.engine)
            .prepared
            .as_ref()
            .map(|prepared| prepared.config)
    }

    pub(crate) fn processor(&self) -> Processor {
        Processor {
            engine: Arc::clone(&self.engine),
        }
    }

    /// Processes one block on this thread. `input` and `output` hold the main port channels
    /// after [`HostedPlugin::prepare`], or every active port's channels in native order after
    /// [`HostedPlugin::prepare_audio`]. `events` go to the first note input port: notes as CLAP
    /// note events when the port takes them, other messages (and notes on MIDI-only ports) as
    /// MIDI events when it takes MIDI; the rest are dropped.
    ///
    /// [`HostedPlugin::prepare`]: crate::HostedPlugin::prepare
    /// [`HostedPlugin::prepare_audio`]: crate::HostedPlugin::prepare_audio
    pub fn process<S: plughost_core::Sample>(
        &mut self,
        context: &plughost_core::BlockContext,
        input: &[&[S]],
        output: &mut [&mut [S]],
        automation: &[plughost_core::ParameterChange],
        events: &[MidiEvent],
        produced: &mut Vec<MidiEvent>,
    ) -> Result<(), ClapError> {
        self.processor()
            .process(context, input, output, automation, events, produced)
    }

    pub(crate) fn latency(&self) -> Result<u32, ClapError> {
        lock(&self.engine)
            .prepared
            .as_ref()
            .map(|prepared| prepared.latency)
            .ok_or(ClapError::NotPrepared)
    }

    pub(crate) fn tail(&self) -> Result<Tail, ClapError> {
        let tail = self.shared().extensions().tail;
        let mut engine = lock(&self.engine);
        let processor = engine.processor.as_mut().ok_or(ClapError::NotPrepared)?;
        let Some(tail) = tail else {
            return Ok(Tail::Samples(0));
        };
        let length = in_audio_context(|| tail.get(&processor.plugin_handle()));
        Ok(if length.is_infinite() {
            Tail::Infinite
        } else {
            Tail::Samples(length.to_raw())
        })
    }

    /// The cached parameter list, read again after the plugin rescanned its parameter info. The
    /// processing thread's copy follows.
    fn parameter_cache(&mut self) -> Arc<ParameterCache> {
        if self
            .shared()
            .parameters_changed
            .swap(false, Ordering::Relaxed)
        {
            let params = self.shared().extensions().params;
            self.parameters =
                Arc::new(ParameterCache::read(params, &self.instance.plugin_handle()));
            if let Some(prepared) = &mut lock(&self.engine).prepared {
                prepared.parameters = Arc::clone(&self.parameters);
            }
        }
        Arc::clone(&self.parameters)
    }

    /// Each parameter with its current value, normalized over its range. Queued host edits
    /// reach the plugin first.
    pub(crate) fn parameters(&mut self) -> Vec<(ParameterInfo, f64)> {
        if self.shared().has_pending() {
            self.flush();
        }
        let Some(params) = self.shared().extensions().params else {
            return Vec::new();
        };
        let cache = self.parameter_cache();
        let handle = self.instance.plugin_handle();
        cache
            .iter()
            .map(|parameter| {
                let value = params
                    .get_value(&handle, parameter.native)
                    .map_or(0.0, |plain| parameter.normalized(plain));
                (parameter.info.clone(), value)
            })
            .collect()
    }

    /// Queues a host edit of a normalized value; it reaches the plugin with the next block or
    /// flush.
    pub(crate) fn set_parameter(&mut self, id: u64, value: f64) -> Result<(), ClapError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(ClapError::Input(plughost_core::InputError::ParameterValue));
        }
        let cache = self.parameter_cache();
        let parameter = cache.get(id).ok_or(ClapError::Input(
            plughost_core::InputError::UnknownParameter { id },
        ))?;
        if parameter.info.flags.read_only {
            return Err(ClapError::Input(
                plughost_core::InputError::ReadOnlyParameter { id },
            ));
        }
        let mut pending = self
            .shared()
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if pending.len() == plughost_core::MAX_BLOCK_EVENTS {
            return Err(ClapError::Input(plughost_core::InputError::EventCapacity));
        }
        pending.push((parameter.native.get(), parameter.plain(value)));
        Ok(())
    }

    /// Delivers queued host edits and any the plugin asked to flush.
    fn flush(&mut self) {
        let parameters = self.parameter_cache();
        let shared = self.shared();
        let pending = shared.take_pending();
        shared.flush_requested.store(false, Ordering::Relaxed);
        let Some(params) = shared.extensions().params else {
            return;
        };
        let mut buffer = EventBuffer::new();
        for (id, value) in pending {
            if let Some(id) = ClapId::from_raw(id) {
                buffer.push(&ParamValueEvent::new(0, id, Pckn::match_all(), value));
            }
        }
        let input = InputEvents::from_buffer(&buffer);
        let parameter_events = shared.parameter_events.clone();
        let mut output_buffer = events::ParameterOutput {
            parameters: &parameters,
            queue: &parameter_events,
        };
        let mut output = OutputEvents::from_buffer(&mut output_buffer);
        let mut engine = lock(&self.engine);
        match engine.processor.as_mut() {
            Some(processor) => {
                in_audio_context(|| {
                    params.flush_active(&mut processor.plugin_handle(), &input, &mut output)
                });
                drop(engine);
            }
            None => {
                drop(engine);
                if let Some(mut handle) = self.instance.inactive_plugin_handle() {
                    params.flush(&mut handle, &input, &mut output);
                }
            }
        }
    }

    pub(crate) fn save_state(
        &mut self,
        purpose: plughost_core::StatePurpose,
    ) -> Result<PluginState, ClapError> {
        let context = self.shared().extensions().state_context;
        if context.is_none() && purpose != plughost_core::StatePurpose::Project {
            return Err(ClapError::StatePurposeUnsupported);
        }
        self.flush();
        let state = self
            .shared()
            .extensions()
            .state
            .ok_or(ClapError::StateUnsupported)?;
        let mut bytes = crate::state_stream::StateWriter::new(plughost_core::MAX_STATE_BYTES);
        let result = if let Some(context) = context {
            context.save(
                &self.instance.plugin_handle(),
                &mut bytes,
                state_context(purpose),
            )
        } else {
            state.save(&self.instance.plugin_handle(), &mut bytes)
        };
        if bytes.exceeded() {
            return Err(ClapError::StateTooLarge);
        }
        result.map_err(|_| ClapError::State)?;
        Ok(PluginState {
            format: PluginFormat::Clap,
            class_id: self.info.class_id.clone(),
            name: self.info.name.clone(),
            vendor: self.info.vendor.clone(),
            version: self.info.version.clone(),
            component: bytes.into_bytes(),
            controller: Vec::new(),
        })
    }

    pub(crate) fn restore_state(
        &mut self,
        state: &PluginState,
        purpose: plughost_core::StatePurpose,
    ) -> Result<(), ClapError> {
        state.validate().map_err(ClapError::Input)?;
        let context = self.shared().extensions().state_context;
        if context.is_none() && purpose != plughost_core::StatePurpose::Project {
            return Err(ClapError::StatePurposeUnsupported);
        }
        if state.format != PluginFormat::Clap || state.class_id != self.info.class_id {
            return Err(ClapError::StateMismatch);
        }
        self.shared().take_pending();
        let extension = self
            .shared()
            .extensions()
            .state
            .ok_or(ClapError::StateUnsupported)?;
        if let Some(context) = context {
            context.load(
                &self.instance.plugin_handle(),
                &mut Cursor::new(&state.component),
                state_context(purpose),
            )
        } else {
            extension.load(
                &self.instance.plugin_handle(),
                &mut Cursor::new(&state.component),
            )
        }
        .map_err(|_| ClapError::State)
    }

    /// Clears the plugin's internal audio state with its native `reset`, keeping its settings.
    pub(crate) fn reset(&mut self) -> Result<(), ClapError> {
        let mut engine = lock(&self.engine);
        let processor = engine.processor.as_mut().ok_or(ClapError::NotPrepared)?;
        in_audio_context(|| processor.reset());
        Ok(())
    }

    /// Opens the editor embedded in `parent` and returns its size.
    ///
    /// # Safety
    ///
    /// `parent` must be a valid native view that outlives the editor.
    pub(crate) unsafe fn open_editor(
        &mut self,
        parent: *mut c_void,
        resize: ResizeRequest,
    ) -> Result<EditorView, ClapError> {
        self.close_editor();
        let gui = self.shared().extensions().gui.ok_or(ClapError::NoEditor)?;
        let api = GuiApiType::default_for_current_platform().ok_or(ClapError::NoEditor)?;
        let handle = self.instance.plugin_handle();
        // An editor that cannot draw into the host's view may still open its own window.
        let configuration = [false, true]
            .map(|is_floating| GuiConfiguration {
                api_type: api,
                is_floating,
            })
            .into_iter()
            .find(|configuration| gui.is_api_supported(&handle, *configuration))
            .ok_or(ClapError::NoEditor)?;
        gui.create(&handle, configuration)
            .map_err(|error| ClapError::Editor(error.to_string()))?;
        if configuration.is_floating {
            if let Ok(title) = CString::new(self.info.name.as_str()) {
                gui.suggest_title(&handle, &title);
            }
            if let Err(error) = gui.show(&handle) {
                gui.destroy(&handle);
                return Err(ClapError::Editor(error.to_string()));
            }
            // A floating editor sizes its own window.
            self.editor = Some(Box::new(|_, _| false));
            self.shared().editor_open.store(true, Ordering::Relaxed);
            return Ok(EditorView::Floating);
        }
        #[cfg(target_os = "macos")]
        let window = unsafe { Window::from_cocoa_nsview(parent) };
        #[cfg(target_os = "windows")]
        let window = unsafe { Window::from_win32_hwnd(parent) };
        let attached = unsafe { gui.set_parent(&handle, window) };
        if let Err(error) = attached {
            gui.destroy(&handle);
            return Err(ClapError::Editor(error.to_string()));
        }
        let _ = gui.show(&handle);
        let (width, height) = gui
            .get_size(&handle)
            .map_or((0, 0), |size| (size.width, size.height));
        self.editor = Some(resize);
        self.shared().editor_open.store(true, Ordering::Relaxed);
        Ok(EditorView::Embedded { width, height })
    }

    pub(crate) fn set_editor_visible(&mut self, visible: bool) {
        if self.editor.is_none() {
            return;
        }
        if let Some(gui) = self.shared().extensions().gui {
            let handle = self.instance.plugin_handle();
            let _ = if visible {
                gui.show(&handle)
            } else {
                gui.hide(&handle)
            };
        }
    }

    pub(crate) fn take_editor_request(&mut self) -> Option<EditorRequest> {
        self.shared()
            .editor_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    pub(crate) fn close_editor(&mut self) {
        self.shared().editor_open.store(false, Ordering::Relaxed);
        if self.editor.take().is_some()
            && let Some(gui) = self.shared().extensions().gui
        {
            gui.destroy(&self.instance.plugin_handle());
        }
    }

    pub(crate) fn editor_can_resize(&mut self) -> bool {
        match (&self.editor, self.shared().extensions().gui) {
            (Some(_), Some(gui)) => gui.can_resize(&self.instance.plugin_handle()),
            _ => false,
        }
    }

    /// Applies the monitor scale when the Windows GUI supports it and returns its pixel size.
    #[cfg(target_os = "windows")]
    pub(crate) fn set_editor_scale(&mut self, scale: f64) -> Option<(u32, u32)> {
        self.editor.as_ref()?;
        let gui = self.shared().extensions().gui?;
        let handle = self.instance.plugin_handle();
        gui.set_scale(&handle, scale).ok()?;
        let size = gui.get_size(&handle)?;
        Some((size.width, size.height))
    }

    pub(crate) fn resize_editor(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        self.editor.as_ref()?;
        let gui = self.shared().extensions().gui?;
        let handle = self.instance.plugin_handle();
        let size = gui
            .adjust_size(&handle, GuiSize { width, height })
            .unwrap_or(GuiSize { width, height });
        gui.set_size(&handle, size).ok()?;
        Some((size.width, size.height))
    }

    /// Serves what the plugin asked of the main thread: callbacks, timers, parameter flushes,
    /// restarts, and editor resizes. Call it regularly from the owning thread.
    pub(crate) fn idle(&mut self) {
        let (callback, flush, timer, requested) = {
            let shared = self.shared();
            (
                shared.callback_requested.swap(false, Ordering::Relaxed),
                shared.flush_requested.load(Ordering::Relaxed),
                shared.extensions().timer,
                shared
                    .resize_requested
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take(),
            )
        };
        if callback {
            self.instance.call_on_main_thread_callback();
        }
        if let Some(timer) = timer {
            let now = Instant::now();
            let due: Vec<_> = self.instance.access_handler(|main| {
                let mut timers = main.timers.borrow_mut();
                timers
                    .iter_mut()
                    .filter(|t| t.due <= now)
                    .map(|t| {
                        t.due = now + t.period;
                        t.id
                    })
                    .collect()
            });
            for id in due {
                // An earlier callback of this tick may have removed the timer.
                let registered = self
                    .instance
                    .access_handler(|main| main.timers.borrow().iter().any(|timer| timer.id == id));
                if registered {
                    timer.on_timer(&self.instance.plugin_handle(), id);
                }
            }
        }
        if flush {
            self.flush();
        }
        if let (Some((width, height)), Some(resize)) = (requested, &self.editor) {
            resize(width, height);
        }
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        self.close_editor();
        self.deactivate();
    }
}

/// Exact native speaker orders matching the portable layouts.
fn surround_orders(layout: Layout) -> Option<&'static [&'static [SurroundChannel]]> {
    use SurroundChannel::{
        BackLeft, BackRight, FrontCenter, FrontLeft, FrontRight, LowFrequency, SideLeft, SideRight,
    };
    match layout {
        Layout::Surround51 => Some(&[&[
            FrontLeft,
            FrontRight,
            FrontCenter,
            LowFrequency,
            BackLeft,
            BackRight,
        ]]),
        Layout::Surround71 => Some(&[&[
            FrontLeft,
            FrontRight,
            FrontCenter,
            LowFrequency,
            BackLeft,
            BackRight,
            SideLeft,
            SideRight,
        ]]),
        Layout::None
        | Layout::Mono
        | Layout::Stereo
        | Layout::Lcr
        | Layout::Quad
        | Layout::Surround50
        | Layout::Surround70
        | Layout::Surround512
        | Layout::Surround514
        | Layout::Surround712
        | Layout::Surround714
        | Layout::Surround916
        | Layout::Ambisonics1
        | Layout::Ambisonics2
        | Layout::Ambisonics3
        | Layout::Ambisonics4 => None,
    }
}

/// Processes a CLAP plugin from a processing thread.
#[derive(Clone)]
pub struct Processor {
    engine: Arc<Mutex<Engine>>,
}

impl Processor {
    /// Processes one block. `input` and `output` hold the main port channels after
    /// [`Plugin::prepare`], or every active port's channels in native order after
    /// [`Plugin::prepare_audio`]. `events` go to their requested note input ports: notes as CLAP
    /// note events when the port takes them, other messages (and notes on MIDI-only ports) as MIDI
    /// events when it takes MIDI; the rest are not delivered. `produced` receives the output
    /// events in offset order.
    pub(crate) fn process<S: plughost_core::Sample>(
        &self,
        context: &plughost_core::BlockContext,
        input: &[&[S]],
        output: &mut [&mut [S]],
        automation: &[plughost_core::ParameterChange],
        events: &[MidiEvent],
        produced: &mut Vec<MidiEvent>,
    ) -> Result<(), ClapError> {
        self.process_audio_impl(context, input, output, automation, events, produced)
    }
    pub(crate) fn restart_required(&self) -> bool {
        lock(&self.engine)
            .processor
            .as_ref()
            .is_some_and(|processor| {
                processor.access_shared_handler(|shared| {
                    shared.restart_requested.load(Ordering::Relaxed)
                        || shared.latency_changed.load(Ordering::Relaxed)
                })
            })
    }

    pub(crate) fn tail(&self) -> Result<Tail, ClapError> {
        let mut engine = lock(&self.engine);
        let processor = engine.processor.as_mut().ok_or(ClapError::NotPrepared)?;
        let extension = processor.access_shared_handler(|shared| shared.extensions().tail);
        let Some(extension) = extension else {
            return Ok(Tail::Samples(0));
        };
        let tail = in_audio_context(|| extension.get(&processor.plugin_handle()));
        Ok(if tail.is_infinite() {
            Tail::Infinite
        } else {
            Tail::Samples(tail.to_raw())
        })
    }

    pub(crate) fn latency(&self) -> Result<u32, ClapError> {
        lock(&self.engine)
            .prepared
            .as_ref()
            .map(|prepared| prepared.latency)
            .ok_or(ClapError::NotPrepared)
    }
}

fn state_context(
    purpose: plughost_core::StatePurpose,
) -> clack_extensions::state_context::StateContextType {
    use clack_extensions::state_context::StateContextType;
    match purpose {
        plughost_core::StatePurpose::Project => StateContextType::ForProject,
        plughost_core::StatePurpose::Preset => StateContextType::ForPreset,
        plughost_core::StatePurpose::Duplicate => StateContextType::ForDuplicate,
    }
}
