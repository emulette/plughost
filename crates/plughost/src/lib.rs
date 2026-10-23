//! Load and run VST3, Audio Unit, and CLAP plugins from Rust, isolated in helper processes.
//!
//! Plugin code never runs in the application's process. A [`Scanner`] finds installed plugins,
//! running one short-lived helper per bundle; a [`Chain`] loads plugins into one helper and
//! processes audio through them in order. If a plugin crashes or hangs, only its helper ends,
//! and errors report the last known plugin slot when available.
//!
//! The helper is a small executable the application builds around `plughost_helper::run`, from
//! the same plughost version. On macOS it is a background `.app` signed with the Hardened Runtime
//! and the `disable-library-validation` and `allow-unsigned-executable-memory` entitlements.
//!
//! [`Chain::take_diagnostics`] drains structured plugin logs and helper operation failures.
//! Startup errors and arbitrary plugin stderr remain on standard error; plugin stdout is discarded.
//!
//! [`Chain::prepare_audio`] selects native buses, explicit routing and f32 or f64 precision;
//! [`Chain::main_bus_config`] builds the common serial main-bus configuration. Rendering retains the
//! prepared sample type. For instruments with an empty input slice, specify
//! `render::<f32, _>` or `render::<f64, _>` so Rust can infer the output precision.
//!
//! The [detailed API guide](https://github.com/emulette/plughost/blob/v0.1.6/llms.txt)
//! covers scanning, cancellation, routing, state, and failure contracts.
//!
//! # Stereo effect rendering
//!
//! ```no_run
//! use std::path::Path;
//! use plughost::{
//!     Chain, Events, Layout, PluginRef, RenderOptions, SpawnOptions, render,
//! };
//!
//! fn render_effect(
//!     helper: &Path,
//!     plugin: PluginRef,
//!     left: &[f32],
//!     right: &[f32],
//! ) -> Result<Vec<Vec<f32>>, plughost::Error> {
//!     let mut chain = Chain::spawn(helper, &[plugin.into()], &SpawnOptions::default())?;
//!     let config = chain.main_bus_config(48_000.0, 512, Layout::Stereo, &[Layout::Stereo])?;
//!     chain.prepare_audio(&config)?;
//!     let output = render(
//!         &mut chain,
//!         &[left, right],
//!         left.len(),
//!         &Events::new(),
//!         &RenderOptions::default(),
//!     )?;
//!     Ok(output.channels)
//! }
//! ```

#![warn(missing_docs)]

mod chain;
mod errors;
mod helper;
mod preset_paths;
mod presets;
mod realtime;
mod scan;

pub use chain::{Chain, ChainSlot, ChainSnapshot, SpawnOptions, Timeouts};
pub use errors::Error;
pub use helper::{ChainCanceller, HelperMonitor};
pub use plughost_core::render::{
    AutomationRamp, Delivery, Process, RenderInput, RenderOptions, RenderProgress, RenderSchedule,
    RenderSession, RenderStatus, Rendered, SessionState, Tail, TailPolicy, TransportChange, render,
    render_stream, render_with_schedule,
};
pub use plughost_core::{
    AudioBusConfig, AudioBusInfo, AudioBusRole, AudioConfiguration, AudioDirection,
    AudioInputRoute, AudioSource, AutomationEvent, BarPosition, BlockContext, BypassState,
    CHANGE_CAPACITY, Capabilities, CapabilityReport, ChangeBatch, ChannelAdaptation, ClassId,
    DIAGNOSTIC_CAPACITY, DIAGNOSTIC_MESSAGE_BYTES, Diagnostic, DiagnosticBatch, DiagnosticSeverity,
    DiscoveredPreset, Event, EventConfig, EventData, EventInputRoute, EventPortInfo, EventSource,
    Events, EventsIter, ExpressionKind, FactoryPreset, FactoryPresetId, Failure, FailureKind,
    HostIdentity, InputError, Layout, LoopRegion, MAX_ALIGNMENT_BYTES, MAX_AUDIO_BUSES,
    MAX_AUDIO_CHANNELS, MAX_BLOCK_EVENTS, MAX_BLOCK_SYSEX_BYTES, MAX_EVENT_PORTS,
    MAX_FACTORY_PRESETS, MAX_NOTE_ID, MAX_PRESET_BYTES, MAX_STATE_BYTES, Message, Note,
    NoteExpression, PARAMETER_CHOICE_PAGE_SIZE, PARAMETER_EVENT_CAPACITY, PARAMETER_TEXT_BYTES,
    PRESET_DISCOVERY_TEXT_BYTES, ParameterChange, ParameterChoice, ParameterChoicePage,
    ParameterDetails, ParameterEvent, ParameterEventBatch, ParameterFlags, ParameterGroup,
    ParameterInfo, PluginFormat, PluginId, PluginInfo, PluginKind, PluginRef, PluginState,
    PluginTiming, PresetDiscovery, PresetDiscoveryTarget, PresetFileFailure, PresetFileType,
    PresetFlags, PresetInfo, PresetLocation, PresetLocationInfo, PresetMetadata, PresetPluginId,
    PresetProviderInfo, PresetSoundpack, ProcessMode, RenderError, RoutedChainConfig, Sample,
    SampleFormat, SlotAudioConfig, SlotChange, SlotEventConfig, StatePurpose, Support,
    TimeSignature, Transport,
};
pub use preset_paths::{
    PresetDirectory, PresetFile, PresetSearchError, discover_preset_files,
    standard_preset_directories,
};
pub use presets::{discover_presets, preset_providers};
pub use realtime::{
    Fallback, FallbackReason, RealtimeConfig, RealtimeEndpoint, RealtimeFailure, RealtimeOutput,
    RealtimeStatus,
};
pub use scan::{
    BundleScan, Catalog, ScanAction, ScanControl, ScanEvent, ScanItem, ScanOutcome, ScanPolicy,
    ScanStage, ScanTarget, Scanner, Source, default_directories,
};

// Parser entry points for cargo-fuzz; absent from normal builds.
#[cfg(fuzzing)]
#[doc(hidden)]
pub use scan::fuzz;
