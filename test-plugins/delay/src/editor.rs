//! The CLAP editor of the `editor` variant, on macOS: a view drawn inside the host's window. It
//! takes keyboard focus when given it and then sends a key to its window, so the parameter
//! `clap::EDITOR_FOCUS` tells whether the host focused the view and whether keys reach it.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use clack_extensions::gui::{
    GuiApiType, GuiConfiguration, GuiSize, PluginGuiImpl, Window as ParentWindow,
};
use clack_plugin::prelude::*;
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSResponder, NSView,
};
use objc2_foundation::{NSObject, NSPoint, NSRect, NSSize, NSString};

use crate::clap::MainThread;
use crate::errors;

/// The view became the window's first responder.
pub const FOCUSED: u32 = 1;
/// A key reached the view.
pub const KEY: u32 = 2;

const SIZE: GuiSize = GuiSize {
    width: 240,
    height: 120,
};

pub struct Ivars {
    focus: Arc<AtomicU32>,
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
            self.send_key();
            true
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, _event: &NSEvent) {
            self.ivars().focus.store(KEY, Ordering::Relaxed);
        }
    }
);

impl FocusView {
    /// Queues a key press for this view's window, which the application delivers as it does a
    /// typed key: to its key window's first responder.
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
            NSApplication::sharedApplication(self.mtm()).postEvent_atStart(&event, false);
        }
    }
}

/// The open editor's view, removed from the host's window when dropped.
pub struct Editor(Retained<FocusView>);

impl Drop for Editor {
    fn drop(&mut self) {
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

    fn set_size(&self, size: GuiSize) -> Result<(), PluginError> {
        if size == SIZE {
            Ok(())
        } else {
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
    let view = FocusView::alloc(mtm).set_ivars(Ivars { focus });
    let frame = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(f64::from(SIZE.width), f64::from(SIZE.height)),
    );
    let view: Retained<FocusView> = unsafe { msg_send![super(view), initWithFrame: frame] };
    parent.addSubview(&view);
    Some(Editor(view))
}

/// Where the main thread keeps the open editor.
pub type Slot = RefCell<Option<Editor>>;
