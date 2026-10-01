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

//! # Stereo effect rendering
//!
//! ```no_run
//! use std::path::Path;
//! use plughost::{
//!     Chain, HostIdentity, Layout, PluginRef, RenderOptions, Timeouts, render,
//! };
//!
//! fn render_effect(
//!     helper: &Path,
//!     plugin: PluginRef,
//!     left: &[f32],
//!     right: &[f32],
//! ) -> Result<Vec<Vec<f32>>, plughost::Error> {
//!     let mut chain = Chain::spawn(helper, &[plugin], &HostIdentity::default(), Timeouts::default())?;
//!     let config = chain.main_bus_config(48_000.0, 512, Layout::Stereo, &[Layout::Stereo])?;
//!     chain.prepare_audio(&config)?;
//!     let output = render(&mut chain, &[left, right], left.len(), &[], &RenderOptions::default())?;
//!     Ok(output.channels)
//! }
//! ```

mod chain;
mod errors;
mod helper;
mod preset_paths;
mod presets;
mod scan;

pub use chain::{Chain, Timeouts};
pub use errors::Error;
pub use helper::HelperMonitor;
pub use plughost_core::render::{
    AutomationRamp, Delivery, RenderInput, RenderOptions, RenderProgress, RenderSchedule,
    RenderSession, RenderStatus, Rendered, SessionState, Tail, TailPolicy, TransportChange, render,
    render_stream, render_with_schedule,
};
pub use plughost_core::{
    AudioBusConfig, AudioBusInfo, AudioBusRole, AudioConfiguration, AudioDirection,
    AudioInputRoute, AudioSource, AutomationEvent, BarPosition, BlockContext, BypassState,
    Capabilities, CapabilityReport, ChangeBatch, ChannelAdaptation, Diagnostic, DiagnosticBatch,
    DiagnosticSeverity, DiscoveredPreset, Event, EventConfig, EventData, EventInputRoute,
    EventPortInfo, EventSource, ExpressionKind, FactoryPreset, FactoryPresetId, Failure,
    FailureKind, HostIdentity, InputError, Layout, LoopRegion, MAX_BLOCK_EVENTS,
    MAX_BLOCK_SYSEX_BYTES, MAX_EVENT_PORTS, MAX_NOTE_ID, MAX_PRESET_BYTES, MAX_STATE_BYTES,
    Message, Note, NoteExpression, PARAMETER_CHOICE_PAGE_SIZE, ParameterChange, ParameterChoice,
    ParameterChoicePage, ParameterDetails, ParameterEvent, ParameterEventBatch, ParameterGroup,
    ParameterInfo, PluginFormat, PluginInfo, PluginKind, PluginRef, PluginState, PluginTiming,
    PresetDiscovery, PresetDiscoveryTarget, PresetFileFailure, PresetFileType, PresetInfo,
    PresetLocation, PresetLocationInfo, PresetMetadata, PresetPluginId, PresetProviderInfo,
    PresetSoundpack, ProcessMode, RoutedChainConfig, SampleFormat, SlotAudioConfig, SlotChange,
    SlotEventConfig, StatePurpose, Support, TimeSignature, Transport,
};
pub use preset_paths::{
    PresetDirectory, PresetFile, PresetSearchError, discover_preset_files,
    standard_preset_directories,
};
pub use presets::{discover_presets, preset_providers};
pub use scan::{
    BundleScan, Catalog, ScanAction, ScanControl, ScanEvent, ScanItem, ScanOutcome, ScanPolicy,
    ScanStage, ScanTarget, Scanner, Source, default_directories,
};
