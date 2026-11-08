//! The CLAP editor of the `editor` variant, on macOS: a view drawn inside the host's window. It
//! takes keyboard focus when given it and then sends a key to its window, so the parameter
//! `clap::EDITOR_FOCUS` tells whether the host focused the view and whether keys reach it. It
//! claims to be resizable but accepts only its own size, and once open it resizes its window as
//! a user dragging it would; `clap::REFUSED_SIZES` counts the sizes it refused. Set
//! `clap::EDITOR_WINDOW`, it hides or shows the helper application, closes its window, activates
//! the application as a click in a JUCE editor does, opens a popup window beside the editor,
//! runs a modal dialog for `MODAL` and ends it, or paints its window red and green in turn.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};

use clack_extensions::gui::{
    GuiApiType, GuiConfiguration, GuiSize, PluginGuiImpl, Window as ParentWindow,
};
use clack_plugin::prelude::*;
use objc2::rc::Retained;
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSColor, NSCursor, NSEvent, NSEventModifierFlags,
    NSEventType, NSResponder, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{NSArray, NSObject, NSPoint, NSRect, NSSize, NSString};

use crate::clap::MainThread;
use crate::errors;

/// The view became the window's first responder.
pub const FOCUSED: u32 = 1;
/// A key reached the view.
pub const KEY: u32 = 2;
/// A key the host sent reached the view, not the view's own.
pub const HOST_KEY: u32 = 3;

const SIZE: GuiSize = GuiSize {
    width: 240,
    height: 120,
};
/// How far the editor drags its window past `SIZE`.
const DRAG: u32 = 60;
/// Where the popup opens, in points from the editor window's top-left corner, and its size.
const POPUP: NSRect = NSRect::new(NSPoint::new(250.0, 100.0), NSSize::new(60.0, 40.0));

/// How long the editor's modal dialog runs before it ends itself.
const MODAL: f64 = 8.0;

/// Whether the application said it was active after the editor last tried to activate it.
static APPLICATION_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn application_active() -> bool {
    APPLICATION_ACTIVE.load(Ordering::Relaxed)
}

pub struct Ivars {
    focus: Arc<AtomicU32>,
    popup: RefCell<Option<Retained<NSWindow>>>,
    /// The window is painted red, not green.
    red: std::cell::Cell<bool>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    struct FocusView;

    impl FocusView {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            self.ivars().focus.fetch_max(FOCUSED, Ordering::Relaxed);
            // Dispatch after NSWindow has finished installing its first responder.
            unsafe {
                let _: () = msg_send![self, performSelector: sel!(deliverTestKey),
                    withObject: None::<&NSObject>, afterDelay: 0.0f64];
            }
            true
        }

        #[unsafe(method(deliverTestKey))]
        fn deliver_test_key(&self) {
            self.send_key();
        }

        #[unsafe(method(dragWindow))]
        fn drag_window(&self) {
            if let Some(window) = self.window() {
                window.setContentSize(NSSize::new(
                    f64::from(SIZE.width + DRAG),
                    f64::from(SIZE.height + DRAG),
                ));
            }
        }

        /// Hides the application, as Hide Others in another application does.
        #[unsafe(method(hideApplication))]
        fn hide_application(&self) {
            NSApplication::sharedApplication(self.mtm()).hide(None);
        }

        #[unsafe(method(unhideApplication))]
        fn unhide_application(&self) {
            NSApplication::sharedApplication(self.mtm()).unhideWithoutActivation();
        }

        /// Shows the application again the way that also activates it.
        #[unsafe(method(unhideAndActivate))]
        fn unhide_and_activate(&self) {
            NSApplication::sharedApplication(self.mtm()).unhide(None);
        }

        /// Asks to be activated from the frontmost application, as cooperative activation does.
        #[unsafe(method(activateFromFrontmost))]
        fn activate_from_frontmost(&self) {
            if let Some(frontmost) =
                objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication()
            {
                objc2_app_kit::NSRunningApplication::currentApplication()
                    .activateFromApplication_options(
                        &frontmost,
                        objc2_app_kit::NSApplicationActivationOptions::ActivateAllWindows,
                    );
            }
        }

        /// Activates the application, as JUCE does on a click in its editor.
        #[unsafe(method(activateApplication))]
        fn activate_application(&self) {
            let app = NSApplication::sharedApplication(self.mtm());
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            APPLICATION_ACTIVE.store(app.isActive(), Ordering::Relaxed);
        }

        /// Opens a borderless popup window beside the editor, as a plugin's own menu does.
        #[unsafe(method(openPopup))]
        fn open_popup(&self) {
            let Some(window) = self.window() else {
                return;
            };
            let frame = window.frame();
            let popup = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(self.mtm()),
                    NSRect::new(NSPoint::new(0.0, 0.0), POPUP.size),
                    NSWindowStyleMask::Borderless,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            unsafe { popup.setReleasedWhenClosed(false) };
            popup.setFrameTopLeftPoint(NSPoint::new(
                frame.origin.x + POPUP.origin.x,
                frame.origin.y + frame.size.height - POPUP.origin.y,
            ));
            popup.orderFrontRegardless();
            if let Some(old) = self.ivars().popup.replace(Some(popup)) {
                old.close();
            }
        }

        /// Runs a titled dialog modally, as a plugin's About box does, until it ends itself.
        #[unsafe(method(runModal))]
        fn run_modal(&self) {
            let dialog = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(self.mtm()),
                    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(200.0, 80.0)),
                    NSWindowStyleMask::Titled,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            unsafe { dialog.setReleasedWhenClosed(false) };
            let app = NSApplication::sharedApplication(self.mtm());
            unsafe {
                let _: () = msg_send![&app, performSelector: sel!(stopModal),
                    withObject: None::<&NSObject>, afterDelay: MODAL,
                    inModes: &*NSArray::from_retained_slice(&[NSString::from_str("NSModalPanelRunLoopMode")])];
            }
            app.runModalForWindow(&dialog);
            dialog.close();
        }

        /// Paints the window red, then green the next time, and so on.
        #[unsafe(method(paint))]
        fn paint(&self) {
            let Some(window) = self.window() else {
                return;
            };
            let red = !self.ivars().red.get();
            self.ivars().red.set(red);
            let color = if red {
                NSColor::redColor()
            } else {
                NSColor::greenColor()
            };
            window.setBackgroundColor(Some(&color));
        }

        /// Closes the window as its close button does.
        #[unsafe(method(closeWindow))]
        fn close_window(&self) {
            if let Some(window) = self.window() {
                window.performClose(None);
            }
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if SENDING_KEY.load(Ordering::Relaxed) {
                self.ivars().focus.store(KEY, Ordering::Relaxed);
                return;
            }
            self.ivars().focus.store(HOST_KEY, Ordering::Relaxed);
            let character = event
                .characters()
                .and_then(|characters| characters.to_string().chars().next());
            INPUT.keys.fetch_add(1, Ordering::Relaxed);
            if let Some(character) = character {
                INPUT.character.store(character as i64, Ordering::Relaxed);
            }
            if character == Some('m') {
                self.run_closable_modal();
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            INPUT.clicks.fetch_add(event.clickCount() as i64, Ordering::Relaxed);
            INPUT.down_x.store(point.x.round() as i64, Ordering::Relaxed);
            INPUT
                .down_y
                .store((self.frame().size.height - point.y).round() as i64, Ordering::Relaxed);
            INPUT.pressed.store(1, Ordering::Relaxed);
            // What a plugin asks outside the event, as JUCE does to tell whether a button is
            // still under the pointer.
            if let Some(window) = self.window() {
                let asked = self.convertPoint_fromView(
                    window.convertPointFromScreen(NSEvent::mouseLocation()),
                    None,
                );
                INPUT.asked_x.store(asked.x.round() as i64, Ordering::Relaxed);
                INPUT.asked_y.store(
                    (self.frame().size.height - asked.y).round() as i64,
                    Ordering::Relaxed,
                );
            }
            INPUT
                .asked_buttons
                .store(NSEvent::pressedMouseButtons() as i64, Ordering::Relaxed);
            INPUT.asked_shift.store(
                i64::from(NSEvent::modifierFlags_class().contains(NSEventModifierFlags::Shift)),
                Ordering::Relaxed,
            );
            LAST.set(point);
        }

        /// With Command, as a JUCE slider's velocity mode: the distance from the last point
        /// counts, and the cursor goes back to the view's centre.
        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            INPUT.drags.fetch_add(1, Ordering::Relaxed);
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            if !event.modifierFlags().contains(NSEventModifierFlags::Command) {
                LAST.set(point);
                return;
            }
            INPUT
                .warped_drag
                .fetch_add((LAST.get().y - point.y).round() as i64, Ordering::Relaxed);
            let size = self.frame().size;
            let centre = NSPoint::new(size.width / 2.0, size.height / 2.0);
            if let Some(window) = self.window() {
                let screen = window.convertPointToScreen(self.convertPoint_toView(centre, None));
                // SAFETY: Core Graphics answers for any display and point.
                unsafe {
                    let main = CGDisplayBounds(CGMainDisplayID()).size.height;
                    CGWarpMouseCursorPosition(objc2_foundation::NSPoint::new(
                        screen.x,
                        main - screen.y,
                    ));
                }
            }
            LAST.set(centre);
        }

        /// As FabFilter's and VSTGUI's views set their cursors.
        #[unsafe(method(cursorUpdate:))]
        fn cursor_update(&self, _event: &NSEvent) {
            NSCursor::pointingHandCursor().set();
        }

        #[unsafe(method(setIBeam))]
        fn set_i_beam(&self) {
            NSCursor::IBeamCursor().set();
        }

        #[unsafe(method(setImageCursor))]
        fn set_image_cursor(&self) {
            image_cursor(true).set();
        }

        /// As JUCE hides the cursor.
        #[unsafe(method(setBlankCursor))]
        fn set_blank_cursor(&self) {
            image_cursor(false).set();
        }

        #[unsafe(method(hideCursor))]
        fn hide_cursor(&self) {
            NSCursor::hide();
        }

        #[unsafe(method(unhideCursor))]
        fn unhide_cursor(&self) {
            NSCursor::unhide();
        }

        #[unsafe(method(hideDisplayCursor))]
        fn hide_display_cursor(&self) {
            unsafe { CGDisplayHideCursor(CGMainDisplayID()) };
        }

        #[unsafe(method(showDisplayCursor))]
        fn show_display_cursor(&self) {
            unsafe { CGDisplayShowCursor(CGMainDisplayID()) };
        }

        #[unsafe(method(hideCursorUntilMoved))]
        fn hide_cursor_until_moved(&self) {
            NSCursor::setHiddenUntilMouseMoves(true);
        }

        /// Pops up an AppKit menu at the view's corner, as a plugin's context menu does: an item,
        /// a separator, a hidden item, a submenu with a checked and a disabled item.
        #[unsafe(method(popUpMenu))]
        fn pop_up_menu(&self) {
            let mtm = self.mtm();
            let item = |title: &str, tag: isize| {
                let item = unsafe {
                    objc2_app_kit::NSMenuItem::initWithTitle_action_keyEquivalent(
                        objc2_app_kit::NSMenuItem::alloc(mtm),
                        &NSString::from_str(title),
                        Some(sel!(menuChose:)),
                        &NSString::new(),
                    )
                };
                unsafe { item.setTarget(Some(self)) };
                item.setTag(tag);
                item
            };
            let submenu = objc2_app_kit::NSMenu::initWithTitle(
                objc2_app_kit::NSMenu::alloc(mtm),
                &NSString::from_str("More"),
            );
            submenu.setAutoenablesItems(false);
            let deep = item("Deep", 3);
            deep.setState(objc2_app_kit::NSControlStateValueOn);
            submenu.addItem(&deep);
            let off = item("Off", 4);
            off.setEnabled(false);
            submenu.addItem(&off);
            let menu = objc2_app_kit::NSMenu::initWithTitle(
                objc2_app_kit::NSMenu::alloc(mtm),
                &NSString::new(),
            );
            menu.setAutoenablesItems(false);
            menu.addItem(&item("First", 1));
            menu.addItem(&objc2_app_kit::NSMenuItem::separatorItem(mtm));
            let hidden = item("Hidden", 2);
            hidden.setHidden(true);
            menu.addItem(&hidden);
            let more = item("More", 0);
            more.setSubmenu(Some(&submenu));
            menu.addItem(&more);
            menu.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::new(10.0, 10.0), Some(self));
            INPUT.menus_closed.fetch_add(1, Ordering::Relaxed);
        }

        /// Saves through a save panel run modally, writing to the path it answers.
        #[unsafe(method(runSavePanel))]
        fn run_save_panel(&self) {
            let panel = objc2_app_kit::NSSavePanel::savePanel(self.mtm());
            set_up_panel(&panel);
            panel.setNameFieldStringValue(&NSString::from_str("Untitled"));
            panel.setMessage(Some(&NSString::from_str("Save the preset")));
            let saved = panel.runModal() == 1;
            INPUT.panel_result.store(if saved { 1 } else { 2 }, Ordering::Relaxed);
            if saved && let Some(path) = panel.URL().and_then(|url| url.path()) {
                INPUT.panel_files.store(1, Ordering::Relaxed);
                let _ = std::fs::write(path.to_string(), b"saved");
            }
        }

        /// Opens files through an open panel that takes several, answered to its handler.
        #[unsafe(method(beginOpenPanel))]
        fn begin_open_panel(&self) {
            let panel = objc2_app_kit::NSOpenPanel::openPanel(self.mtm());
            set_up_panel(&panel);
            panel.setAllowsMultipleSelection(true);
            let read = panel.clone();
            let handler = block2::RcBlock::new(move |response: isize| {
                INPUT
                    .panel_result
                    .store(if response == 1 { 3 } else { 4 }, Ordering::Relaxed);
                INPUT
                    .panel_files
                    .store(read.URLs().count() as i64, Ordering::Relaxed);
            });
            panel.beginWithCompletionHandler(&handler);
        }

        /// Begins an open panel and closes it before it is answered. Its handler records 7 if it
        /// is called.
        #[unsafe(method(beginAndClosePanel))]
        fn begin_and_close_panel(&self) {
            let panel = objc2_app_kit::NSOpenPanel::openPanel(self.mtm());
            set_up_panel(&panel);
            let handler = block2::RcBlock::new(move |_response: isize| {
                INPUT.panel_result.store(7, Ordering::Relaxed);
            });
            panel.beginWithCompletionHandler(&handler);
            panel.close();
        }

        /// Saves through a save panel as a sheet on the editor's window.
        #[unsafe(method(beginSavePanelSheet))]
        fn begin_save_panel_sheet(&self) {
            let Some(window) = self.window() else {
                return;
            };
            let panel = objc2_app_kit::NSSavePanel::savePanel(self.mtm());
            set_up_panel(&panel);
            let handler = block2::RcBlock::new(move |response: isize| {
                INPUT
                    .panel_result
                    .store(if response == 1 { 5 } else { 6 }, Ordering::Relaxed);
            });
            panel.beginSheetModalForWindow_completionHandler(&window, &handler);
        }

        /// Copies to the general pasteboard and reads it back.
        #[unsafe(method(copyText))]
        fn copy_text(&self) {
            let pasteboard = objc2_app_kit::NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            let text = NSString::from_str(COPIED);
            // SAFETY: the type constant is AppKit's.
            let kind = unsafe { objc2_app_kit::NSPasteboardTypeString };
            pasteboard.setString_forType(&text, kind);
            let read = pasteboard.stringForType(kind);
            INPUT.copied.store(
                i64::from(read.is_some_and(|read| read.to_string() == COPIED)),
                Ordering::Relaxed,
            );
        }

        /// Opens a web page, as a plugin's link to its manual does.
        #[unsafe(method(openPage))]
        fn open_page(&self) {
            if let Some(url) =
                objc2_foundation::NSURL::URLWithString(&NSString::from_str(OPENED_URL))
            {
                objc2_app_kit::NSWorkspace::sharedWorkspace().openURL(&url);
            }
        }

        /// Shows a file in the Finder.
        #[unsafe(method(revealFile))]
        fn reveal_file(&self) {
            let url = objc2_foundation::NSURL::fileURLWithPath(&NSString::from_str(REVEALED));
            objc2_app_kit::NSWorkspace::sharedWorkspace()
                .activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
        }

        /// Opens a web page through Launch Services, as C++ frameworks do.
        #[unsafe(method(openThroughLaunchServices))]
        fn open_through_launch_services(&self) {
            if let Some(url) =
                objc2_foundation::NSURL::URLWithString(&NSString::from_str(LAUNCHED_URL))
            {
                // SAFETY: the URL is a CFURL, toll-free bridged; no launched URL is asked for.
                unsafe {
                    LSOpenCFURLRef(objc2::rc::Retained::as_ptr(&url).cast(), std::ptr::null_mut())
                };
            }
        }

        #[unsafe(method(menuChose:))]
        fn menu_chose(&self, sender: &objc2_app_kit::NSMenuItem) {
            INPUT.menu_choice.store(sender.tag() as i64, Ordering::Relaxed);
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            INPUT.entered.fetch_add(1, Ordering::Relaxed);
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, _event: &NSEvent) {
            INPUT.moved.fetch_add(1, Ordering::Relaxed);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            INPUT.exited.fetch_add(1, Ordering::Relaxed);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            INPUT.pressed.store(0, Ordering::Relaxed);
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            INPUT
                .wheel
                .fetch_add(event.scrollingDeltaY().round() as i64, Ordering::Relaxed);
        }
    }
);

impl FocusView {
    /// Runs a dialog modally until a click in it and then a key, as a plugin's dialog that a key
    /// opens and that takes typing does.
    fn run_closable_modal(&self) {
        let dialog = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(self.mtm()),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(200.0, 80.0)),
                NSWindowStyleMask::Titled,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { dialog.setReleasedWhenClosed(false) };
        let view = CloseView::alloc(self.mtm()).set_ivars(());
        let view: Retained<CloseView> = unsafe { msg_send![super(view), init] };
        dialog.setContentView(Some(&view));
        dialog.makeFirstResponder(Some(&view));
        DIALOG_CLICKED.store(false, Ordering::Relaxed);
        NSApplication::sharedApplication(self.mtm()).runModalForWindow(&dialog);
        dialog.close();
    }

    /// Sends a key through this view's owning window to its first responder. Another helper's
    /// activation cannot redirect the event to a process-global key window.
    fn send_key(&self) {
        let Some(window) = self.window() else {
            return;
        };
        let key = NSString::from_str("k");
        let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                NSEventType::KeyDown,
                NSPoint::new(0.0, 0.0),
                NSEventModifierFlags::empty(),
                0.0,
                window.windowNumber(),
                None,
                &key,
                &key,
                false,
                40,
            );
        if let Some(event) = event {
            SENDING_KEY.store(true, Ordering::Relaxed);
            window.sendEvent(&event);
            SENDING_KEY.store(false, Ordering::Relaxed);
        }
    }
}

/// The view is sending its own key, which is not input.
static SENDING_KEY: AtomicBool = AtomicBool::new(false);

/// The dialog was clicked since it opened.
static DIALOG_CLICKED: AtomicBool = AtomicBool::new(false);

define_class!(
    /// Ends the modal session on a key after a click.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    struct CloseView;

    impl CloseView {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            DIALOG_CLICKED.store(true, Ordering::Relaxed);
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, _event: &NSEvent) {
            if DIALOG_CLICKED.load(Ordering::Relaxed) {
                NSApplication::sharedApplication(self.mtm()).stopModal();
            }
        }
    }
);

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(display: u32) -> NSRect;
    /// Imported, as plugins that warp import it, so that a host can rebind it.
    fn CGWarpMouseCursorPosition(point: NSPoint) -> i32;
    fn CGDisplayHideCursor(display: u32) -> i32;
    fn CGDisplayShowCursor(display: u32) -> i32;
}

/// What the editor copies, opens and shows.
pub const COPIED: &str = "plughost test copy";
pub const OPENED_URL: &str = "plughost-test://open/page";
pub const LAUNCHED_URL: &str = "plughost-test://launch-services";
pub const REVEALED: &str = "/tmp/plughost-test-panels/shown.fxp";

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    /// Imported, as plugins that open URLs this way import it, so that a host can rebind it.
    fn LSOpenCFURLRef(url: *const c_void, launched: *mut *const c_void) -> i32;
}

/// The folder and type the editor's panels ask for.
pub const PANEL_FOLDER: &str = "/tmp/plughost-test-panels";

fn set_up_panel(panel: &objc2_app_kit::NSSavePanel) {
    panel.setDirectoryURL(Some(&objc2_foundation::NSURL::fileURLWithPath(
        &NSString::from_str(PANEL_FOLDER),
    )));
    #[expect(deprecated, reason = "plugins set their panels' types this way")]
    panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&[NSString::from_str(
        "fxp",
    )])));
}

/// A 16 by 16 point cursor with its hot spot 3 points right and 4 down, red or with nothing to
/// see.
fn image_cursor(red: bool) -> Retained<NSCursor> {
    const SIDE: isize = 16;
    // SAFETY: no planes asks the representation to allocate its own, 4 bytes per pixel.
    let rep = unsafe {
        objc2_app_kit::NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            objc2_app_kit::NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            SIDE,
            SIDE,
            8,
            4,
            true,
            false,
            objc2_app_kit::NSDeviceRGBColorSpace,
            SIDE * 4,
            32,
        )
    }
    .expect(crate::errors::BITMAP);
    let pixel: [u8; 4] = if red { [255, 0, 0, 255] } else { [0; 4] };
    // SAFETY: the representation holds SIDE * SIDE pixels of 4 bytes.
    let data =
        unsafe { std::slice::from_raw_parts_mut(rep.bitmapData(), (SIDE * SIDE * 4) as usize) };
    for chunk in data.chunks_mut(4) {
        chunk.copy_from_slice(&pixel);
    }
    let image = objc2_app_kit::NSImage::initWithSize(
        objc2_app_kit::NSImage::alloc(),
        NSSize::new(SIDE as f64, SIDE as f64),
    );
    image.addRepresentation(&rep);
    NSCursor::initWithImage_hotSpot(NSCursor::alloc(), &image, NSPoint::new(3.0, 4.0))
}

thread_local! {
    /// The view's last mouse down or drag point, or its centre after a warp.
    static LAST: std::cell::Cell<NSPoint> = const { std::cell::Cell::new(NSPoint::new(0.0, 0.0)) };
}

/// What the editor's view received, which the `editor` variant reports as parameters.
pub struct Input {
    /// The click counts of the mouse downs, summed.
    pub clicks: AtomicI64,
    pub drags: AtomicI64,
    /// The wheel's vertical distances, summed.
    pub wheel: AtomicI64,
    pub keys: AtomicI64,
    /// Where the last mouse down was, in points from the view's top-left corner.
    pub down_x: AtomicI64,
    pub down_y: AtomicI64,
    /// The last key's first character.
    pub character: AtomicI64,
    /// A button is down in the view.
    pub pressed: AtomicI64,
    /// The distances down of the drags with Command, summed.
    pub warped_drag: AtomicI64,
    /// Where `+[NSEvent mouseLocation]` said the mouse was at the last mouse down, in points
    /// from the view's top-left corner.
    pub asked_x: AtomicI64,
    pub asked_y: AtomicI64,
    /// What `+[NSEvent pressedMouseButtons]` said then.
    pub asked_buttons: AtomicI64,
    /// Whether `+[NSEvent modifierFlags]` said Shift was held then.
    pub asked_shift: AtomicI64,
    /// The messages the view's tracking area brought.
    pub entered: AtomicI64,
    pub moved: AtomicI64,
    pub exited: AtomicI64,
    /// The tag of the menu item chosen last.
    pub menu_choice: AtomicI64,
    /// How many of the menus it popped up closed.
    pub menus_closed: AtomicI64,
    /// How its last panel ended: 1 saved or 2 cancelled modally, 3 opened or 4 cancelled to a
    /// handler, 5 saved or 6 cancelled as a sheet.
    pub panel_result: AtomicI64,
    /// How many files its last panel answered.
    pub panel_files: AtomicI64,
    /// Whether what it copied read back.
    pub copied: AtomicI64,
}

pub static INPUT: Input = Input {
    clicks: AtomicI64::new(0),
    drags: AtomicI64::new(0),
    wheel: AtomicI64::new(0),
    keys: AtomicI64::new(0),
    down_x: AtomicI64::new(0),
    down_y: AtomicI64::new(0),
    character: AtomicI64::new(0),
    pressed: AtomicI64::new(0),
    warped_drag: AtomicI64::new(0),
    asked_x: AtomicI64::new(0),
    asked_y: AtomicI64::new(0),
    asked_buttons: AtomicI64::new(0),
    asked_shift: AtomicI64::new(0),
    entered: AtomicI64::new(0),
    moved: AtomicI64::new(0),
    exited: AtomicI64::new(0),
    menu_choice: AtomicI64::new(0),
    menus_closed: AtomicI64::new(0),
    panel_result: AtomicI64::new(0),
    panel_files: AtomicI64::new(0),
    copied: AtomicI64::new(0),
};

/// What the view received, for the parameter `index`: in the order of `Input`'s fields.
pub fn input(index: u32) -> f64 {
    let value = match index {
        0 => &INPUT.clicks,
        1 => &INPUT.drags,
        2 => &INPUT.wheel,
        3 => &INPUT.keys,
        4 => &INPUT.down_x,
        5 => &INPUT.down_y,
        6 => &INPUT.character,
        7 => &INPUT.pressed,
        8 => &INPUT.warped_drag,
        9 => &INPUT.asked_x,
        10 => &INPUT.asked_y,
        11 => &INPUT.asked_buttons,
        12 => &INPUT.asked_shift,
        13 => &INPUT.entered,
        14 => &INPUT.moved,
        15 => &INPUT.exited,
        16 => &INPUT.menu_choice,
        17 => &INPUT.menus_closed,
        18 => &INPUT.panel_result,
        19 => &INPUT.panel_files,
        _ => &INPUT.copied,
    };
    value.load(Ordering::Relaxed) as f64
}

/// The open editor's view, removed from the host's window when dropped.
pub struct Editor(Retained<FocusView>);

impl Editor {
    /// After the current plugin call: 1 hides the application, 2 shows it again, 3 closes the
    /// window, 4 activates the application, 5 opens a popup window, 6 runs a modal dialog, 7
    /// paints the window red, then green, 8 shows the application and activates it, 9 asks the
    /// frontmost application to activate it, 10 sets the I-beam cursor, 11 an image cursor, 12
    /// a cursor with nothing to see, 13 hides the cursor, 14 shows it, 15 and 16 do so through
    /// Core Graphics, 17 hides it until the mouse moves, 18 pops up a menu, 19 runs a save panel,
    /// 20 begins an open panel, 21 begins a save panel as a sheet, 22 copies text, 23 opens a web
    /// page, 24 shows a file in the Finder, 25 opens a web page through Launch Services, 26 begins
    /// an open panel and closes it, as a plugin does when its editor goes away.
    pub fn act(&self, action: u32) {
        let action = match action {
            1 => sel!(hideApplication),
            2 => sel!(unhideApplication),
            3 => sel!(closeWindow),
            4 => sel!(activateApplication),
            5 => sel!(openPopup),
            6 => sel!(runModal),
            7 => sel!(paint),
            8 => sel!(unhideAndActivate),
            9 => sel!(activateFromFrontmost),
            10 => sel!(setIBeam),
            11 => sel!(setImageCursor),
            12 => sel!(setBlankCursor),
            13 => sel!(hideCursor),
            14 => sel!(unhideCursor),
            15 => sel!(hideDisplayCursor),
            16 => sel!(showDisplayCursor),
            17 => sel!(hideCursorUntilMoved),
            18 => sel!(popUpMenu),
            19 => sel!(runSavePanel),
            20 => sel!(beginOpenPanel),
            21 => sel!(beginSavePanelSheet),
            22 => sel!(copyText),
            23 => sel!(openPage),
            24 => sel!(revealFile),
            25 => sel!(openThroughLaunchServices),
            26 => sel!(beginAndClosePanel),
            _ => return,
        };
        unsafe {
            let _: () = msg_send![&self.0, performSelector: action,
                withObject: None::<&NSObject>, afterDelay: 0.0f64];
        }
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        if let Some(popup) = self.0.ivars().popup.take() {
            popup.close();
        }
        self.0.removeFromSuperview();
    }
}

fn embedded(configuration: &GuiConfiguration) -> bool {
    configuration.api_type == GuiApiType::COCOA && !configuration.is_floating
}

impl PluginGuiImpl for MainThread<'_> {
    fn is_api_supported(&self, configuration: GuiConfiguration) -> bool {
        embedded(&configuration)
    }

    fn get_preferred_api(&self) -> Option<GuiConfiguration<'_>> {
        Some(GuiConfiguration {
            api_type: GuiApiType::COCOA,
            is_floating: false,
        })
    }

    fn create(&self, configuration: GuiConfiguration) -> Result<(), PluginError> {
        if embedded(&configuration) {
            Ok(())
        } else {
            Err(PluginError::Message(errors::GUI_API))
        }
    }

    fn destroy(&self) {
        self.editor().borrow_mut().take();
    }

    fn set_scale(&self, _scale: f64) -> Result<(), PluginError> {
        Ok(())
    }

    fn get_size(&self) -> Option<GuiSize> {
        Some(SIZE)
    }

    fn can_resize(&self) -> bool {
        true
    }

    fn set_size(&self, size: GuiSize) -> Result<(), PluginError> {
        if size == SIZE {
            Ok(())
        } else {
            self.shared.refused_sizes.fetch_add(1, Ordering::Relaxed);
            Err(PluginError::Message(errors::GUI_API))
        }
    }

    fn set_parent(&self, window: ParentWindow) -> Result<(), PluginError> {
        let parent = window
            .as_cocoa_nsview()
            .ok_or(PluginError::Message(errors::GUI_PARENT))?;
        let editor = unsafe { open(parent, self.focus()) }
            .ok_or(PluginError::Message(errors::GUI_PARENT))?;
        *self.editor().borrow_mut() = Some(editor);
        Ok(())
    }

    fn set_transient(&self, _window: ParentWindow) -> Result<(), PluginError> {
        Ok(())
    }

    fn show(&self) -> Result<(), PluginError> {
        Ok(())
    }

    fn hide(&self) -> Result<(), PluginError> {
        Ok(())
    }
}

/// Adds the editor's view to `parent`.
///
/// # Safety
///
/// `parent` must be a valid `NSView`; call on the main thread.
unsafe fn open(parent: *mut c_void, focus: Arc<AtomicU32>) -> Option<Editor> {
    let mtm = MainThreadMarker::new()?;
    let parent = unsafe { parent.cast::<NSView>().as_ref() }?;
    let view = FocusView::alloc(mtm).set_ivars(Ivars {
        focus,
        popup: RefCell::new(None),
        red: std::cell::Cell::new(false),
    });
    let frame = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(f64::from(SIZE.width), f64::from(SIZE.height)),
    );
    let view: Retained<FocusView> = unsafe { msg_send![super(view), initWithFrame: frame] };
    // As JUCE's, FabFilter's and VSTGUI's views track the mouse over them.
    let area = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            NSRect::ZERO,
            NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::MouseMoved
                | NSTrackingAreaOptions::CursorUpdate
                | NSTrackingAreaOptions::ActiveAlways
                | NSTrackingAreaOptions::InVisibleRect,
            Some(&view),
            None,
        )
    };
    view.addTrackingArea(&area);
    parent.addSubview(&view);
    // Once the host has shown its window.
    unsafe {
        let _: () = msg_send![&view, performSelector: sel!(dragWindow),
            withObject: None::<&NSObject>, afterDelay: 0.3f64];
    }
    Some(Editor(view))
}

/// Where the main thread keeps the open editor.
pub type Slot = RefCell<Option<Editor>>;
