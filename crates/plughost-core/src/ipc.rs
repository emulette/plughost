//! Messages between an application and a plughost helper process, and their framing: a
//! little-endian `u32` length followed by the postcard encoding. postcard carries no type
//! information, so the application checks the helper's [`PROTOCOL_VERSION`] before sending requests.
//!
//! This module and its shared-memory transport are internal to `plughost` and `plughost-helper`
//! and exempt from semver.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::errors::InputError;
use crate::parameter::ParameterInfo;
use crate::plugin::{PluginInfo, PluginRef};
use crate::render::Tail;
use crate::state::{PluginState, PresetInfo, StatePurpose};
use crate::{CapabilityReport, DiagnosticBatch, Failure, HostIdentity};

pub const PROTOCOL_VERSION: u32 = 22;
/// Upper bound on one message. A helper whose memory a plugin corrupted could send anything.
pub const MAX_MESSAGE_BYTES: usize = 256 << 20;

/// The helper's first message after connecting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    /// The token the application passed on the command line, proving the connection comes from
    /// the helper it started.
    pub token: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Request {
    Changes,
    AudioBuses {
        slot: usize,
    },
    EventPorts {
        slot: usize,
    },
    AudioConfigurations {
        slot: usize,
    },
    Bypass {
        slot: usize,
    },
    SetBypass {
        slot: usize,
        enabled: bool,
    },
    PrepareAudio {
        config: crate::RoutedChainConfig,
        memory: shared::Descriptor,
        handle: u64,
    },
    ParameterEvents {
        slot: usize,
    },
    FactoryPresets {
        slot: usize,
    },
    SelectFactoryPreset {
        slot: usize,
        preset: crate::FactoryPresetId,
    },
    LoadDiscoveredPreset {
        slot: usize,
        location: crate::PresetLocation,
        load_key: Option<String>,
    },
    PresetProviders {
        bundle: PathBuf,
        host: HostIdentity,
    },
    DiscoverPresets {
        bundle: PathBuf,
        host: HostIdentity,
        target: crate::PresetDiscoveryTarget,
    },
    /// Drains bounded plugin and host diagnostic queues. Logs never push unsolicited traffic.
    Diagnostics,
    /// Scan mode: load one bundle and report its classes. An interactive scan brings the helper
    /// to the front first, so the user can answer a dialog the plugin shows while loading.
    Scan {
        bundle: PathBuf,
        interactive: bool,
    },
    /// Scan mode: report the Audio Units in the system registry, without loading them.
    ListAudioUnits,
    /// Host mode: load the chain. `activity` is the transfer token of the application's
    /// [`shared::Activity`], through which the helper announces the slot each of its threads calls.
    Load {
        host: HostIdentity,
        plugins: Vec<PluginRef>,
        activity: u64,
    },
    /// Host mode, right after `Load`: restores a project state into the slot's newly loaded
    /// plugin. Each state goes in its own request, so the states of a chain together may exceed
    /// one message.
    LoadState {
        slot: usize,
        state: crate::PluginState,
    },
    /// One block through the chain, with events on the chain's external event inputs.
    Process {
        context: crate::BlockContext,
        submission: shared::Submission,
    },
    Parameters {
        slot: usize,
    },
    Capabilities {
        slot: usize,
    },
    SetParameter {
        slot: usize,
        id: u64,
        value: f64,
    },
    ParameterDetails {
        slot: usize,
        id: u64,
    },
    ParameterChoices {
        slot: usize,
        id: u64,
        start: u64,
        count: u32,
    },
    ParameterText {
        slot: usize,
        id: u64,
        value: f64,
    },
    ParameterFromText {
        slot: usize,
        id: u64,
        text: String,
    },
    ParameterToPlain {
        slot: usize,
        id: u64,
        value: f64,
    },
    ParameterToNormalized {
        slot: usize,
        id: u64,
        plain: f64,
    },
    SetParameterText {
        slot: usize,
        id: u64,
        text: String,
    },
    SaveState {
        slot: usize,
        purpose: StatePurpose,
    },
    RestoreState {
        slot: usize,
        state: PluginState,
        purpose: StatePurpose,
    },
    InspectPreset {
        slot: usize,
        bytes: Vec<u8>,
    },
    ExportPreset {
        slot: usize,
    },
    ImportPreset {
        slot: usize,
        bytes: Vec<u8>,
    },
    Reset,
    Timing,
    /// Opens the plugin's editor in a window of the helper, which processes while it is open.
    OpenEditor {
        slot: usize,
    },
    CloseEditor {
        slot: usize,
    },
    /// Whether the editor is still open; the user may have closed its window.
    EditorOpen {
        slot: usize,
    },
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Response {
    Changes(crate::ChangeBatch),
    AudioBuses(Vec<crate::AudioBusInfo>),
    EventPorts(Vec<crate::EventPortInfo>),
    AudioConfigurations(Vec<crate::AudioConfiguration>),
    Bypass(crate::BypassState),
    AudioPrepared {
        generation: u64,
        buses: Vec<Vec<crate::AudioBusInfo>>,
        latency: u32,
        tail: Tail,
    },
    ParameterEvents(crate::ParameterEventBatch),
    FactoryPresets(Vec<crate::FactoryPreset>),
    PresetProviders(Vec<crate::PresetProviderInfo>),
    DiscoveredPresets(crate::PresetDiscovery),
    Diagnostics(DiagnosticBatch),
    /// Scan progress: the module loaded.
    ModuleLoaded,
    /// Scan progress: one audio module class.
    Class(PluginInfo),
    Loaded(Vec<PluginInfo>),
    Done,
    /// Confirms a published shared output, with the chain timing after the block.
    Processed {
        submission: shared::Submission,
        /// Output events written to the shared output event slots.
        events: usize,
        latency: u32,
        tail: Tail,
    },
    /// Each parameter with its current normalized value.
    Parameters(Vec<(ParameterInfo, f64)>),
    Capabilities(CapabilityReport),
    ParameterDetails(crate::ParameterDetails),
    ParameterChoices(crate::ParameterChoicePage),
    ParameterText(String),
    ParameterValue(f64),
    State(PluginState),
    PresetInfo(PresetInfo),
    Preset(Vec<u8>),
    StateRestored {
        timing: Option<(u32, Tail)>,
    },
    /// Latency and tail of the whole chain.
    Timing {
        latency: u32,
        tail: Tail,
    },
    EditorOpen(bool),
    /// Invalid caller input; the helper remains usable and no mutation was attempted.
    Rejected {
        slot: Option<usize>,
        error: InputError,
    },
    Failed {
        slot: Option<usize>,
        failure: Failure,
    },
}

/// Local planar scratch with the same precision as the prepared shared slot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AudioSamples {
    F32(Vec<Vec<f32>>),
    F64(Vec<Vec<f64>>),
}

mod framing;
pub use framing::{FrameDecoder, MessageReader, MessageWriter, encode_message, write_message};

pub mod shared;
