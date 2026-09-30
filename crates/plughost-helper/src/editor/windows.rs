//! Native editor parents. Window messages only update state; plugin calls stay outside WndProc.

use std::cell::Cell;
use std::io;
use std::ptr::null_mut;
use std::rc::Rc;
use std::sync::OnceLock;

use plughost_core::{Failure, FailureKind};
use plughost_formats::{EditorView, HostedPlugin, ResizeRequest};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    GetDpiForWindow, SetThreadDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::w;

use crate::errors::EDITOR_WINDOW;

const CLASS: windows_sys::core::PCWSTR = w!("PlughostEditorWindow");
const STYLE: u32 = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;

struct DpiContext(DPI_AWARENESS_CONTEXT);

impl DpiContext {
    fn enter() -> Self {
        Self(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}

impl Drop for DpiContext {
    fn drop(&mut self) {
        unsafe { SetThreadDpiAwarenessContext(self.0) };
    }
}

#[derive(Default)]
struct WindowState {
    hwnd: Cell<HWND>,
    closed: Cell<bool>,
    size: Cell<(u32, u32)>,
    scale: Cell<Option<u32>>,
}

impl WindowState {
    fn resize(&self, width: u32, height: u32) -> bool {
        let _context = DpiContext::enter();
        let hwnd = self.hwnd.get();
        let (Ok(width_i32), Ok(height_i32)) = (i32::try_from(width), i32::try_from(height)) else {
            return false;
        };
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: width_i32,
            bottom: height_i32,
        };
        // SAFETY: the window belongs to this thread and outlives every plugin resize callback.
        let resized = unsafe {
            AdjustWindowRectExForDpi(
                &mut rect,
                GetWindowLongW(hwnd, GWL_STYLE) as u32,
                0,
                0,
                GetDpiForWindow(hwnd),
            ) != 0
                && SetWindowPos(
                    hwnd,
                    null_mut(),
                    0,
                    0,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                ) != 0
        };
        if resized {
            self.size.set((width, height));
        }
        resized
    }
}

pub struct EditorWindow {
    state: Rc<WindowState>,
    /// The plugin draws in its own window; this one stays hidden.
    floating: bool,
    /// Hidden at the plugin's request, not closed.
    hidden: bool,
}

impl EditorWindow {
    pub fn open(plugin: &mut dyn HostedPlugin) -> Result<Self, Failure> {
        let _context = DpiContext::enter();
        register().map_err(window_error)?;
        let state = Rc::new(WindowState::default());
        let title: Vec<u16> = plugin.info().name.encode_utf16().chain([0]).collect();
        // SAFETY: state has a stable allocation, owned until after DestroyWindow; title is read
        // during this call. WM_NCCREATE installs the state pointer before other messages use it.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                CLASS,
                title.as_ptr(),
                STYLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                400,
                300,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null_mut()),
                Rc::as_ptr(&state).cast(),
            )
        };
        if hwnd.is_null() {
            return Err(window_error(io::Error::last_os_error()));
        }
        state.hwnd.set(hwnd);
        let mut window = Self {
            state,
            floating: false,
            hidden: false,
        };
        let frame = Rc::clone(&window.state);
        let resize: ResizeRequest = Box::new(move |width, height| frame.resize(width, height));
        // SAFETY: close() and tick() detach the plugin before the native parent is destroyed.
        let view = unsafe { plugin.open_editor(hwnd, resize) }.map_err(|error| error.failure())?;
        let EditorView::Embedded { width, height } = view else {
            window.floating = true;
            return Ok(window);
        };
        if plugin.editor_can_resize() {
            unsafe {
                SetWindowLongW(
                    hwnd,
                    GWL_STYLE,
                    (STYLE | WS_THICKFRAME | WS_MAXIMIZEBOX) as i32,
                )
            };
        }
        if !window.state.resize(width, height) {
            let error = window_error(io::Error::last_os_error());
            plugin.close_editor();
            return Err(error);
        }
        window
            .state
            .scale
            .set(Some(unsafe { GetDpiForWindow(hwnd) }));
        window.update_scale(plugin);
        window.bring_to_front();
        Ok(window)
    }

    fn update_scale(&self, plugin: &mut dyn HostedPlugin) {
        if let Some(dpi) = self.state.scale.take()
            && let Some((width, height)) = plugin.set_editor_scale(f64::from(dpi) / 96.0)
        {
            self.state.resize(width, height);
        }
    }

    /// Shows the editor and brings its window to the front. A floating editor is asked to show
    /// its own window.
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
            unsafe { ShowWindow(self.state.hwnd.get(), SW_HIDE) };
        }
    }

    fn bring_to_front(&self) {
        let hwnd = self.state.hwnd.get();
        unsafe {
            ShowWindow(
                hwnd,
                if IsIconic(hwnd) != 0 {
                    SW_RESTORE
                } else {
                    SW_SHOW
                },
            );
            SetForegroundWindow(hwnd);
        }
    }

    pub fn tick(&mut self, plugin: &mut dyn HostedPlugin) -> bool {
        let _context = DpiContext::enter();
        if self.state.closed.get() {
            plugin.close_editor();
            return false;
        }
        // A floating editor sizes its own window; a hidden one is followed once shown again.
        if self.floating || self.hidden {
            return true;
        }
        self.update_scale(plugin);
        let hwnd = self.state.hwnd.get();
        if unsafe { IsIconic(hwnd) } != 0 {
            return true;
        }
        let mut rect = RECT::default();
        if unsafe { GetClientRect(hwnd, &mut rect) } != 0 {
            let dragged = (
                (rect.right - rect.left) as u32,
                (rect.bottom - rect.top) as u32,
            );
            if dragged != self.state.size.get() {
                let accepted = if plugin.editor_can_resize() {
                    plugin.resize_editor(dragged.0, dragged.1)
                } else {
                    None
                };
                // Refused user resizes must restore the last size the plugin accepted.
                let accepted = accepted.unwrap_or(self.state.size.get());
                if accepted != dragged {
                    self.state.resize(accepted.0, accepted.1);
                } else {
                    self.state.size.set(accepted);
                }
            }
        }
        true
    }

    pub fn close(self, plugin: &mut dyn HostedPlugin) {
        let _context = DpiContext::enter();
        plugin.close_editor();
    }
}

impl Drop for EditorWindow {
    fn drop(&mut self) {
        // The editor has already detached; its parent and the WndProc state can now be freed.
        unsafe { DestroyWindow(self.state.hwnd.get()) };
    }
}

fn window_error(error: io::Error) -> Failure {
    Failure::new(FailureKind::Host, format!("{EDITOR_WINDOW}: {error}"))
}

fn register() -> io::Result<()> {
    static REGISTERED: OnceLock<Result<(), i32>> = OnceLock::new();
    let result = REGISTERED.get_or_init(|| {
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: unsafe { GetModuleHandleW(null_mut()) },
            hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
            lpszClassName: CLASS,
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            Err(io::Error::last_os_error().raw_os_error().unwrap())
        } else {
            Ok(())
        }
    });
    result.map_err(io::Error::from_raw_os_error)
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: Win32 supplies these message-specific pointers. The EditorWindow retains the
    // WindowState through DestroyWindow. Only Cells are changed, including during reentrancy.
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let state = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const WindowState).as_ref();
        if let Some(state) = state {
            match message {
                WM_CLOSE => {
                    state.closed.set(true);
                    ShowWindow(hwnd, SW_HIDE);
                    return 0;
                }
                // Activating the window gives keyboard focus back to the editor inside it.
                WM_SETFOCUS => {
                    let child = GetWindow(hwnd, GW_CHILD);
                    if !child.is_null() {
                        SetFocus(child);
                        return 0;
                    }
                }
                WM_DPICHANGED => {
                    state.scale.set(Some((wparam & 0xffff) as u32));
                    let rect = &*(lparam as *const RECT);
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    return 0;
                }
                _ => {}
            }
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

#[cfg(test)]
#[path = "windows/tests.rs"]
mod tests;
