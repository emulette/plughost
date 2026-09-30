//! Editor windows: a top-level window of the helper per open editor. They are not embedded in the
//! application's windows.

#[cfg(target_os = "macos")]
mod platform {
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::rc::Rc;

    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSBackingStoreType, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use plughost_core::{Failure, FailureKind};
    use plughost_formats::{EditorView, HostedPlugin, ResizeRequest};

    use crate::errors::NOT_MAIN_THREAD;

    pub struct EditorWindow {
        window: Retained<NSWindow>,
        /// The editor size as last set by the plugin or accepted after a user resize.
        size: Rc<Cell<(u32, u32)>>,
        /// The plugin draws in its own window; this one stays hidden.
        floating: bool,
        /// Hidden at the plugin's request, not closed.
        hidden: bool,
    }

    fn content_size(width: u32, height: u32) -> NSSize {
        NSSize::new(f64::from(width), f64::from(height))
    }

    impl EditorWindow {
        pub fn open(plugin: &mut dyn HostedPlugin) -> Result<EditorWindow, Failure> {
            let mtm = MainThreadMarker::new()
                .ok_or_else(|| Failure::new(FailureKind::Host, NOT_MAIN_THREAD))?;
            let style = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable;
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    NSRect::new(NSPoint::new(0.0, 0.0), content_size(400, 300)),
                    style,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            unsafe { window.setReleasedWhenClosed(false) };
            window.setTitle(&NSString::from_str(&plugin.info().name));

            let size = Rc::new(Cell::new((0, 0)));
            let (frame_window, frame_size) = (window.clone(), Rc::clone(&size));
            let resize: ResizeRequest = Box::new(move |width, height| {
                frame_window.setContentSize(content_size(width, height));
                frame_size.set((width, height));
                true
            });
            let content = window
                .contentView()
                .ok_or_else(|| Failure::new(FailureKind::Host, NOT_MAIN_THREAD))?;
            // SAFETY: the content view belongs to the window, which outlives the editor: close()
            // and tick() detach the editor before the window goes away.
            let view =
                unsafe { plugin.open_editor(Retained::as_ptr(&content) as *mut c_void, resize) }
                    .map_err(|error| error.failure())?;
            let mut editor = EditorWindow {
                window,
                size,
                floating: false,
                hidden: false,
            };
            let EditorView::Embedded { width, height } = view else {
                editor.floating = true;
                return Ok(editor);
            };
            editor.window.setContentSize(content_size(width, height));
            editor.size.set((width, height));
            if plugin.editor_can_resize() {
                editor
                    .window
                    .setStyleMask(style | NSWindowStyleMask::Resizable);
            }
            editor.window.center();
            editor.bring_to_front();
            Ok(editor)
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

        fn bring_to_front(&self) {
            crate::events::activate();
            self.window.makeKeyAndOrderFront(None);
        }

        /// Follows the window: detaches the editor when the user closed it (returns false), and
        /// offers the editor the size the user dragged the window to. A minimized window is not
        /// visible either, but its editor stays.
        pub fn tick(&mut self, plugin: &mut dyn HostedPlugin) -> bool {
            // A floating editor sizes its own window; a hidden one is followed once shown again.
            if self.floating || self.hidden {
                return true;
            }
            if !self.window.isVisible() && !self.window.isMiniaturized() {
                plugin.close_editor();
                return false;
            }
            if plugin.editor_can_resize() {
                let frame = self.window.contentRectForFrameRect(self.window.frame());
                let dragged = (frame.size.width as u32, frame.size.height as u32);
                if dragged != self.size.get()
                    && let Some(accepted) = plugin.resize_editor(dragged.0, dragged.1)
                {
                    if accepted != dragged {
                        self.window
                            .setContentSize(content_size(accepted.0, accepted.1));
                    }
                    self.size.set(accepted);
                }
            }
            true
        }

        pub fn close(self, plugin: &mut dyn HostedPlugin) {
            plugin.close_editor();
            self.window.close();
        }
    }
}

#[cfg(target_os = "windows")]
#[path = "editor/windows.rs"]
mod platform;

pub use platform::EditorWindow;
