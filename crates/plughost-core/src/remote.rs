//! Remote editors: editors a remote-mode helper shows in a region of a display that the
//! application reads, for someone who is not in front of the machine. The application owns the
//! display, reading the region, and sending what it reads.

use std::path::PathBuf;

/// How far in from a region's top-left corner a remote editor opens, in points.
pub const EDITOR_INSET: f64 = 20.0;

/// A rectangle in points, from the top-left corner of what it is in, down and to the right.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Rect {
    /// Points from the left edge.
    pub x: f64,
    /// Points from the top edge.
    pub y: f64,
    /// Width in points.
    pub width: f64,
    /// Height in points.
    pub height: f64,
}

impl Rect {
    /// A rectangle at `(x, y)` of the given size.
    #[must_use]
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// Whether `other` lies entirely within this rectangle.
    #[must_use]
    pub fn contains(&self, other: &Rect) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.x + other.width <= self.x + self.width
            && other.y + other.height <= self.y + self.height
    }
}

/// Where remote editors show: a rectangle of a display, in points from the display's top-left
/// corner. It must have a finite position, a positive size, and lie within the display.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Region {
    /// The display's identifier (a `CGDirectDisplayID` on macOS).
    pub display: u32,
    /// The region within the display.
    pub rect: Rect,
}

impl Region {
    /// The region `rect` of `display`.
    #[must_use]
    pub fn new(display: u32, rect: Rect) -> Region {
        Region { display, rect }
    }

    /// Whether the rectangle has a finite position and a positive, finite size. Whether it lies
    /// within its display is checked where the display is known.
    #[must_use]
    pub fn is_well_formed(&self) -> bool {
        let Rect {
            x,
            y,
            width,
            height,
        } = self.rect;
        x.is_finite()
            && y.is_finite()
            && width.is_finite()
            && height.is_finite()
            && width > 0.0
            && height > 0.0
    }
}

/// An open remote editor, which stays the same until it closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EditorId(pub u64);

/// A remote editor that opened: which one, and its size in points.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Opened {
    /// The editor, which commands and notices name.
    pub editor: EditorId,
    /// The editor's width in points.
    pub width: f64,
    /// The editor's height in points.
    pub height: f64,
}

impl Opened {
    /// The editor `editor` of the given size Internal to the plughost crates; exempt from semver.
    #[doc(hidden)]
    #[must_use]
    pub fn new(editor: EditorId, width: f64, height: f64) -> Opened {
        Opened {
            editor,
            width,
            height,
        }
    }
}

/// What a window the helper shows is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WindowKind {
    /// The remote editor's own window.
    Editor,
    /// An AppKit menu.
    Menu,
    /// A window with a title bar the plugin opened, such as a dialog.
    Dialog,
    /// A borderless window the plugin opened, such as its own menu or a popup.
    Popup,
}

/// A window the helper shows for the remote editor: its own, or one its plugin opened.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Window {
    /// The window's number, which stays the same while the window exists.
    pub id: u64,
    /// What the window is.
    pub kind: WindowKind,
    /// The window's frame, in points from the region's top-left corner.
    pub frame: Rect,
    /// Whether part of the window lies outside the region, where the application does not read
    /// it and may show another session.
    pub outside: bool,
}

impl Window {
    /// The window `id` of `kind` at `frame`, reaching out of the region when `outside` is set Internal to the plughost crates; exempt from semver.
    #[doc(hidden)]
    #[must_use]
    pub fn new(id: u64, kind: WindowKind, frame: Rect, outside: bool) -> Window {
        Window {
            id,
            kind,
            frame,
            outside,
        }
    }
}

/// What a remote-mode helper tells the application on its remote session channel, as it
/// happens. Match it with a wildcard arm: notices are added.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Notice {
    /// The shown remote editor's windows changed, or a session began: its own and those its
    /// plugin opened, front to back.
    #[non_exhaustive]
    Windows {
        /// The shown editor.
        editor: EditorId,
        /// Its windows, front to back.
        windows: Vec<Window>,
    },
    /// A remote editor closed: the application closed it, its plugin did, or its slot's plugin
    /// was replaced by a chain load.
    #[non_exhaustive]
    EditorClosed {
        /// The editor.
        editor: EditorId,
        /// Its slot.
        slot: usize,
    },
    /// The helper's main thread entered or left a loop nested in its own, such as a dialog's,
    /// a menu's or a file panel's, which `depth` counts. While it is not 0 the chain's requests
    /// wait for the loop to end; the helper keeps them alive while that loop runs.
    #[non_exhaustive]
    Nested {
        /// How many loops are nested in the helper's own.
        depth: u32,
    },
    /// The cursor the plugin set over the shown editor changed, or a session began. The
    /// application draws it, and locks the pointer while it is hidden (send `Motion` then).
    #[non_exhaustive]
    Cursor {
        /// Its shape.
        shape: CursorShape,
        /// The plugin hid it, or set a cursor with nothing to see.
        hidden: bool,
    },
    /// An AppKit menu the plugin opened is tracking, or a session began while it was. Input
    /// does not choose its items: draw them over the menu's window (a `Windows` window of kind
    /// `Menu`) and send `ChooseMenuItem` or `CloseMenu`. While it is open the main thread is in
    /// its loop.
    #[non_exhaustive]
    MenuOpened {
        /// The menu.
        menu: MenuId,
        /// Its items, hidden ones left out.
        items: Vec<MenuItem>,
        /// The frame of the menu's own window in region points, behind the submenus AppKit may
        /// open in front of it at once; `None` when the window did not show within half a second
        /// of the menu beginning to track.
        frame: Option<Rect>,
    },
    /// The menu closed: an item was chosen, it was closed, its session ended, or it stayed open
    /// past the chain's menu timeout.
    #[non_exhaustive]
    MenuClosed {
        /// The menu.
        menu: MenuId,
    },
    /// The plugin asked for an open or save panel, or a session began while one waited. No
    /// panel shows on the machine: pick the files for it and send `AnswerPanel`. While a panel the
    /// plugin runs modally waits, input to the editor is dropped, as the real panel would.
    #[non_exhaustive]
    PanelRequested {
        /// The panel.
        panel: PanelId,
        /// What it asks for.
        request: PanelRequest,
    },
    /// The panel's call returned to the plugin with the answer. That is no promise the plugin
    /// finished writing or reading the files.
    #[non_exhaustive]
    PanelFinished {
        /// The panel.
        panel: PanelId,
    },
    /// The plugin asked to open a URL, a web page or a file such as its manual. Nothing opens
    /// on the machine: open it for the user, or not.
    #[non_exhaustive]
    OpenUrl {
        /// The URL as the plugin gave it.
        url: String,
    },
    /// The plugin asked to show files in the Finder. Nothing shows on the machine.
    #[non_exhaustive]
    RevealFiles {
        /// The files.
        paths: Vec<PathBuf>,
    },
    /// The region's display went away. Remote editors stay open but are not on a display the
    /// application reads.
    DisplayLost,
}

/// An open or save panel a plugin asked for, unique within its helper.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PanelId(pub u64);

/// What a panel is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PanelKind {
    /// It opens files.
    Open,
    /// It saves a file.
    Save,
}

/// The answer to a panel: the paths the plugin gets, or `None` to cancel. A save takes one
/// path, an open one or, if it takes several, more; an answer that does not fit is not given.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PanelAnswer {
    /// The panel.
    pub panel: PanelId,
    /// The paths, or `None` to cancel.
    pub paths: Option<Vec<PathBuf>>,
}

impl PanelAnswer {
    /// The answer `paths` to `panel`.
    #[must_use]
    pub fn new(panel: PanelId, paths: Option<Vec<PathBuf>>) -> PanelAnswer {
        PanelAnswer { panel, paths }
    }
}

/// What a panel asked for, as [`Notice::PanelRequested`] tells it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct PanelRequest {
    /// Whether it opens files or saves one.
    pub kind: PanelKind,
    /// The folder the plugin starts it in, if it names one.
    pub directory: Option<PathBuf>,
    /// The file name it proposes, which may be empty.
    pub name: String,
    /// The file name extensions it allows, none for any.
    pub types: Vec<String>,
    /// Whether it takes several files.
    pub multiple: bool,
    /// Whether it takes folders.
    pub directories: bool,
    /// Whether it takes files; a save panel does.
    pub files: bool,
    /// Whether it takes a bundle, such as a `.vst3` or a `.component`, as one file, as AppKit
    /// does unless the plugin asks to look into bundles as folders: list bundles among the files
    /// it may take.
    pub packages: bool,
    /// The message it shows, which may be empty.
    pub message: String,
}

impl PanelRequest {
    /// A request taking one file, no folder and bundles as files; see the fields. Internal to
    /// the plughost crates; exempt from semver.
    #[doc(hidden)]
    #[must_use]
    pub fn new(
        kind: PanelKind,
        directory: Option<PathBuf>,
        name: impl Into<String>,
        types: Vec<String>,
        message: impl Into<String>,
    ) -> PanelRequest {
        PanelRequest {
            kind,
            directory,
            name: name.into(),
            types,
            multiple: false,
            directories: false,
            files: true,
            packages: true,
            message: message.into(),
        }
    }
}

/// An open menu, unique within its helper.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MenuId(pub u64);

/// Whether a menu item shows a check mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MenuItemState {
    /// No mark.
    Off,
    /// A check mark.
    On,
    /// A dash, for a mixed state.
    Mixed,
}

/// An item of an open menu.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct MenuItem {
    /// Its place among all its menu's items, hidden ones included: a [`MenuChoice`] path names
    /// items by it, as titles may be empty or repeat.
    pub index: usize,
    /// Its title, or the text of its attributed title.
    pub title: String,
    /// Whether it can be chosen.
    pub enabled: bool,
    /// Its mark.
    pub state: MenuItemState,
    /// Whether it is a separator line.
    pub separator: bool,
    /// The items of its submenu, if it opens one.
    pub submenu: Vec<MenuItem>,
}

impl MenuItem {
    /// A menu item; see the fields Internal to the plughost crates; exempt from semver.
    #[doc(hidden)]
    #[must_use]
    pub fn new(
        index: usize,
        title: impl Into<String>,
        enabled: bool,
        state: MenuItemState,
        separator: bool,
        submenu: Vec<MenuItem>,
    ) -> MenuItem {
        MenuItem {
            index,
            title: title.into(),
            enabled,
            state,
            separator,
            submenu,
        }
    }
}

/// An item of an open menu to choose: the indexes of the submenus' items that lead to it, then
/// its own.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct MenuChoice {
    /// The menu.
    pub menu: MenuId,
    /// The item's index path.
    pub path: Vec<usize>,
}

impl MenuChoice {
    /// The item at `path` of `menu`.
    #[must_use]
    pub fn new(menu: MenuId, path: Vec<usize>) -> MenuChoice {
        MenuChoice { menu, path }
    }
}

/// A window the shown editor's plugin opened, a `Dialog` or a `Popup` as [`Notice::Windows`]
/// lists it, to move so its top-left corner is at a region point.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct WindowMove {
    /// The shown editor.
    pub editor: EditorId,
    /// The window, by [`Window::id`].
    pub window: u64,
    /// Where its top-left corner goes, in region points.
    pub to: Point,
}

impl WindowMove {
    /// Moves `window` of `editor` so its top-left corner is at `to`.
    #[must_use]
    pub fn new(editor: EditorId, window: u64, to: Point) -> WindowMove {
        WindowMove { editor, window, to }
    }
}

/// A window the shown editor's plugin opened, a `Dialog` or a `Popup` as [`Notice::Windows`]
/// lists it, to close as its close button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct WindowClose {
    /// The shown editor.
    pub editor: EditorId,
    /// The window, by [`Window::id`].
    pub window: u64,
}

impl WindowClose {
    /// Closes `window` of `editor`.
    #[must_use]
    pub fn new(editor: EditorId, window: u64) -> WindowClose {
        WindowClose { editor, window }
    }
}

/// A cursor's shape: one of AppKit's, or a plugin's own image. Match it with a wildcard arm:
/// shapes are added.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum CursorShape {
    /// The arrow.
    Arrow,
    /// The I-beam over text.
    IBeam,
    /// The I-beam over vertical text.
    IBeamVertical,
    /// The pointing hand over a link or button.
    PointingHand,
    /// The crosshair.
    Crosshair,
    /// The open hand over something to drag.
    OpenHand,
    /// The closed hand while dragging.
    ClosedHand,
    /// Resizing to the left.
    ResizeLeft,
    /// Resizing to the right.
    ResizeRight,
    /// Resizing left or right.
    ResizeLeftRight,
    /// Resizing up.
    ResizeUp,
    /// Resizing down.
    ResizeDown,
    /// Resizing up or down.
    ResizeUpDown,
    /// Dragging to make a link.
    DragLink,
    /// Dragging to copy.
    DragCopy,
    /// A drop that is not allowed.
    OperationNotAllowed,
    /// A contextual menu is available.
    ContextualMenu,
    /// Dropping makes the item disappear.
    DisappearingItem,
    /// The plugin's own image.
    Image(CursorImage),
}

/// A plugin's own cursor image.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct CursorImage {
    /// The image as PNG, at the display's pixels per point or more.
    pub png: Vec<u8>,
    /// Its width in points.
    pub width: f64,
    /// Its height in points.
    pub height: f64,
    /// The point of the image that is the pointer's position, from its top-left corner.
    pub hot_spot: Point,
}

impl CursorImage {
    /// A cursor image; see the fields Internal to the plughost crates; exempt from semver.
    #[doc(hidden)]
    #[must_use]
    pub fn new(png: Vec<u8>, width: f64, height: f64, hot_spot: Point) -> CursorImage {
        CursorImage {
            png,
            width,
            height,
            hot_spot,
        }
    }
}

/// Internal to the plughost crates; exempt from semver: the helper builds notices.
impl Notice {
    /// The shown editor's windows.
    #[doc(hidden)]
    #[must_use]
    pub fn windows(editor: EditorId, windows: Vec<Window>) -> Notice {
        Notice::Windows { editor, windows }
    }

    /// A remote editor closed.
    #[doc(hidden)]
    #[must_use]
    pub fn editor_closed(editor: EditorId, slot: usize) -> Notice {
        Notice::EditorClosed { editor, slot }
    }

    /// The main thread's loops nest `depth` deep.
    #[doc(hidden)]
    #[must_use]
    pub fn nested(depth: u32) -> Notice {
        Notice::Nested { depth }
    }

    /// The plugin's cursor.
    #[doc(hidden)]
    #[must_use]
    pub fn cursor(shape: CursorShape, hidden: bool) -> Notice {
        Notice::Cursor { shape, hidden }
    }

    /// A menu opened.
    #[doc(hidden)]
    #[must_use]
    pub fn menu_opened(menu: MenuId, items: Vec<MenuItem>, frame: Option<Rect>) -> Notice {
        Notice::MenuOpened { menu, items, frame }
    }

    /// A menu closed.
    #[doc(hidden)]
    #[must_use]
    pub fn menu_closed(menu: MenuId) -> Notice {
        Notice::MenuClosed { menu }
    }

    /// A panel waits for an answer.
    #[doc(hidden)]
    #[must_use]
    pub fn panel_requested(panel: PanelId, request: PanelRequest) -> Notice {
        Notice::PanelRequested { panel, request }
    }

    /// A panel's call returned to the plugin.
    #[doc(hidden)]
    #[must_use]
    pub fn panel_finished(panel: PanelId) -> Notice {
        Notice::PanelFinished { panel }
    }

    /// The plugin asked to open `url`.
    #[doc(hidden)]
    #[must_use]
    pub fn open_url(url: impl Into<String>) -> Notice {
        Notice::OpenUrl { url: url.into() }
    }

    /// The plugin asked to show `paths` in the Finder.
    #[doc(hidden)]
    #[must_use]
    pub fn reveal_files(paths: Vec<PathBuf>) -> Notice {
        Notice::RevealFiles { paths }
    }
}

/// A point in points from the region's top-left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Point {
    /// Points from the left edge.
    pub x: f64,
    /// Points from the top edge.
    pub y: f64,
}

impl Point {
    /// The point `(x, y)`.
    #[must_use]
    pub fn new(x: f64, y: f64) -> Point {
        Point { x, y }
    }
}

bitflags::bitflags! {
    /// The modifier keys held with an input.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct Modifiers: u32 {
        /// Shift.
        const SHIFT = 1 << 0;
        /// Control.
        const CONTROL = 1 << 1;
        /// Option (Alt).
        const OPTION = 1 << 2;
        /// Command.
        const COMMAND = 1 << 3;
        /// Caps Lock.
        const CAPS_LOCK = 1 << 4;
    }
}

/// What a pointer input does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PointerKind {
    /// A button went down.
    Down,
    /// The button that went down came up.
    Up,
    /// The pointer moved, dragging while a button is down.
    Move,
}

/// A pointer button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Button {
    /// The primary button.
    Left,
    /// The secondary button.
    Right,
    /// The middle button.
    Middle,
}

/// What a wheel input's distances count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WheelUnit {
    /// Lines, as a mouse wheel's notches.
    Line,
    /// Points, as a trackpad's precise scrolling.
    Point,
}

/// A pointer input at a point of the region, for the shown editor `editor`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct PointerInput {
    /// The shown editor the input is for; the helper drops input for another.
    pub editor: EditorId,
    /// What the pointer does.
    pub kind: PointerKind,
    /// The button that goes down or up; ignored for a move.
    pub button: Button,
    /// Where, in points from the region's top-left corner.
    pub at: Point,
    /// The click count of a down or up: 2 for the second click of a double click.
    pub clicks: u32,
    /// The modifiers held.
    pub modifiers: Modifiers,
}

impl PointerInput {
    /// A pointer input; see the fields.
    #[must_use]
    pub fn new(
        editor: EditorId,
        kind: PointerKind,
        button: Button,
        at: Point,
        clicks: u32,
        modifiers: Modifiers,
    ) -> PointerInput {
        PointerInput {
            editor,
            kind,
            button,
            at,
            clicks,
            modifiers,
        }
    }
}

/// A relative pointer move, as a browser's Pointer Lock reports while a plugin hides the cursor:
/// the pointer goes on from where it is, which includes where the plugin moved it, dragging
/// while a button is down.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct MotionInput {
    /// The shown editor the input is for.
    pub editor: EditorId,
    /// How far right, in points, after the system's pointer acceleration.
    pub dx: f64,
    /// How far down, in points, after the system's pointer acceleration.
    pub dy: f64,
    /// The modifiers held.
    pub modifiers: Modifiers,
}

impl MotionInput {
    /// A relative move; see the fields.
    #[must_use]
    pub fn new(editor: EditorId, dx: f64, dy: f64, modifiers: Modifiers) -> MotionInput {
        MotionInput {
            editor,
            dx,
            dy,
            modifiers,
        }
    }
}

/// A wheel input at a point of the region. Positive `dy` scrolls the content down (the wheel
/// turned towards the user), positive `dx` to the right.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct WheelInput {
    /// The shown editor the input is for.
    pub editor: EditorId,
    /// Where, in points from the region's top-left corner.
    pub at: Point,
    /// The horizontal distance.
    pub dx: f64,
    /// The vertical distance.
    pub dy: f64,
    /// What the distances count.
    pub unit: WheelUnit,
    /// The modifiers held.
    pub modifiers: Modifiers,
}

impl WheelInput {
    /// A wheel input; see the fields.
    #[must_use]
    pub fn new(
        editor: EditorId,
        at: Point,
        dx: f64,
        dy: f64,
        unit: WheelUnit,
        modifiers: Modifiers,
    ) -> WheelInput {
        WheelInput {
            editor,
            at,
            dx,
            dy,
            unit,
            modifiers,
        }
    }
}

/// A key going down or up. It goes to the window the last pointer down went to while that window
/// shows, or else to the shown editor's; a key goes up in the window it went down in.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct KeyInput {
    /// The shown editor the input is for.
    pub editor: EditorId,
    /// Whether the key went down; up otherwise.
    pub down: bool,
    /// The macOS virtual key code (`kVK_ANSI_A` is 0). Mapping a browser's key codes to these is
    /// the application's.
    pub key_code: u16,
    /// The characters the key produces with the modifiers held.
    pub characters: String,
    /// The characters the key produces without modifiers but Shift.
    pub characters_ignoring_modifiers: String,
    /// Whether this is a repeat of a held key.
    pub repeat: bool,
    /// The modifiers held.
    pub modifiers: Modifiers,
}

impl KeyInput {
    /// A key input; see the fields.
    #[must_use]
    pub fn new(
        editor: EditorId,
        down: bool,
        key_code: u16,
        characters: impl Into<String>,
        characters_ignoring_modifiers: impl Into<String>,
        repeat: bool,
        modifiers: Modifiers,
    ) -> KeyInput {
        KeyInput {
            editor,
            down,
            key_code,
            characters: characters.into(),
            characters_ignoring_modifiers: characters_ignoring_modifiers.into(),
            repeat,
            modifiers,
        }
    }
}

/// Text typed, such as a composed or pasted string, to the window the last pointer down went to.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct TextInput {
    /// The shown editor the input is for.
    pub editor: EditorId,
    /// The text.
    pub text: String,
}

impl TextInput {
    /// Text for `editor`.
    #[must_use]
    pub fn new(editor: EditorId, text: impl Into<String>) -> TextInput {
        TextInput {
            editor,
            text: text.into(),
        }
    }
}

/// The modifier keys held changed with no other input.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct ModifiersInput {
    /// The shown editor the input is for.
    pub editor: EditorId,
    /// The modifiers held now.
    pub modifiers: Modifiers,
}

impl ModifiersInput {
    /// The modifiers `modifiers` held for `editor`.
    #[must_use]
    pub fn new(editor: EditorId, modifiers: Modifiers) -> ModifiersInput {
        ModifiersInput { editor, modifiers }
    }
}

/// What the application tells a remote session's helper to do, in order. Commands get no answer.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Command {
    /// A pointer input.
    Pointer(PointerInput),
    /// A relative pointer move.
    Motion(MotionInput),
    /// A wheel input.
    Wheel(WheelInput),
    /// A key input.
    Key(KeyInput),
    /// Typed text.
    Text(TextInput),
    /// The modifiers held changed.
    Modifiers(ModifiersInput),
    /// Releases a held button, the keys down and the modifiers, as when the user's browser lost
    /// the focus.
    ReleaseAll(EditorId),
    /// Chooses an item of an open menu, which closes it. A disabled item, a separator, an item
    /// that opens a submenu or a path that names no item is not chosen.
    ChooseMenuItem(MenuChoice),
    /// Closes an open menu without choosing.
    CloseMenu(MenuId),
    /// Answers a waiting panel.
    AnswerPanel(PanelAnswer),
    /// Moves a window the plugin opened, as dragging its title bar does, which input cannot.
    MoveWindow(WindowMove),
    /// Closes a window the plugin opened as its close button does: the plugin may refuse, and a
    /// window without a close button stays.
    CloseWindow(WindowClose),
}
