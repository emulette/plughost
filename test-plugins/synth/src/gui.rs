//! A floating editor: a plain top-level window on Windows and macOS. Notes on keys 0, 1 and 2
//! make the plugin ask the host to hide, show and close it, as editors do on their own.

use clack_extensions::gui::{
    GuiApiType, GuiConfiguration, GuiSize, PluginGuiImpl, Window as ParentWindow,
};
use clack_plugin::prelude::*;

use crate::{MainThread, Shared};

pub const HIDE: u8 = 1;
pub const SHOW: u8 = 2;
pub const CLOSED: u8 = 3;

const SIZE: GuiSize = GuiSize {
    width: 200,
    height: 100,
};

/// Called on the main thread after the audio thread recorded a request.
pub fn serve(shared: &Shared) {
    let Some(gui) = shared.gui else {
        return;
    };
    let host = &shared.host;
    match shared
        .editor_request
        .swap(0, std::sync::atomic::Ordering::Relaxed)
    {
        HIDE => {
            let _ = gui.request_hide(host);
        }
        SHOW => {
            let _ = gui.request_show(host);
        }
        CLOSED => gui.closed(host, true),
        _ => {}
    }
}

/// The platform's window API, which the floating editor uses.
const API: GuiApiType = if cfg!(target_os = "macos") {
    GuiApiType::COCOA
} else {
    GuiApiType::WIN32
};

fn floating(configuration: &GuiConfiguration) -> bool {
    configuration.api_type == API && configuration.is_floating
}

impl PluginGuiImpl for MainThread<'_> {
    fn is_api_supported(&self, configuration: GuiConfiguration) -> bool {
        floating(&configuration)
    }

    fn get_preferred_api(&self) -> Option<GuiConfiguration<'_>> {
        let configuration = GuiConfiguration {
            api_type: API,
            is_floating: true,
        };
        floating(&configuration).then_some(configuration)
    }

    fn create(&self, configuration: GuiConfiguration) -> Result<(), PluginError> {
        if !floating(&configuration) {
            return Err(PluginError::Message(crate::errors::GUI_API));
        }
        *self.window.borrow_mut() =
            Some(window::Window::new().ok_or(PluginError::Message(crate::errors::GUI_WINDOW))?);
        Ok(())
    }

    fn destroy(&self) {
        self.window.borrow_mut().take();
    }

    fn set_scale(&self, _scale: f64) -> Result<(), PluginError> {
        Err(PluginError::Message(crate::errors::GUI_API))
    }

    fn get_size(&self) -> Option<GuiSize> {
        Some(SIZE)
    }

    fn set_size(&self, _size: GuiSize) -> Result<(), PluginError> {
        Err(PluginError::Message(crate::errors::GUI_API))
    }

    fn set_parent(&self, _window: ParentWindow) -> Result<(), PluginError> {
        Err(PluginError::Message(crate::errors::GUI_API))
    }

    fn set_transient(&self, _window: ParentWindow) -> Result<(), PluginError> {
        Ok(())
    }

    fn suggest_title(&self, title: &str) {
        if let Some(window) = &*self.window.borrow() {
            window.set_title(title);
        }
    }

    fn show(&self) -> Result<(), PluginError> {
        self.set_visible(true)
    }

    fn hide(&self) -> Result<(), PluginError> {
        self.set_visible(false)
    }
}

impl MainThread<'_> {
    fn set_visible(&self, visible: bool) -> Result<(), PluginError> {
        let window = self.window.borrow();
        let window = window
            .as_ref()
            .ok_or(PluginError::Message(crate::errors::GUI_WINDOW))?;
        window.set_visible(visible);
        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub mod window {
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SW_HIDE,
        SW_SHOWNOACTIVATE, SetWindowTextW, ShowWindow, WNDCLASSW, WS_OVERLAPPEDWINDOW,
    };
    use windows_sys::core::w;

    /// Tests find the editor by this window class.
    const CLASS: windows_sys::core::PCWSTR = w!("PlughostTestSynthEditor");

    pub struct Window(HWND);

    impl Window {
        pub fn new() -> Option<Window> {
            let instance = unsafe { GetModuleHandleW(null()) };
            let class = WNDCLASSW {
                lpfnWndProc: Some(DefWindowProcW),
                hInstance: instance,
                lpszClassName: CLASS,
                ..Default::default()
            };
            // A class registered by an earlier instance stays registered.
            unsafe { RegisterClassW(&class) };
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    CLASS,
                    w!("plughost test synth"),
                    WS_OVERLAPPEDWINDOW,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    super::SIZE.width as i32,
                    super::SIZE.height as i32,
                    null_mut(),
                    null_mut(),
                    instance,
                    null(),
                )
            };
            (!hwnd.is_null()).then_some(Window(hwnd))
        }

        pub fn set_visible(&self, visible: bool) {
            unsafe { ShowWindow(self.0, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE }) };
        }

        pub fn set_title(&self, title: &str) {
            let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
            unsafe { SetWindowTextW(self.0, title.as_ptr()) };
        }
    }

    impl Drop for Window {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.0) };
        }
    }
}

#[cfg(target_os = "macos")]
pub mod window {
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSBackingStoreType, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    /// Tests find the editor by its width, `SIZE.width`, among the helper's windows.
    pub struct Window(Retained<NSWindow>);

    impl Window {
        /// Called on the main thread, as CLAP GUI calls are.
        pub fn new() -> Option<Window> {
            let mtm = MainThreadMarker::new()?;
            let size = NSSize::new(f64::from(super::SIZE.width), f64::from(super::SIZE.height));
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    NSRect::new(NSPoint::new(100.0, 100.0), size),
                    NSWindowStyleMask::Titled,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            unsafe { window.setReleasedWhenClosed(false) };
            Some(Window(window))
        }

        pub fn set_visible(&self, visible: bool) {
            if visible {
                self.0.orderFront(None);
            } else {
                self.0.orderOut(None);
            }
        }

        pub fn set_title(&self, title: &str) {
            self.0.setTitle(&NSString::from_str(title));
        }
    }

    impl Drop for Window {
        fn drop(&mut self) {
            self.0.close();
        }
    }
}
