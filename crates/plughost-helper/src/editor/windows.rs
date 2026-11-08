//! Native editor parents. Window messages only update state; plugin calls stay outside WndProc.

use std::cell::Cell;
use std::io;
use std::ptr::null_mut;
use std::rc::Rc;
use std::sync::OnceLock;

use plughost_core::remote::{EditorId, Region};
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
    /// Restores a maximized window that shows, without activating it, for an editor that cannot
    /// take its size.
    fn unmaximize(&self) {
        let hwnd = self.hwnd.get();
        if unsafe { IsZoomed(hwnd) } != 0 && unsafe { IsWindowVisible(hwnd) } != 0 {
            unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        }
    }

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
        if unsafe {
            AdjustWindowRectExForDpi(
                &mut rect,
                GetWindowLongW(hwnd, GWL_STYLE) as u32,
                0,
                0,
                GetDpiForWindow(hwnd),
            )
        } == 0
        {
            return false;
        }
        let (outer_width, outer_height) = (rect.right - rect.left, rect.bottom - rect.top);
        // `SetWindowPos` sizes what shows: the frame of a maximized window, which stays
        // maximized, or nothing of a minimized one, not the window either restores to.
        if unsafe { IsZoomed(hwnd) } != 0 {
            // A maximized window keeps its size, which `tick` offers the editor.
            self.size.set((width, height));
            return true;
        }
        let resized = if unsafe { IsIconic(hwnd) } != 0 {
            // The window restores to this size, still minimized and not activated meanwhile.
            let mut placement = WINDOWPLACEMENT {
                length: size_of::<WINDOWPLACEMENT>() as u32,
                ..WINDOWPLACEMENT::default()
            };
            let got = unsafe { GetWindowPlacement(hwnd, &mut placement) } != 0;
            let normal = &mut placement.rcNormalPosition;
            normal.right = normal.left + outer_width;
            normal.bottom = normal.top + outer_height;
            placement.showCmd = SW_SHOWMINNOACTIVE as u32;
            got && unsafe { SetWindowPlacement(hwnd, &placement) } != 0
        } else {
            let moved = unsafe {
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    0,
                    0,
                    outer_width,
                    outer_height,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                )
            };
            moved != 0
        };
        if resized {
            // The system keeps a window between its smallest and largest sizes. Record the size
            // the window got, which `tick` compares it with; a minimized window has no client area.
            let mut client = RECT::default();
            let size = if unsafe { IsIconic(hwnd) } == 0
                && unsafe { GetClientRect(hwnd, &mut client) } != 0
            {
                (
                    (client.right - client.left) as u32,
                    (client.bottom - client.top) as u32,
                )
            } else {
                (width, height)
            };
            self.size.set(size);
        }
        resized
    }

    /// Gives the window the size a plugin asked for. When the system keeps the window from
    /// taking exactly that size, the window goes back to the size it had and the request fails;
    /// a maximized window takes no other size.
    fn resize_exactly(&self, width: u32, height: u32) -> bool {
        if unsafe { IsZoomed(self.hwnd.get()) } != 0 {
            return false;
        }
        let previous = self.size.get();
        if self.resize(width, height) && self.size.get() == (width, height) {
            return true;
        }
        self.resize(previous.0, previous.1);
        false
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
        window.attach(plugin)?;
        if !window.floating {
            window.bring_to_front();
        }
        Ok(window)
    }

    /// Remote editors are available on macOS only, where a chain can start in remote mode.
    pub fn open_remote(
        _plugin: &mut dyn HostedPlugin,
        _region: Region,
        _id: EditorId,
    ) -> Result<Self, Failure> {
        Err(Failure::new(
            FailureKind::Unsupported,
            crate::errors::NOT_REMOTE,
        ))
    }

    /// No editor is remote on Windows.
    pub fn remote_id(&self) -> Option<EditorId> {
        None
    }

    pub fn size(&self) -> (u32, u32) {
        self.state.size.get()
    }

    pub fn set_region(&mut self, _region: Region) {}

    /// Opens `plugin`'s editor in this window after [`EditorWindow::detach`]. The window keeps
    /// its place, order, and minimized or hidden state, and does not take focus; it shows again
    /// when a floating editor before hid it. On failure the caller drops the window.
    pub fn reattach(&mut self, plugin: &mut dyn HostedPlugin) -> Result<(), Failure> {
        let _context = DpiContext::enter();
        let was_floating = self.floating;
        self.attach(plugin)?;
        if self.hidden {
            plugin.set_editor_visible(false);
        } else if was_floating && !self.floating {
            // The floating editor before hid the window; it shows again without activating.
            unsafe { ShowWindow(self.state.hwnd.get(), SW_SHOWNA) };
        }
        Ok(())
    }

    /// Closes the editor and keeps its window for the instance that replaces `plugin`.
    pub fn detach(&mut self, plugin: &mut dyn HostedPlugin) {
        let _context = DpiContext::enter();
        plugin.close_editor();
    }

    fn attach(&mut self, plugin: &mut dyn HostedPlugin) -> Result<(), Failure> {
        let hwnd = self.state.hwnd.get();
        let frame = Rc::clone(&self.state);
        let resize: ResizeRequest =
            Box::new(move |width, height| frame.resize_exactly(width, height));
        // SAFETY: close(), detach() and tick() detach the plugin before the native parent is
        // destroyed.
        let view = unsafe { plugin.open_editor(hwnd, resize) }.map_err(|error| error.failure())?;
        let EditorView::Embedded { width, height } = view else {
            self.floating = true;
            unsafe { ShowWindow(hwnd, SW_HIDE) };
            return Ok(());
        };
        self.floating = false;
        // Only the resize frame follows the editor; the window keeps its visible, minimized and
        // maximized state, which a reattached editor must not change.
        let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
        let style = if plugin.editor_can_resize() {
            style | WS_THICKFRAME | WS_MAXIMIZEBOX
        } else {
            style & !(WS_THICKFRAME | WS_MAXIMIZEBOX)
        };
        unsafe { SetWindowLongW(hwnd, GWL_STYLE, style as i32) };
        if !plugin.editor_can_resize() {
            self.state.unmaximize();
        }
        if !self.state.resize(width, height) {
            let error = window_error(io::Error::last_os_error());
            plugin.close_editor();
            return Err(error);
        }
        self.state.scale.set(Some(unsafe { GetDpiForWindow(hwnd) }));
        self.update_scale(plugin);
        Ok(())
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
                    // A maximized window the editor cannot fill is restored to its size.
                    self.state.unmaximize();
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
