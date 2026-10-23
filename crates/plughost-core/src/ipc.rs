//! Messages between an application and a plughost helper process, and their framing: a
//! little-endian `u32` length followed by the postcard encoding. postcard carries no type
//! information, so the application checks the helper's [`PROTOCOL_VERSION`] before sending requests.
//!
//! This module and its shared-memory transport are internal to `plughost` and `plughost-helper`
//! and exempt from semver.

use std::borrow::Cow;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::errors::InputError;
use crate::parameter::ParameterInfo;
use crate::plugin::{PluginInfo, PluginRef};
use crate::render::Tail;
use crate::state::{PluginState, PresetInfo, StatePurpose};
use crate::{CapabilityReport, DiagnosticBatch, Failure, HostIdentity};

pub const PROTOCOL_VERSION: u32 = 33;
/// Upper bound on one message. A helper whose memory a plugin corrupted could send anything.
pub const MAX_MESSAGE_BYTES: usize = 256 << 20;

/// The helper's first message after connecting. Its encoding is the same in every protocol
/// version, so an application can report the version of a helper built with another one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    /// The token the application passed on the command line, proving the connection comes from
    /// the helper it started.
    pub token: u64,
}

/// Outgoing state and preset payloads can borrow caller storage until serialization. Decoding
/// always owns these payloads, so queued helper requests remain independent of receive buffers.
#[serde_as]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Request<'a> {
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
        #[serde_as(as = "wire::RoutedChainConfigDef")]
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
        #[serde_as(as = "wire::FactoryPresetIdDef")]
        preset: crate::FactoryPresetId,
    },
    LoadDiscoveredPreset {
        slot: usize,
        #[serde_as(as = "wire::PresetLocationDef")]
        location: crate::PresetLocation,
        load_key: Option<String>,
    },
    PresetProviders {
        bundle: PathBuf,
        #[serde_as(as = "wire::HostIdentityDef")]
        host: HostIdentity,
    },
    DiscoverPresets {
        bundle: PathBuf,
        #[serde_as(as = "wire::HostIdentityDef")]
        host: HostIdentity,
        #[serde_as(as = "wire::PresetDiscoveryTargetDef")]
        target: crate::PresetDiscoveryTarget,
    },
    /// Drains bounded plugin and host diagnostic queues. Logs never push unsolicited traffic.
    Diagnostics,
    /// Scan mode: load one bundle and report its classes. An interactive scan brings the helper
    /// to the front first, so the user can answer a dialog the plugin shows while loading.
    Scan {
        bundle: PathBuf,
        #[serde_as(as = "wire::HostIdentityDef")]
        host: HostIdentity,
        interactive: bool,
    },
    /// Scan mode: report the Audio Units in the system registry, without loading them.
    ListAudioUnits,
    /// Validate each distinct render automation target before the first block advances DSP.
    ValidateAutomation {
        targets: Vec<(usize, u64)>,
    },
    /// Host mode: load the chain. `activity` is the transfer token of the application's
    /// [`shared::Activity`], through which the helper announces the slot each of its threads calls.
    Load {
        #[serde_as(as = "wire::HostIdentityDef")]
        host: HostIdentity,
        #[serde_as(as = "Vec<wire::PluginRefDef>")]
        plugins: Vec<PluginRef>,
        activity: u64,
    },
    /// Host mode, right after `Load`: restores a project state into the slot's newly loaded
    /// plugin. Each state goes in its own request, so the states of a chain together may exceed
    /// one message.
    LoadState {
        slot: usize,
        #[serde(
            serialize_with = "state::serialize",
            deserialize_with = "state::deserialize_cow"
        )]
        state: Cow<'a, PluginState>,
    },
    /// One block through the chain, with events on the chain's external event inputs.
    Process {
        #[serde_as(as = "wire::BlockContextDef")]
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
        #[serde_as(as = "wire::StatePurposeDef")]
        purpose: StatePurpose,
    },
    RestoreState {
        slot: usize,
        #[serde(
            serialize_with = "state::serialize",
            deserialize_with = "state::deserialize_cow"
        )]
        state: Cow<'a, PluginState>,
        #[serde_as(as = "wire::StatePurposeDef")]
        purpose: StatePurpose,
    },
    InspectPreset {
        slot: usize,
        bytes: Cow<'a, [u8]>,
    },
    ExportPreset {
        slot: usize,
    },
    ImportPreset {
        slot: usize,
        bytes: Cow<'a, [u8]>,
    },
    /// Replaces the slot's instance with a fresh one restored from the active one's project
    /// state, staged like `RestoreState`.
    ReprepareSlot {
        slot: usize,
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
    /// Installs the realtime ring the transfers `tokens` name, for the prepared chain, replacing
    /// one installed before. Preparing the chain again removes it.
    RealtimeInstall {
        ring: shared::realtime::RingDescriptor,
        tokens: Vec<u64>,
    },
    /// Starts the installed ring's worker at the generation the application stored in the ring.
    RealtimeStart,
    /// Stops the realtime worker between quanta.
    RealtimeStop,
    /// Resynchronizes the running worker: it adopts the generation the application stored in
    /// the ring between quanta, and every slot is reset as by `Reset`. Answered with the timing.
    RealtimeResync,
    Shutdown,
}

impl Request<'_> {
    /// The single plugin target, when this control request has one.
    pub fn slot(&self) -> Option<usize> {
        match self {
            Self::AudioBuses { slot }
            | Self::EventPorts { slot }
            | Self::AudioConfigurations { slot }
            | Self::Bypass { slot }
            | Self::SetBypass { slot, .. }
            | Self::ParameterEvents { slot }
            | Self::FactoryPresets { slot }
            | Self::SelectFactoryPreset { slot, .. }
            | Self::LoadDiscoveredPreset { slot, .. }
            | Self::Parameters { slot }
            | Self::Capabilities { slot }
            | Self::SetParameter { slot, .. }
            | Self::ParameterDetails { slot, .. }
            | Self::ParameterChoices { slot, .. }
            | Self::ParameterText { slot, .. }
            | Self::ParameterFromText { slot, .. }
            | Self::ParameterToPlain { slot, .. }
            | Self::ParameterToNormalized { slot, .. }
            | Self::SetParameterText { slot, .. }
            | Self::InspectPreset { slot, .. }
            | Self::ExportPreset { slot }
            | Self::ImportPreset { slot, .. }
            | Self::ReprepareSlot { slot }
            | Self::SaveState { slot, .. }
            | Self::RestoreState { slot, .. }
            | Self::LoadState { slot, .. }
            | Self::OpenEditor { slot }
            | Self::CloseEditor { slot }
            | Self::EditorOpen { slot } => Some(*slot),
            _ => None,
        }
    }
}

#[serde_as]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Response {
    Changes(#[serde_as(as = "wire::ChangeBatchDef")] crate::ChangeBatch),
    AudioBuses(#[serde_as(as = "Vec<wire::AudioBusInfoDef>")] Vec<crate::AudioBusInfo>),
    EventPorts(#[serde_as(as = "Vec<wire::EventPortInfoDef>")] Vec<crate::EventPortInfo>),
    AudioConfigurations(
        #[serde_as(as = "Vec<wire::AudioConfigurationDef>")] Vec<crate::AudioConfiguration>,
    ),
    Bypass(#[serde_as(as = "wire::BypassStateDef")] crate::BypassState),
    AudioPrepared {
        generation: u64,
        #[serde_as(as = "Vec<Vec<wire::AudioBusInfoDef>>")]
        buses: Vec<Vec<crate::AudioBusInfo>>,
        latency: u32,
        #[serde_as(as = "wire::TailDef")]
        tail: Tail,
    },
    ParameterEvents(#[serde_as(as = "wire::ParameterEventBatchDef")] crate::ParameterEventBatch),
    FactoryPresets(#[serde_as(as = "Vec<wire::FactoryPresetDef>")] Vec<crate::FactoryPreset>),
    PresetProviders(
        #[serde_as(as = "Vec<wire::PresetProviderInfoDef>")] Vec<crate::PresetProviderInfo>,
    ),
    DiscoveredPresets(#[serde_as(as = "wire::PresetDiscoveryDef")] crate::PresetDiscovery),
    Diagnostics(#[serde_as(as = "wire::DiagnosticBatchDef")] DiagnosticBatch),
    /// Scan progress: the module loaded.
    ModuleLoaded,
    /// Scan progress: one audio module class.
    Class(#[serde_as(as = "wire::PluginInfoDef")] PluginInfo),
    Loaded(#[serde_as(as = "Vec<wire::PluginInfoDef>")] Vec<PluginInfo>),
    Done,
    /// Host progress: a request that loads or prepares the slots one at a time finished one,
    /// which renews the application's deadline for the rest.
    Progress,
    /// Confirms a published shared output, with the chain timing after the block.
    Processed {
        submission: shared::Submission,
        /// Output events written to the shared output event slots.
        events: usize,
        latency: u32,
        #[serde_as(as = "wire::TailDef")]
        tail: Tail,
    },
    /// Each parameter with its current normalized value.
    Parameters(
        #[serde_as(as = "Vec<(wire::ParameterInfoDef, _)>")] Vec<(ParameterInfo, Option<f64>)>,
    ),
    Capabilities(#[serde_as(as = "wire::CapabilityReportDef")] CapabilityReport),
    ParameterDetails(#[serde_as(as = "wire::ParameterDetailsDef")] crate::ParameterDetails),
    ParameterChoices(#[serde_as(as = "wire::ParameterChoicePageDef")] crate::ParameterChoicePage),
    ParameterText(String),
    ParameterValue(f64),
    State(#[serde(with = "state")] PluginState),
    PresetInfo(#[serde_as(as = "wire::PresetInfoDef")] PresetInfo),
    Preset(Vec<u8>),
    StateRestored {
        #[serde_as(as = "Option<(_, wire::TailDef)>")]
        timing: Option<(u32, Tail)>,
    },
    /// Latency and tail of the whole chain, and whether a plugin announced a latency that is
    /// not applied yet.
    Timing {
        latency: u32,
        #[serde_as(as = "wire::TailDef")]
        tail: Tail,
        latency_changed: bool,
    },
    EditorOpen(bool),
    /// Invalid caller input; the helper remains usable and no mutation was attempted.
    Rejected {
        slot: Option<usize>,
        #[serde_as(as = "wire::InputErrorDef")]
        error: InputError,
    },
    Failed {
        slot: Option<usize>,
        #[serde_as(as = "wire::FailureDef")]
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
mod state;
mod wire;
pub use framing::{FrameDecoder, MessageReader, MessageWriter, encode_message, write_message};

pub mod shared;

#[cfg(any(test, fuzzing))]
pub mod fuzz;
