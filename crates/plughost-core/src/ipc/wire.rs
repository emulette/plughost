//! The helper protocol's serde form of the public types it carries. The public types have no
//! serde form of their own; these remote definitions mirror their fields and change with
//! [`super::PROTOCOL_VERSION`].

use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DeserializeAs, SerializeAs, serde_as};

use crate::render::Tail;
use crate::*;

/// Lets a remote definition stand for its type in `serde_as` containers.
macro_rules! remote {
    ($def:ident, $ty:ty) => {
        impl SerializeAs<$ty> for $def {
            fn serialize_as<S: Serializer>(value: &$ty, serializer: S) -> Result<S::Ok, S::Error> {
                $def::serialize(value, serializer)
            }
        }

        impl<'de> DeserializeAs<'de, $ty> for $def {
            fn deserialize_as<D: Deserializer<'de>>(deserializer: D) -> Result<$ty, D::Error> {
                $def::deserialize(deserializer)
            }
        }
    };
}

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::AudioBusConfig")]
pub(super) struct AudioBusConfigDef {
    id: u64,
    #[serde_as(as = "LayoutDef")]
    layout: Layout,
    active: bool,
}
remote!(AudioBusConfigDef, crate::AudioBusConfig);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::AudioBusInfo")]
pub(super) struct AudioBusInfoDef {
    id: u64,
    index: u32,
    name: String,
    #[serde_as(as = "AudioDirectionDef")]
    direction: AudioDirection,
    #[serde_as(as = "AudioBusRoleDef")]
    role: AudioBusRole,
    #[serde_as(as = "Option<LayoutDef>")]
    layout: Option<Layout>,
    channels: u32,
    active: Option<bool>,
    #[serde_as(as = "SupportDef")]
    f32: Support,
    #[serde_as(as = "SupportDef")]
    f64: Support,
}
remote!(AudioBusInfoDef, crate::AudioBusInfo);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::AudioBusRole")]
pub(super) enum AudioBusRoleDef {
    Main,
    Auxiliary,
}
remote!(AudioBusRoleDef, crate::AudioBusRole);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::AudioConfiguration")]
pub(super) struct AudioConfigurationDef {
    id: u64,
    name: String,
    #[serde_as(as = "Vec<AudioBusInfoDef>")]
    inputs: Vec<AudioBusInfo>,
    #[serde_as(as = "Vec<AudioBusInfoDef>")]
    outputs: Vec<AudioBusInfo>,
}
remote!(AudioConfigurationDef, crate::AudioConfiguration);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::AudioDirection")]
pub(super) enum AudioDirectionDef {
    Input,
    Output,
}
remote!(AudioDirectionDef, crate::AudioDirection);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::AudioInputRoute")]
pub(super) struct AudioInputRouteDef {
    #[serde_as(as = "AudioBusConfigDef")]
    bus: AudioBusConfig,
    #[serde_as(as = "AudioSourceDef")]
    source: AudioSource,
    #[serde_as(as = "ChannelAdaptationDef")]
    adaptation: ChannelAdaptation,
}
remote!(AudioInputRouteDef, crate::AudioInputRoute);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::AudioSource")]
pub(super) enum AudioSourceDef {
    External { bus: usize },
    Previous { bus: u64 },
    Silence,
}
remote!(AudioSourceDef, crate::AudioSource);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::BypassState")]
pub(super) struct BypassStateDef {
    #[serde_as(as = "SupportDef")]
    support: Support,
    enabled: Option<bool>,
}
remote!(BypassStateDef, crate::BypassState);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::ChannelAdaptation")]
pub(super) enum ChannelAdaptationDef {
    Exact,
    MonoToStereo,
    StereoToMono,
}
remote!(ChannelAdaptationDef, crate::ChannelAdaptation);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::RoutedChainConfig")]
pub(super) struct RoutedChainConfigDef {
    sample_rate: f64,
    max_block_size: usize,
    #[serde_as(as = "SampleFormatDef")]
    sample_format: SampleFormat,
    #[serde_as(as = "ProcessModeDef")]
    mode: ProcessMode,
    #[serde_as(as = "Vec<LayoutDef>")]
    inputs: Vec<Layout>,
    event_inputs: usize,
    #[serde_as(as = "Vec<SlotAudioConfigDef>")]
    slots: Vec<SlotAudioConfig>,
}
remote!(RoutedChainConfigDef, crate::RoutedChainConfig);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::SlotAudioConfig")]
pub(super) struct SlotAudioConfigDef {
    configuration: Option<u64>,
    #[serde_as(as = "Vec<AudioInputRouteDef>")]
    inputs: Vec<AudioInputRoute>,
    #[serde_as(as = "Vec<AudioBusConfigDef>")]
    outputs: Vec<AudioBusConfig>,
    #[serde_as(as = "SlotEventConfigDef")]
    events: crate::SlotEventConfig,
}
remote!(SlotAudioConfigDef, crate::SlotAudioConfig);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::Capabilities")]
pub(super) struct CapabilitiesDef {
    #[serde_as(as = "SupportDef")]
    embedded_editor: Support,
    #[serde_as(as = "SupportDef")]
    bus_discovery: Support,
    #[serde_as(as = "SupportDef")]
    note_input: Support,
    #[serde_as(as = "SupportDef")]
    note_output: Support,
    #[serde_as(as = "SupportDef")]
    sample_accurate_automation: Support,
    #[serde_as(as = "SupportDef")]
    state: Support,
    #[serde_as(as = "SupportDef")]
    factory_presets: Support,
    #[serde_as(as = "SupportDef")]
    f32: Support,
    #[serde_as(as = "SupportDef")]
    f64: Support,
}
remote!(CapabilitiesDef, crate::Capabilities);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::CapabilityReport")]
pub(super) struct CapabilityReportDef {
    #[serde_as(as = "CapabilitiesDef")]
    plugin: Capabilities,
    #[serde_as(as = "CapabilitiesDef")]
    host: Capabilities,
}
remote!(CapabilityReportDef, crate::CapabilityReport);

#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::Rect")]
pub(super) struct RectDef {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}
remote!(RectDef, crate::remote::Rect);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::Region")]
pub(super) struct RegionDef {
    display: u32,
    #[serde_as(as = "RectDef")]
    rect: remote::Rect,
}
remote!(RegionDef, crate::remote::Region);

#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::EditorId")]
pub(super) struct EditorIdDef(u64);
remote!(EditorIdDef, crate::remote::EditorId);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::Opened")]
pub(super) struct OpenedDef {
    #[serde_as(as = "EditorIdDef")]
    editor: remote::EditorId,
    width: f64,
    height: f64,
}
remote!(OpenedDef, crate::remote::Opened);

#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::WindowKind")]
pub(super) enum WindowKindDef {
    Editor,
    Menu,
    Dialog,
    Popup,
}
remote!(WindowKindDef, crate::remote::WindowKind);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::Window")]
pub(super) struct WindowDef {
    id: u64,
    #[serde_as(as = "WindowKindDef")]
    kind: remote::WindowKind,
    #[serde_as(as = "RectDef")]
    frame: remote::Rect,
    outside: bool,
}
remote!(WindowDef, crate::remote::Window);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::Notice")]
pub(super) enum NoticeDef {
    Windows {
        #[serde_as(as = "EditorIdDef")]
        editor: remote::EditorId,
        #[serde_as(as = "Vec<WindowDef>")]
        windows: Vec<remote::Window>,
    },
    EditorClosed {
        #[serde_as(as = "EditorIdDef")]
        editor: remote::EditorId,
        slot: usize,
    },
    Nested {
        depth: u32,
    },
    Cursor {
        #[serde_as(as = "CursorShapeDef")]
        shape: remote::CursorShape,
        hidden: bool,
    },
    MenuOpened {
        #[serde_as(as = "MenuIdDef")]
        menu: remote::MenuId,
        #[serde_as(as = "MenuTreeDef")]
        items: Vec<remote::MenuItem>,
        #[serde_as(as = "Option<RectDef>")]
        frame: Option<remote::Rect>,
    },
    MenuClosed {
        #[serde_as(as = "MenuIdDef")]
        menu: remote::MenuId,
    },
    PanelRequested {
        #[serde_as(as = "PanelIdDef")]
        panel: remote::PanelId,
        #[serde_as(as = "PanelRequestDef")]
        request: remote::PanelRequest,
    },
    PanelFinished {
        #[serde_as(as = "PanelIdDef")]
        panel: remote::PanelId,
    },
    OpenUrl {
        url: String,
    },
    RevealFiles {
        paths: Vec<PathBuf>,
    },
    DisplayLost,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::PanelId")]
pub(super) struct PanelIdDef(u64);
remote!(PanelIdDef, crate::remote::PanelId);

#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::PanelKind")]
pub(super) enum PanelKindDef {
    Open,
    Save,
}
remote!(PanelKindDef, crate::remote::PanelKind);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::PanelRequest")]
pub(super) struct PanelRequestDef {
    #[serde_as(as = "PanelKindDef")]
    kind: remote::PanelKind,
    directory: Option<PathBuf>,
    name: String,
    types: Vec<String>,
    multiple: bool,
    directories: bool,
    files: bool,
    packages: bool,
    message: String,
}
remote!(PanelRequestDef, crate::remote::PanelRequest);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::PanelAnswer")]
pub(super) struct PanelAnswerDef {
    #[serde_as(as = "PanelIdDef")]
    panel: remote::PanelId,
    paths: Option<Vec<PathBuf>>,
}
remote!(PanelAnswerDef, crate::remote::PanelAnswer);

#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::MenuId")]
pub(super) struct MenuIdDef(u64);
remote!(MenuIdDef, crate::remote::MenuId);

#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::MenuItemState")]
pub(super) enum MenuItemStateDef {
    Off,
    On,
    Mixed,
}
remote!(MenuItemStateDef, crate::remote::MenuItemState);

/// A menu's items as a list in order, each with its depth: a tree decoded recursively would let
/// a helper exhaust the application's stack with a deeply nested menu.
pub(super) struct MenuTreeDef;

#[serde_as]
#[derive(Serialize, Deserialize)]
struct FlatMenuItem {
    depth: usize,
    index: usize,
    title: String,
    enabled: bool,
    #[serde_as(as = "MenuItemStateDef")]
    state: remote::MenuItemState,
    separator: bool,
}

fn flatten(items: &[remote::MenuItem], depth: usize, flat: &mut Vec<FlatMenuItem>) {
    for item in items {
        flat.push(FlatMenuItem {
            depth,
            index: item.index,
            title: item.title.clone(),
            enabled: item.enabled,
            state: item.state,
            separator: item.separator,
        });
        flatten(&item.submenu, depth + 1, flat);
    }
}

impl SerializeAs<Vec<remote::MenuItem>> for MenuTreeDef {
    fn serialize_as<S: Serializer>(
        items: &Vec<remote::MenuItem>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut flat = Vec::new();
        flatten(items, 0, &mut flat);
        flat.serialize(serializer)
    }
}

impl<'de> DeserializeAs<'de, Vec<remote::MenuItem>> for MenuTreeDef {
    fn deserialize_as<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<remote::MenuItem>, D::Error> {
        /// Gives the items of the deepest open level to the last item of the level above.
        fn close(levels: &mut Vec<Vec<remote::MenuItem>>) {
            if let Some(items) = levels.pop()
                && let Some(parent) = levels.last_mut().and_then(|level| level.last_mut())
            {
                parent.submenu = items;
            }
        }
        let flat = Vec::<FlatMenuItem>::deserialize(deserializer)?;
        let mut levels: Vec<Vec<remote::MenuItem>> = vec![Vec::new()];
        for item in flat {
            // An item is at most one level below the one before it, under an item.
            let opens =
                item.depth == levels.len() && levels.last().is_some_and(|level| !level.is_empty());
            if item.depth > super::MAX_MENU_DEPTH || (item.depth >= levels.len() && !opens) {
                return Err(serde::de::Error::custom(crate::errors::MALFORMED_MENU));
            }
            while levels.len() > item.depth + 1 {
                close(&mut levels);
            }
            if opens {
                levels.push(Vec::new());
            }
            levels[item.depth].push(remote::MenuItem {
                index: item.index,
                title: item.title,
                enabled: item.enabled,
                state: item.state,
                separator: item.separator,
                submenu: Vec::new(),
            });
        }
        while levels.len() > 1 {
            close(&mut levels);
        }
        Ok(levels.pop().unwrap_or_default())
    }
}

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::MenuChoice")]
pub(super) struct MenuChoiceDef {
    #[serde_as(as = "MenuIdDef")]
    menu: remote::MenuId,
    path: Vec<usize>,
}
remote!(MenuChoiceDef, crate::remote::MenuChoice);
remote!(NoticeDef, crate::remote::Notice);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::CursorShape")]
pub(super) enum CursorShapeDef {
    Arrow,
    IBeam,
    IBeamVertical,
    PointingHand,
    Crosshair,
    OpenHand,
    ClosedHand,
    ResizeLeft,
    ResizeRight,
    ResizeLeftRight,
    ResizeUp,
    ResizeDown,
    ResizeUpDown,
    DragLink,
    DragCopy,
    OperationNotAllowed,
    ContextualMenu,
    DisappearingItem,
    Image(#[serde_as(as = "CursorImageDef")] remote::CursorImage),
}
remote!(CursorShapeDef, crate::remote::CursorShape);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::CursorImage")]
pub(super) struct CursorImageDef {
    png: Vec<u8>,
    width: f64,
    height: f64,
    #[serde_as(as = "PointDef")]
    hot_spot: remote::Point,
}
remote!(CursorImageDef, crate::remote::CursorImage);

#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::Point")]
pub(super) struct PointDef {
    x: f64,
    y: f64,
}
remote!(PointDef, crate::remote::Point);

/// Modifiers as their bits.
pub(super) struct ModifiersDef;
impl SerializeAs<remote::Modifiers> for ModifiersDef {
    fn serialize_as<S: Serializer>(
        modifiers: &remote::Modifiers,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        modifiers.bits().serialize(serializer)
    }
}
impl<'de> DeserializeAs<'de, remote::Modifiers> for ModifiersDef {
    fn deserialize_as<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<remote::Modifiers, D::Error> {
        u32::deserialize(deserializer).map(remote::Modifiers::from_bits_retain)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::PointerKind")]
pub(super) enum PointerKindDef {
    Down,
    Up,
    Move,
}
remote!(PointerKindDef, crate::remote::PointerKind);

#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::Button")]
pub(super) enum ButtonDef {
    Left,
    Right,
    Middle,
}
remote!(ButtonDef, crate::remote::Button);

#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::WheelUnit")]
pub(super) enum WheelUnitDef {
    Line,
    Point,
}
remote!(WheelUnitDef, crate::remote::WheelUnit);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::PointerInput")]
pub(super) struct PointerInputDef {
    #[serde_as(as = "EditorIdDef")]
    editor: remote::EditorId,
    #[serde_as(as = "PointerKindDef")]
    kind: remote::PointerKind,
    #[serde_as(as = "ButtonDef")]
    button: remote::Button,
    #[serde_as(as = "PointDef")]
    at: remote::Point,
    clicks: u32,
    #[serde_as(as = "ModifiersDef")]
    modifiers: remote::Modifiers,
}
remote!(PointerInputDef, crate::remote::PointerInput);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::MotionInput")]
pub(super) struct MotionInputDef {
    #[serde_as(as = "EditorIdDef")]
    editor: remote::EditorId,
    dx: f64,
    dy: f64,
    #[serde_as(as = "ModifiersDef")]
    modifiers: remote::Modifiers,
}
remote!(MotionInputDef, crate::remote::MotionInput);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::WheelInput")]
pub(super) struct WheelInputDef {
    #[serde_as(as = "EditorIdDef")]
    editor: remote::EditorId,
    #[serde_as(as = "PointDef")]
    at: remote::Point,
    dx: f64,
    dy: f64,
    #[serde_as(as = "WheelUnitDef")]
    unit: remote::WheelUnit,
    #[serde_as(as = "ModifiersDef")]
    modifiers: remote::Modifiers,
}
remote!(WheelInputDef, crate::remote::WheelInput);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::KeyInput")]
pub(super) struct KeyInputDef {
    #[serde_as(as = "EditorIdDef")]
    editor: remote::EditorId,
    down: bool,
    key_code: u16,
    characters: String,
    characters_ignoring_modifiers: String,
    repeat: bool,
    #[serde_as(as = "ModifiersDef")]
    modifiers: remote::Modifiers,
}
remote!(KeyInputDef, crate::remote::KeyInput);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::TextInput")]
pub(super) struct TextInputDef {
    #[serde_as(as = "EditorIdDef")]
    editor: remote::EditorId,
    text: String,
}
remote!(TextInputDef, crate::remote::TextInput);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::ModifiersInput")]
pub(super) struct ModifiersInputDef {
    #[serde_as(as = "EditorIdDef")]
    editor: remote::EditorId,
    #[serde_as(as = "ModifiersDef")]
    modifiers: remote::Modifiers,
}
remote!(ModifiersInputDef, crate::remote::ModifiersInput);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::Command")]
pub(super) enum CommandDef {
    Pointer(#[serde_as(as = "PointerInputDef")] remote::PointerInput),
    Motion(#[serde_as(as = "MotionInputDef")] remote::MotionInput),
    Wheel(#[serde_as(as = "WheelInputDef")] remote::WheelInput),
    Key(#[serde_as(as = "KeyInputDef")] remote::KeyInput),
    Text(#[serde_as(as = "TextInputDef")] remote::TextInput),
    Modifiers(#[serde_as(as = "ModifiersInputDef")] remote::ModifiersInput),
    ReleaseAll(#[serde_as(as = "EditorIdDef")] remote::EditorId),
    ChooseMenuItem(#[serde_as(as = "MenuChoiceDef")] remote::MenuChoice),
    CloseMenu(#[serde_as(as = "MenuIdDef")] remote::MenuId),
    AnswerPanel(#[serde_as(as = "PanelAnswerDef")] remote::PanelAnswer),
    MoveWindow(#[serde_as(as = "WindowMoveDef")] remote::WindowMove),
    CloseWindow(#[serde_as(as = "WindowCloseDef")] remote::WindowClose),
}
remote!(CommandDef, crate::remote::Command);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::WindowMove")]
pub(super) struct WindowMoveDef {
    #[serde_as(as = "EditorIdDef")]
    editor: remote::EditorId,
    window: u64,
    #[serde_as(as = "PointDef")]
    to: remote::Point,
}
remote!(WindowMoveDef, crate::remote::WindowMove);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::remote::WindowClose")]
pub(super) struct WindowCloseDef {
    #[serde_as(as = "EditorIdDef")]
    editor: remote::EditorId,
    window: u64,
}
remote!(WindowCloseDef, crate::remote::WindowClose);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::Support")]
pub(super) enum SupportDef {
    Unknown,
    Unsupported,
    Supported,
}
remote!(SupportDef, crate::Support);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::ChangeBatch")]
pub(super) struct ChangeBatchDef {
    #[serde_as(as = "Vec<SlotChangeDef>")]
    changes: Vec<SlotChange>,
    resync_required: bool,
    #[serde_as(as = "Vec<Option<PluginTimingDef>>")]
    snapshot: Vec<Option<PluginTiming>>,
}
remote!(ChangeBatchDef, crate::ChangeBatch);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::PluginTiming")]
pub(super) struct PluginTimingDef {
    latency: u32,
    #[serde_as(as = "TailDef")]
    tail: Tail,
    restart_required: bool,
    latency_changed: bool,
}
remote!(PluginTimingDef, crate::PluginTiming);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::SlotChange")]
pub(super) struct SlotChangeDef {
    slot: usize,
    #[serde_as(as = "PluginTimingDef")]
    timing: PluginTiming,
}
remote!(SlotChangeDef, crate::SlotChange);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::Layout")]
pub(super) enum LayoutDef {
    None,
    Mono,
    Stereo,
    Surround51,
    Surround71,
    Lcr,
    Quad,
    Surround50,
    Surround70,
    Surround512,
    Surround514,
    Surround712,
    Surround714,
    Surround916,
    Ambisonics1,
    Ambisonics2,
    Ambisonics3,
    Ambisonics4,
}
remote!(LayoutDef, crate::Layout);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::ProcessMode")]
pub(super) enum ProcessModeDef {
    Offline,
    Realtime,
}
remote!(ProcessModeDef, crate::ProcessMode);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::SampleFormat")]
pub(super) enum SampleFormatDef {
    F32,
    F64,
}
remote!(SampleFormatDef, crate::SampleFormat);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::Diagnostic")]
pub(super) struct DiagnosticDef {
    #[serde_as(as = "DiagnosticSeverityDef")]
    severity: DiagnosticSeverity,
    message: String,
    plugin: Option<PluginId>,
    slot: Option<usize>,
    #[serde_as(as = "Option<FailureKindDef>")]
    failure_kind: Option<FailureKind>,
    truncated: bool,
}
remote!(DiagnosticDef, crate::Diagnostic);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::DiagnosticBatch")]
pub(super) struct DiagnosticBatchDef {
    #[serde_as(as = "Vec<DiagnosticDef>")]
    records: Vec<Diagnostic>,
    dropped: u64,
}
remote!(DiagnosticBatchDef, crate::DiagnosticBatch);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::DiagnosticSeverity")]
pub(super) enum DiagnosticSeverityDef {
    Debug,
    Info,
    Warning,
    Error,
    Fatal,
    HostMisbehaving,
    PluginMisbehaving,
}
remote!(DiagnosticSeverityDef, crate::DiagnosticSeverity);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::Failure")]
pub(super) struct FailureDef {
    #[serde_as(as = "FailureKindDef")]
    kind: FailureKind,
    message: String,
}
remote!(FailureDef, crate::Failure);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::FailureKind")]
pub(super) enum FailureKindDef {
    InvalidInput,
    Unsupported,
    NotFound,
    Load,
    Configuration,
    NotPrepared,
    Closed,
    RestartRequired,
    EditorOpen,
    StateMismatch,
    State,
    Processing,
    Editor,
    Host,
    Protocol,
    Crashed,
    TimedOut,
    Cancelled,
}
remote!(FailureKindDef, crate::FailureKind);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::InputError")]
pub(super) enum InputErrorDef {
    AudioSlotCount,
    AudioBusCount,
    AudioChannelCount,
    DuplicateAudioBus {
        id: u64,
    },
    AudioBusLayout {
        id: u64,
    },
    AudioRoute {
        slot: usize,
        bus: u64,
    },
    ChannelAdaptation {
        #[serde_as(as = "LayoutDef")]
        source: crate::Layout,
        #[serde_as(as = "LayoutDef")]
        target: crate::Layout,
    },
    AudioBuffers,
    AudioSampleFormat,
    StateSize,
    StateBytes,
    ParameterChoicePage,
    PresetSize,
    Transport,
    Automation,
    NotAutomatable {
        id: u64,
    },
    HostIdentity,
    PresetDiscoveryTarget,
    EmptyChain,
    SampleRate,
    BlockSize,
    OutputCount,
    MainBus,
    Slot,
    ParameterValue,
    ParameterText,
    PlainParameterValue,
    UnknownParameter {
        id: u64,
    },
    ReadOnlyParameter {
        id: u64,
    },
    EventCapacity,
    EventPortCount,
    DuplicateEventPort {
        id: u64,
    },
    EventRoute {
        slot: usize,
        port: u64,
    },
    EventPort {
        port: usize,
    },
    Events,
    PathEncoding,
    RealtimeQuantum,
    RealtimeLatency,
    RealtimeDry,
    RemoteRegion,
    RemoteEditor,
}
remote!(InputErrorDef, crate::InputError);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::EventInputRoute")]
pub(super) struct EventInputRouteDef {
    port: u64,
    #[serde_as(as = "EventSourceDef")]
    source: EventSource,
}
remote!(EventInputRouteDef, crate::EventInputRoute);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::EventPortInfo")]
pub(super) struct EventPortInfoDef {
    id: u64,
    index: u32,
    name: String,
    #[serde_as(as = "AudioDirectionDef")]
    direction: AudioDirection,
    #[serde_as(as = "SupportDef")]
    midi: Support,
    #[serde_as(as = "SupportDef")]
    sysex: Support,
    #[serde_as(as = "SupportDef")]
    note_expression: Support,
    #[serde_as(as = "SupportDef")]
    mpe: Support,
}
remote!(EventPortInfoDef, crate::EventPortInfo);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::EventSource")]
pub(super) enum EventSourceDef {
    External { port: usize },
    Previous { port: u64 },
}
remote!(EventSourceDef, crate::EventSource);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::SlotEventConfig")]
pub(super) struct SlotEventConfigDef {
    #[serde_as(as = "Vec<EventInputRouteDef>")]
    inputs: Vec<EventInputRoute>,
    outputs: Vec<u64>,
}
remote!(SlotEventConfigDef, crate::SlotEventConfig);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::HostIdentity")]
pub(super) struct HostIdentityDef {
    name: String,
    vendor: String,
    version: String,
}
remote!(HostIdentityDef, crate::HostIdentity);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::ParameterChoice")]
pub(super) struct ParameterChoiceDef {
    index: u64,
    normalized: f64,
    plain: f64,
    text: String,
}
remote!(ParameterChoiceDef, crate::ParameterChoice);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::ParameterChoicePage")]
pub(super) struct ParameterChoicePageDef {
    total: u64,
    start: u64,
    #[serde_as(as = "Vec<ParameterChoiceDef>")]
    choices: Vec<ParameterChoice>,
}
remote!(ParameterChoicePageDef, crate::ParameterChoicePage);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::ParameterDetails")]
pub(super) struct ParameterDetailsDef {
    #[serde_as(as = "ParameterInfoDef")]
    info: ParameterInfo,
    native_unit: Option<u32>,
    plain_at_zero: f64,
    plain_at_one: f64,
    #[serde_as(as = "Option<Vec<ParameterGroupDef>>")]
    groups: Option<Vec<ParameterGroup>>,
}
remote!(ParameterDetailsDef, crate::ParameterDetails);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::ParameterFlags")]
pub(super) struct ParameterFlagsDef {
    discrete: bool,
    automatable: bool,
    read_only: bool,
    hidden: bool,
    bypass: bool,
    list: bool,
    program_change: bool,
}
remote!(ParameterFlagsDef, crate::ParameterFlags);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::ParameterGroup")]
pub(super) struct ParameterGroupDef {
    id: String,
    name: String,
    program_list_id: Option<i32>,
}
remote!(ParameterGroupDef, crate::ParameterGroup);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::ParameterInfo")]
pub(super) struct ParameterInfoDef {
    id: u64,
    title: String,
    short_title: String,
    units: String,
    step_count: u32,
    default_value: Option<f64>,
    #[serde_as(as = "ParameterFlagsDef")]
    flags: ParameterFlags,
}
remote!(ParameterInfoDef, crate::ParameterInfo);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::ParameterEvent")]
pub(super) enum ParameterEventDef {
    Value { id: u64, normalized: f64 },
    BeginEdit { id: u64 },
    EndEdit { id: u64 },
    MetadataChanged,
    ValuesChanged,
    Dirty { dirty: bool },
}
remote!(ParameterEventDef, crate::ParameterEvent);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::ParameterEventBatch")]
pub(super) struct ParameterEventBatchDef {
    #[serde_as(as = "Vec<ParameterEventDef>")]
    events: Vec<ParameterEvent>,
    resync_required: bool,
    dropped: u64,
}
remote!(ParameterEventBatchDef, crate::ParameterEventBatch);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::PluginInfo")]
pub(super) struct PluginInfoDef {
    id: PluginId,
    name: String,
    vendor: String,
    version: String,
    sdk_version: String,
    #[serde_as(as = "PluginKindDef")]
    kind: PluginKind,
    categories: Vec<String>,
}
remote!(PluginInfoDef, crate::PluginInfo);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::PluginKind")]
pub(super) enum PluginKindDef {
    Effect,
    Instrument,
}
remote!(PluginKindDef, crate::PluginKind);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::PluginRef")]
pub(super) struct PluginRefDef {
    id: PluginId,
    bundle: Option<PathBuf>,
}
remote!(PluginRefDef, crate::PluginRef);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::DiscoveredPreset")]
pub(super) struct DiscoveredPresetDef {
    #[serde_as(as = "PresetLocationDef")]
    location: PresetLocation,
    name: Option<String>,
    load_key: Option<String>,
    #[serde_as(as = "Vec<PresetPluginIdDef>")]
    plugin_ids: Vec<PresetPluginId>,
    soundpack_id: Option<String>,
    #[serde_as(as = "Option<PresetFlagsDef>")]
    flags: Option<PresetFlags>,
    creators: Vec<String>,
    description: Option<String>,
    creation_time: Option<u64>,
    modification_time: Option<u64>,
    features: Vec<String>,
    extra_info: Vec<(String, String)>,
}
remote!(DiscoveredPresetDef, crate::DiscoveredPreset);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::PresetDiscovery")]
pub(super) struct PresetDiscoveryDef {
    #[serde_as(as = "Vec<DiscoveredPresetDef>")]
    presets: Vec<DiscoveredPreset>,
    #[serde_as(as = "Vec<PresetFileFailureDef>")]
    failed_files: Vec<PresetFileFailure>,
}
remote!(PresetDiscoveryDef, crate::PresetDiscovery);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::PresetDiscoveryTarget")]
pub(super) struct PresetDiscoveryTargetDef {
    provider_id: String,
    #[serde_as(as = "PresetLocationDef")]
    location: PresetLocation,
}
remote!(PresetDiscoveryTargetDef, crate::PresetDiscoveryTarget);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::PresetFileFailure")]
pub(super) struct PresetFileFailureDef {
    path: PathBuf,
    #[serde_as(as = "FailureDef")]
    failure: crate::Failure,
}
remote!(PresetFileFailureDef, crate::PresetFileFailure);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::PresetFileType")]
pub(super) struct PresetFileTypeDef {
    name: String,
    description: Option<String>,
    extension: Option<String>,
}
remote!(PresetFileTypeDef, crate::PresetFileType);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::PresetLocation")]
pub(super) enum PresetLocationDef {
    Plugin,
    File { path: PathBuf },
}
remote!(PresetLocationDef, crate::PresetLocation);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::PresetLocationInfo")]
pub(super) struct PresetLocationInfoDef {
    name: String,
    #[serde_as(as = "PresetFlagsDef")]
    flags: PresetFlags,
    #[serde_as(as = "PresetLocationDef")]
    location: PresetLocation,
}
remote!(PresetLocationInfoDef, crate::PresetLocationInfo);

/// Preset flags as their bits, those CLAP defines later included.
pub(super) struct PresetFlagsDef;
impl SerializeAs<PresetFlags> for PresetFlagsDef {
    fn serialize_as<S: Serializer>(flags: &PresetFlags, serializer: S) -> Result<S::Ok, S::Error> {
        flags.bits().serialize(serializer)
    }
}
impl<'de> DeserializeAs<'de, PresetFlags> for PresetFlagsDef {
    fn deserialize_as<D: Deserializer<'de>>(deserializer: D) -> Result<PresetFlags, D::Error> {
        u32::deserialize(deserializer).map(PresetFlags::from_bits_retain)
    }
}

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::PresetPluginId")]
pub(super) struct PresetPluginIdDef {
    abi: String,
    id: String,
}
remote!(PresetPluginIdDef, crate::PresetPluginId);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::PresetProviderInfo")]
pub(super) struct PresetProviderInfoDef {
    id: String,
    name: String,
    vendor: Option<String>,
    #[serde_as(as = "Vec<PresetFileTypeDef>")]
    file_types: Vec<PresetFileType>,
    #[serde_as(as = "Vec<PresetLocationInfoDef>")]
    locations: Vec<PresetLocationInfo>,
    #[serde_as(as = "Vec<PresetSoundpackDef>")]
    soundpacks: Vec<PresetSoundpack>,
}
remote!(PresetProviderInfoDef, crate::PresetProviderInfo);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::PresetSoundpack")]
pub(super) struct PresetSoundpackDef {
    id: String,
    name: String,
    #[serde_as(as = "PresetFlagsDef")]
    flags: PresetFlags,
    description: Option<String>,
    homepage_url: Option<String>,
    vendor: Option<String>,
    image_path: Option<String>,
    release_timestamp: Option<u64>,
}
remote!(PresetSoundpackDef, crate::PresetSoundpack);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::FactoryPreset")]
pub(super) struct FactoryPresetDef {
    #[serde_as(as = "FactoryPresetIdDef")]
    id: FactoryPresetId,
    name: String,
    group: Option<String>,
}
remote!(FactoryPresetDef, crate::FactoryPreset);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::FactoryPresetId")]
pub(super) enum FactoryPresetIdDef {
    Vst3 {
        unit_id: i32,
        list_id: i32,
        program_index: i32,
    },
    AudioUnit {
        number: i64,
    },
}
remote!(FactoryPresetIdDef, crate::FactoryPresetId);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::render::Tail")]
pub(super) enum TailDef {
    Samples(u32),
    Infinite,
}
remote!(TailDef, crate::render::Tail);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::PresetInfo")]
pub(super) struct PresetInfoDef {
    class_id: String,
    #[serde_as(as = "PresetMetadataDef")]
    metadata: PresetMetadata,
}
remote!(PresetInfoDef, crate::PresetInfo);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::PresetMetadata")]
pub(super) enum PresetMetadataDef {
    Vst3 { info: Vec<u8> },
    AudioUnit { name: Option<String> },
}
remote!(PresetMetadataDef, crate::PresetMetadata);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::StatePurpose")]
pub(super) enum StatePurposeDef {
    Project,
    Preset,
    Duplicate,
}
remote!(StatePurposeDef, crate::StatePurpose);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::BarPosition")]
pub(super) struct BarPositionDef {
    start: f64,
    number: i32,
}
remote!(BarPositionDef, crate::BarPosition);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::BlockContext")]
pub(super) struct BlockContextDef {
    frames: usize,
    #[serde_as(as = "Option<TransportDef>")]
    transport: Option<Transport>,
}
remote!(BlockContextDef, crate::BlockContext);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::LoopRegion")]
pub(super) struct LoopRegionDef {
    start: f64,
    end: f64,
    sample_start: i64,
    sample_end: i64,
}
remote!(LoopRegionDef, crate::LoopRegion);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::TimeSignature")]
pub(super) struct TimeSignatureDef {
    numerator: u16,
    denominator: u16,
}
remote!(TimeSignatureDef, crate::TimeSignature);

#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::Transport")]
pub(super) struct TransportDef {
    sample_position: i64,
    beat_position: Option<f64>,
    #[serde_as(as = "Option<BarPositionDef>")]
    bar_position: Option<BarPosition>,
    tempo: Option<f64>,
    #[serde_as(as = "Option<TimeSignatureDef>")]
    time_signature: Option<TimeSignature>,
    playing: bool,
    #[serde_as(as = "Option<LoopRegionDef>")]
    loop_region: Option<LoopRegion>,
}
remote!(TransportDef, crate::Transport);

#[cfg(test)]
mod tests {
    use super::super::{MAX_MENU_DEPTH, NoticeMessage};
    use crate::remote::{MenuId, MenuItem, MenuItemState, Notice, Rect};

    fn item(index: usize, submenu: Vec<MenuItem>) -> MenuItem {
        MenuItem::new(index, "item", true, MenuItemState::Off, false, submenu)
    }

    fn nested(depth: usize) -> Vec<MenuItem> {
        (0..depth).fold(vec![item(0, Vec::new())], |items, _| vec![item(0, items)])
    }

    fn round_trip(notice: Notice) -> Result<Notice, postcard::Error> {
        let bytes = postcard::to_allocvec(&NoticeMessage(notice)).unwrap();
        postcard::from_bytes::<NoticeMessage>(&bytes).map(|message| message.0)
    }

    #[test]
    fn a_menu_tree_round_trips() {
        let items = vec![
            item(
                0,
                vec![item(0, Vec::new()), item(2, vec![item(1, Vec::new())])],
            ),
            item(1, Vec::new()),
            item(3, nested(MAX_MENU_DEPTH - 1)),
        ];
        let notice =
            Notice::menu_opened(MenuId(4), items, Some(Rect::new(10.0, 20.0, 160.0, 90.0)));
        assert_eq!(round_trip(notice.clone()).unwrap(), notice);
    }

    #[test]
    fn a_menu_nested_past_the_limit_is_rejected() {
        let notice = Notice::menu_opened(MenuId(4), nested(MAX_MENU_DEPTH + 1), None);
        assert!(round_trip(notice).is_err());
    }
}
