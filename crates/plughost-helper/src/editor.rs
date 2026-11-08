//! Editor windows: a top-level window of the helper per open editor. They are not embedded in the
//! application's windows.

#[cfg(target_os = "macos")]
mod platform {
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::rc::Rc;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSBackingStoreType, NSWindow, NSWindowNumberListOptions, NSWindowStyleMask,
        NSWindowWillCloseNotification,
    };
    use objc2_foundation::{
        NSNotification, NSNotificationCenter, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    };
    use plughost_core::remote::{EDITOR_INSET, EditorId, Point, Rect, Region, Window, WindowKind};
    use plughost_core::{Failure, FailureKind};
    use plughost_formats::{EditorView, HostedPlugin, ResizeRequest};

    use crate::errors::{EXTENSION_EDITOR, FLOATING_REMOTE, NOT_MAIN_THREAD};

    /// AppKit's level for menus (`NSPopUpMenuWindowLevel`).
    const MENU_LEVEL: isize = 101;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGDisplayBounds(display: u32) -> NSRect;
        fn CGMainDisplayID() -> u32;
    }

    /// The height of the main display, against whose top Core Graphics measures global points
    /// down and AppKit's screen points are measured up from the bottom.
    pub fn main_height() -> f64 {
        unsafe { CGDisplayBounds(CGMainDisplayID()) }.size.height
    }

    /// Whether `region` lies within its display, which exists.
    pub fn region_fits(region: &Region) -> bool {
        let bounds = unsafe { CGDisplayBounds(region.display) };
        bounds.size.width > 0.0
            && Rect::new(0.0, 0.0, bounds.size.width, bounds.size.height).contains(&region.rect)
    }

    /// The region's top-left corner in Core Graphics global points, or `None` when no display has
    /// its identifier.
    fn region_origin(region: &Region) -> Option<NSPoint> {
        let bounds = unsafe { CGDisplayBounds(region.display) };
        (bounds.size.width > 0.0).then(|| {
            NSPoint::new(
                bounds.origin.x + region.rect.x,
                bounds.origin.y + region.rect.y,
            )
        })
    }

    /// The AppKit screen point where a remote editor's window has its top-left corner.
    fn editor_top_left(region: &Region) -> Option<NSPoint> {
        let origin = region_origin(region)?;
        Some(NSPoint::new(
            origin.x + EDITOR_INSET,
            main_height() - (origin.y + EDITOR_INSET),
        ))
    }

    /// Gives the window's content `size`; a remote editor's window keeps its top-left corner
    /// where its region puts it.
    fn set_content_size(window: &NSWindow, size: NSSize, region: Option<Region>) {
        window.setContentSize(size);
        if let Some(top_left) = region.as_ref().and_then(editor_top_left) {
            window.setFrameTopLeftPoint(top_left);
        }
    }

    /// The AppKit screen point at `at` in `region`, or `None` when no display has its identifier.
    pub fn screen_point(region: &Region, at: Point) -> Option<NSPoint> {
        let origin = region_origin(region)?;
        Some(NSPoint::new(
            origin.x + at.x,
            main_height() - (origin.y + at.y),
        ))
    }

    /// The helper's windows on screen, front to back, but for the other editors' windows in
    /// `others`.
    pub fn front_to_back(others: &[Retained<NSWindow>]) -> Vec<Retained<NSWindow>> {
        let Some(mtm) = MainThreadMarker::new() else {
            return Vec::new();
        };
        let app = NSApplication::sharedApplication(mtm);
        NSWindow::windowNumbersWithOptions(NSWindowNumberListOptions(0), mtm)
            .map(|numbers| numbers.to_vec())
            .unwrap_or_default()
            .iter()
            .filter_map(|number| app.windowWithWindowNumber(number.integerValue()))
            .filter(|window| window.isVisible() && !others.iter().any(|other| other == window))
            .collect()
    }

    /// The helper's windows on screen while the remote editor in `editor` is shown in
    /// `region`, front to back, but for the other editors' windows in `others`: the editor's and
    /// those its plugin opened.
    pub fn remote_window_list(
        editor: &NSWindow,
        region: &Region,
        others: &[Retained<NSWindow>],
    ) -> Vec<Window> {
        let Some(origin) = region_origin(region) else {
            return Vec::new();
        };
        let height = main_height();
        let bounds = Rect::new(0.0, 0.0, region.rect.width, region.rect.height);
        front_to_back(others)
            .into_iter()
            .map(|window| {
                let frame = window.frame();
                let frame = Rect::new(
                    frame.origin.x - origin.x,
                    height - (frame.origin.y + frame.size.height) - origin.y,
                    frame.size.width,
                    frame.size.height,
                );
                let kind = if &*window == editor {
                    WindowKind::Editor
                } else if window.level() >= MENU_LEVEL {
                    WindowKind::Menu
                } else if window.styleMask().contains(NSWindowStyleMask::Titled) {
                    WindowKind::Dialog
                } else {
                    WindowKind::Popup
                };
                Window::new(
                    window.windowNumber() as u64,
                    kind,
                    frame,
                    !bounds.contains(&frame),
                )
            })
            .collect()
    }

    /// The region's display, in points from the region's top-left corner, or `None` when no
    /// display has its identifier.
    pub fn display_in_region(region: &Region) -> Option<Rect> {
        let bounds = unsafe { CGDisplayBounds(region.display) };
        (bounds.size.width > 0.0).then(|| {
            Rect::new(
                -region.rect.x,
                -region.rect.y,
                bounds.size.width,
                bounds.size.height,
            )
        })
    }

    /// Moves the helper's window numbered `id` to where remote editors open in `region`.
    pub fn place_in_region(id: u64, region: &Region) {
        let (Some(mtm), Some(top_left)) = (MainThreadMarker::new(), editor_top_left(region)) else {
            return;
        };
        if let Some(window) =
            NSApplication::sharedApplication(mtm).windowWithWindowNumber(id as isize)
        {
            window.setFrameTopLeftPoint(top_left);
        }
    }

    /// A remote editor: which one it is, and the region its window is in, which the window's
    /// resizes follow when the region moves.
    struct Remote {
        id: EditorId,
        region: Rc<Cell<Region>>,
    }

    pub struct EditorWindow {
        window: Retained<NSWindow>,
        /// The editor size as last set by the plugin or accepted after a user resize.
        size: Rc<Cell<(u32, u32)>>,
        /// The plugin draws in its own window; this one stays hidden.
        floating: bool,
        /// Hidden at the plugin's request, not closed.
        hidden: bool,
        /// The window is closing: the user closed it. Hidden or minimized windows are not.
        closed: Rc<Cell<bool>>,
        /// Sets `closed` when the window closes.
        observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
        /// A remote editor's window, which never activates the helper.
        remote: Option<Remote>,
    }

    fn content_size(width: u32, height: u32) -> NSSize {
        NSSize::new(f64::from(width), f64::from(height))
    }

    fn style(resizable: bool, remote: bool) -> NSWindowStyleMask {
        if remote {
            return NSWindowStyleMask::Borderless;
        }
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable;
        if resizable {
            style | NSWindowStyleMask::Resizable
        } else {
            style
        }
    }

    impl EditorWindow {
        /// Opens the editor in a new window, centered and in front.
        pub fn open(plugin: &mut dyn HostedPlugin) -> Result<EditorWindow, Failure> {
            let mut editor = Self::new(plugin, None)?;
            editor.attach(plugin)?;
            if !editor.floating {
                editor.window.center();
                editor.bring_to_front();
            }
            Ok(editor)
        }

        /// Opens the editor as remote editor `id` in a borderless window [`EDITOR_INSET`] points
        /// in from the top-left corner of `region`, in front of the other windows there without
        /// activating the helper. An editor that draws only in its own window cannot be placed
        /// there and is closed again.
        pub fn open_remote(
            plugin: &mut dyn HostedPlugin,
            region: Region,
            id: EditorId,
        ) -> Result<EditorWindow, Failure> {
            let remote = Remote {
                id,
                region: Rc::new(Cell::new(region)),
            };
            let mut editor = Self::new(plugin, Some(remote))?;
            editor.attach(plugin)?;
            if editor.floating {
                plugin.close_editor();
                return Err(Failure::new(FailureKind::Unsupported, FLOATING_REMOTE));
            }
            editor.bring_to_front();
            Ok(editor)
        }

        /// The remote editor this window shows, if it is one.
        pub fn remote_id(&self) -> Option<EditorId> {
            self.remote.as_ref().map(|remote| remote.id)
        }

        /// The editor's size in points.
        pub fn size(&self) -> (u32, u32) {
            self.size.get()
        }

        /// Moves a remote editor's window into `region`.
        pub fn set_region(&mut self, region: Region) {
            let Some(remote) = &self.remote else {
                return;
            };
            remote.region.set(region);
            if let Some(top_left) = editor_top_left(&region) {
                self.window.setFrameTopLeftPoint(top_left);
            }
        }

        /// The window, for following a remote editor's windows from outside the host.
        pub fn native(&self) -> Retained<NSWindow> {
            self.window.clone()
        }

        fn new(
            plugin: &mut dyn HostedPlugin,
            remote: Option<Remote>,
        ) -> Result<EditorWindow, Failure> {
            let mtm = MainThreadMarker::new()
                .ok_or_else(|| Failure::new(FailureKind::Host, NOT_MAIN_THREAD))?;
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    NSRect::new(NSPoint::new(0.0, 0.0), content_size(400, 300)),
                    style(false, remote.is_some()),
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            unsafe { window.setReleasedWhenClosed(false) };
            window.setTitle(&NSString::from_str(&plugin.info().name));
            let closed = Rc::new(Cell::new(false));
            let flag = Rc::clone(&closed);
            let callback = RcBlock::new(move |_: NonNull<NSNotification>| flag.set(true));
            // SAFETY: the observation is limited to this window and removed when it is dropped;
            // the notification is posted on the main thread, which owns `closed`.
            let observer = unsafe {
                NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                    Some(NSWindowWillCloseNotification),
                    Some(&window),
                    None,
                    &callback,
                )
            };
            Ok(EditorWindow {
                window,
                size: Rc::new(Cell::new((0, 0))),
                floating: false,
                hidden: false,
                closed,
                observer,
                remote,
            })
        }

        /// Opens `plugin`'s editor in this window after [`EditorWindow::detach`]. The window
        /// keeps its place, order, and minimized or hidden state, and does not take focus; it
        /// shows again when a floating editor before hid it. A remote editor fails as
        /// [`EditorWindow::open_remote`] does. On failure the caller drops the window.
        pub fn reattach(&mut self, plugin: &mut dyn HostedPlugin) -> Result<(), Failure> {
            if self.remote.is_some() && plugin.editor_in_extension() {
                return Err(Failure::new(FailureKind::Unsupported, EXTENSION_EDITOR));
            }
            let was_floating = self.floating;
            self.attach(plugin)?;
            if self.remote.is_some() && self.floating {
                plugin.close_editor();
                return Err(Failure::new(FailureKind::Unsupported, FLOATING_REMOTE));
            }
            if self.hidden {
                plugin.set_editor_visible(false);
            } else if was_floating && !self.floating {
                self.window.orderFrontRegardless();
            }
            Ok(())
        }

        /// Closes the editor and keeps its window for the instance that replaces `plugin`.
        pub fn detach(&mut self, plugin: &mut dyn HostedPlugin) {
            plugin.close_editor();
        }

        fn attach(&mut self, plugin: &mut dyn HostedPlugin) -> Result<(), Failure> {
            let (frame_window, frame_size) = (self.window.clone(), Rc::clone(&self.size));
            let region = self.remote.as_ref().map(|remote| Rc::clone(&remote.region));
            let resize: ResizeRequest = Box::new(move |width, height| {
                let region = region.as_ref().map(|region| region.get());
                set_content_size(&frame_window, content_size(width, height), region);
                frame_size.set((width, height));
                true
            });
            let content = self
                .window
                .contentView()
                .ok_or_else(|| Failure::new(FailureKind::Host, NOT_MAIN_THREAD))?;
            // SAFETY: the content view belongs to the window, which outlives the editor: close(),
            // detach() and tick() detach the editor before the window goes away.
            let view =
                unsafe { plugin.open_editor(Retained::as_ptr(&content) as *mut c_void, resize) }
                    .map_err(|error| error.failure())?;
            let EditorView::Embedded { width, height } = view else {
                self.floating = true;
                self.window.orderOut(None);
                return Ok(());
            };
            self.floating = false;
            self.window
                .setStyleMask(style(plugin.editor_can_resize(), self.remote.is_some()));
            set_content_size(&self.window, content_size(width, height), self.region());
            self.size.set((width, height));
            Ok(())
        }

        /// Shows the editor and brings its window to the front. A floating editor is asked to
        /// show its own window.
        pub fn show(&mut self, plugin: &mut dyn HostedPlugin) {
            if self.floating || self.hidden {
                plugin.set_editor_visible(true);
            }
            self.hidden = false;
            if !self.floating {
                self.bring_to_front();
            }
        }

        /// Hides the editor without closing it.
        pub fn hide(&mut self, plugin: &mut dyn HostedPlugin) {
            if self.hidden {
                return;
            }
            self.hidden = true;
            plugin.set_editor_visible(false);
            if !self.floating {
                self.window.orderOut(None);
            }
        }

        /// The region a remote editor's window is in.
        pub fn region(&self) -> Option<Region> {
            self.remote.as_ref().map(|remote| remote.region.get())
        }

        /// Brings the window to the front; a local editor's also activates the helper and takes
        /// the keyboard.
        fn bring_to_front(&self) {
            if self.remote.is_some() {
                self.window.orderFrontRegardless();
                return;
            }
            crate::events::activate();
            self.window.makeKeyAndOrderFront(None);
        }

        /// Follows the window: detaches the editor when the user closed it (returns false), and
        /// offers the editor the size the user dragged the window to.
        pub fn tick(&mut self, plugin: &mut dyn HostedPlugin) -> bool {
            if self.closed.get() {
                plugin.close_editor();
                return false;
            }
            // A floating editor sizes its own window; a hidden one is followed once shown again.
            if self.floating || self.hidden {
                return true;
            }
            if plugin.editor_can_resize() {
                let frame = self.window.contentRectForFrameRect(self.window.frame());
                let dragged = (frame.size.width as u32, frame.size.height as u32);
                if dragged != self.size.get() {
                    // A refused size restores the last size the editor accepted.
                    let accepted = plugin
                        .resize_editor(dragged.0, dragged.1)
                        .unwrap_or(self.size.get());
                    if accepted != dragged {
                        set_content_size(
                            &self.window,
                            content_size(accepted.0, accepted.1),
                            self.region(),
                        );
                    }
                    self.size.set(accepted);
                }
            }
            true
        }

        pub fn close(self, plugin: &mut dyn HostedPlugin) {
            plugin.close_editor();
        }
    }

    impl Drop for EditorWindow {
        fn drop(&mut self) {
            unsafe {
                NSNotificationCenter::defaultCenter().removeObserver((*self.observer).as_ref())
            };
            // The editor has already detached, or never attached; the window can now close.
            self.window.close();
        }
    }
}

#[cfg(target_os = "windows")]
#[path = "editor/windows.rs"]
mod platform;

pub use platform::EditorWindow;

/// Whether `region` lies within its display, which exists. No display takes remote editors on
/// Windows.
#[cfg(target_os = "macos")]
pub use platform::{
    display_in_region, front_to_back, main_height, place_in_region, region_fits,
    remote_window_list, screen_point,
};
#[cfg(target_os = "windows")]
pub fn region_fits(_region: &plughost_core::remote::Region) -> bool {
    false
}
