//! The CLAP editor of the `editor` variant, on Windows: a child window inside the host's window,
//! narrower than the system lets a window be, so the host's window is wider than the editor. It
//! claims to be resizable but accepts only its own size; `clap::REFUSED_SIZES` counts the sizes it
//! refused.

use std::cell::RefCell;
use std::sync::atomic::Ordering;

use clack_extensions::gui::{
    GuiApiType, GuiConfiguration, GuiSize, PluginGuiImpl, Window as ParentWindow,
};
use clack_plugin::prelude::*;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, WS_CHILD, WS_VISIBLE,
};
use windows_sys::core::w;

use crate::clap::MainThread;
use crate::errors;

const SIZE: GuiSize = GuiSize {
    width: 40,
    height: 120,
};

/// The open editor's window, destroyed when dropped.
pub struct Editor(HWND);

impl Drop for Editor {
    fn drop(&mut self) {
        // SAFETY: the window `set_parent` created on this thread.
        unsafe { DestroyWindow(self.0) };
    }
}

fn embedded(configuration: &GuiConfiguration) -> bool {
    configuration.api_type == GuiApiType::WIN32 && !configuration.is_floating
}

impl PluginGuiImpl for MainThread<'_> {
    fn is_api_supported(&self, configuration: GuiConfiguration) -> bool {
        embedded(&configuration)
    }

    fn get_preferred_api(&self) -> Option<GuiConfiguration<'_>> {
        Some(GuiConfiguration {
            api_type: GuiApiType::WIN32,
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
            .as_win32_hwnd()
            .ok_or(PluginError::Message(errors::GUI_PARENT))?;
        // SAFETY: the host's window outlives the editor, which `destroy` removes; STATIC is a
        // system window class.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                w!("STATIC"),
                std::ptr::null(),
                WS_CHILD | WS_VISIBLE,
                0,
                0,
                SIZE.width as i32,
                SIZE.height as i32,
                parent,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err(PluginError::Message(errors::GUI_PARENT));
        }
        *self.editor().borrow_mut() = Some(Editor(hwnd));
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

/// Where the main thread keeps the open editor.
pub type Slot = RefCell<Option<Editor>>;
