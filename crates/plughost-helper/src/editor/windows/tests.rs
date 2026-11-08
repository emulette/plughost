use std::path::Path;

use plughost_formats::vst3::{Module, Plugin};
use windows_sys::Win32::UI::HiDpi::{AreDpiAwarenessContextsEqual, GetThreadDpiAwarenessContext};

use super::*;

/// The SDK sample's editors crash when two threads open them at once; one test opens them at a
/// time.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static EDITORS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    EDITORS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut rect = RECT::default();
    assert_ne!(unsafe { GetClientRect(hwnd, &mut rect) }, 0);
    (rect.right - rect.left, rect.bottom - rect.top)
}

#[test]
#[ignore = "needs scripts/build-test-plugins.ps1; opens a window"]
fn native_editor_scales_minimizes_closes_and_reopens() {
    let _serial = serial();
    let _runtime = crate::events::init(true).unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-plugins/again.vst3");
    // SAFETY: the tests load only the repository's fixture and SDK sample plugins.
    let module = unsafe { Module::load(&path) }.unwrap();
    let mut plugin = Plugin::new(
        &module,
        module.classes()[0].id.class_id().as_str(),
        &plughost_core::HostIdentity::default(),
    )
    .unwrap();
    let original_context = unsafe { GetThreadDpiAwarenessContext() };
    let mut editor = EditorWindow::open(&mut plugin).unwrap();
    assert_ne!(
        unsafe { AreDpiAwarenessContextsEqual(original_context, GetThreadDpiAwarenessContext()) },
        0
    );
    let hwnd = editor.state.hwnd.get();
    let initial = client_size(hwnd);
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let mut rect = RECT::default();
    assert_ne!(unsafe { GetWindowRect(hwnd, &mut rect) }, 0);
    // A monitor notification is the external boundary. The actual SDK view must scale its
    // content and request a matching parent size; this is not a stand-in editor implementation.
    let doubled = (dpi * 2) as usize;
    unsafe {
        SendMessageW(
            hwnd,
            WM_DPICHANGED,
            doubled | (doubled << 16),
            &rect as *const RECT as isize,
        )
    };
    assert!(editor.tick(&mut plugin));
    assert_eq!(client_size(hwnd), (initial.0 * 2, initial.1 * 2));
    unsafe { ShowWindow(hwnd, SW_MINIMIZE) };
    assert!(editor.tick(&mut plugin));
    assert_ne!(unsafe { IsWindow(hwnd) }, 0);
    editor.bring_to_front();
    unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
    crate::events::pump();
    assert!(!editor.tick(&mut plugin));
    // WM_CLOSE leaves the parent alive until the plugin detaches and the owner drops it.
    assert_ne!(unsafe { IsWindow(hwnd) }, 0);
    drop(editor);
    assert_eq!(unsafe { IsWindow(hwnd) }, 0);
    EditorWindow::open(&mut plugin).unwrap().close(&mut plugin);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.ps1; opens a window"]
fn a_size_the_system_does_not_give_the_window_is_refused_and_undone() {
    let _serial = serial();
    let _runtime = crate::events::init(true).unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-plugins/again.vst3");
    // SAFETY: the tests load only the repository's fixture and SDK sample plugins.
    let module = unsafe { Module::load(&path) }.unwrap();
    let mut plugin = Plugin::new(
        &module,
        module.classes()[0].id.class_id().as_str(),
        &plughost_core::HostIdentity::default(),
    )
    .unwrap();
    let editor = EditorWindow::open(&mut plugin).unwrap();
    let hwnd = editor.state.hwnd.get();
    let initial = client_size(hwnd);
    let (width, height) = (initial.0 as u32, initial.1 as u32);
    // A captioned window is never one pixel wide.
    assert!(!editor.state.resize_exactly(1, height));
    assert_eq!(client_size(hwnd), initial);
    assert_eq!(editor.size(), (width, height));
    assert!(editor.state.resize_exactly(width + 20, height + 10));
    assert_eq!(client_size(hwnd), (initial.0 + 20, initial.1 + 10));
    assert_eq!(editor.size(), (width + 20, height + 10));
    editor.close(&mut plugin);
}

/// The client size a window restores to.
fn restored_size(hwnd: HWND) -> (i32, i32) {
    let mut placement = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        ..WINDOWPLACEMENT::default()
    };
    assert_ne!(unsafe { GetWindowPlacement(hwnd, &mut placement) }, 0);
    let outer = placement.rcNormalPosition;
    let mut frame = RECT::default();
    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
    assert_ne!(
        unsafe { AdjustWindowRectExForDpi(&mut frame, style, 0, 0, GetDpiForWindow(hwnd)) },
        0
    );
    (
        (outer.right - outer.left) - (frame.right - frame.left),
        (outer.bottom - outer.top) - (frame.bottom - frame.top),
    )
}

#[test]
#[ignore = "needs scripts/build-test-plugins.ps1; opens a window"]
fn a_minimized_window_restores_to_the_size_it_was_given_and_a_maximized_one_refuses_it() {
    let _serial = serial();
    let _runtime = crate::events::init(true).unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-plugins/again.vst3");
    // SAFETY: the tests load only the repository's fixture and SDK sample plugins.
    let module = unsafe { Module::load(&path) }.unwrap();
    let mut plugin = Plugin::new(
        &module,
        module.classes()[0].id.class_id().as_str(),
        &plughost_core::HostIdentity::default(),
    )
    .unwrap();
    let editor = EditorWindow::open(&mut plugin).unwrap();
    let hwnd = editor.state.hwnd.get();
    let initial = client_size(hwnd);
    let (width, height) = (initial.0 as u32 + 20, initial.1 as u32 + 10);
    unsafe { ShowWindow(hwnd, SW_SHOWMINNOACTIVE) };
    assert!(editor.state.resize_exactly(width, height));
    assert_ne!(unsafe { IsIconic(hwnd) }, 0);
    assert_eq!(restored_size(hwnd), (width as i32, height as i32));
    unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    assert_eq!(client_size(hwnd), (width as i32, height as i32));
    unsafe { ShowWindow(hwnd, SW_MAXIMIZE) };
    let maximized = client_size(hwnd);
    assert!(!editor.state.resize_exactly(width + 20, height + 10));
    assert_ne!(unsafe { IsZoomed(hwnd) }, 0);
    assert_eq!(client_size(hwnd), maximized);
    editor.close(&mut plugin);
}
