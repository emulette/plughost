//! What a remote helper keeps from the machine every session shares. Its general pasteboard is
//! one of its own, so what a plugin copies stays with its session and copies of other sessions
//! or of the machine's user do not reach it. URLs and files a plugin opens, and files it shows
//! in the Finder, open nothing on the machine: the session is told instead
//! (`-[NSWorkspace openURL:]`, `openURL:configuration:completionHandler:`, `openFile:`,
//! `activateFileViewerSelectingURLs:`, `selectFile:inFileViewerRootedAtPath:`, and through the
//! symbol rebinding `LSOpenCFURLRef`). The machine's own mouse does not reach its windows: an
//! event tap for this process drops the mouse events the window server routes to it, the moves
//! that make tracking areas take an entry first among them, which AppKit hands to their owners
//! inside the loop that takes events, out of reach of monitors and `sendEvent:`. The session's
//! input goes into the application's queue and never passes the tap. A tap that drops events
//! needs the application that started the helper to have accessibility access.

use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::{Once, OnceLock};

use block2::Block;
use objc2::ffi::class_replaceMethod;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{ClassType, sel};
use objc2_app_kit::{NSPasteboard, NSPasteboardNameGeneral, NSWorkspace};
use objc2_core_foundation::{CFMachPort, CFRetained, CFRunLoop, kCFRunLoopCommonModes};
use objc2_core_graphics::{
    CGEvent, CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType,
};
use objc2_foundation::{NSArray, NSString, NSURL};
use plughost_core::remote::Notice;

use super::session;

/// The helper's general pasteboard, for the rest of the process's life.
static PASTEBOARD: OnceLock<usize> = OnceLock::new();
/// `+[NSPasteboard pasteboardWithName:]` as AppKit has it, for the names that are not general.
static WITH_NAME: OnceLock<usize> = OnceLock::new();

fn pasteboard() -> *mut NSPasteboard {
    *PASTEBOARD
        .get_or_init(|| Retained::into_raw(NSPasteboard::pasteboardWithUniqueName()) as usize)
        as *mut NSPasteboard
}

extern "C-unwind" fn general_pasteboard(_class: *mut AnyObject, _cmd: Sel) -> *mut NSPasteboard {
    pasteboard()
}

extern "C-unwind" fn pasteboard_with_name(
    class: *mut AnyObject,
    cmd: Sel,
    name: *mut NSString,
) -> *mut NSPasteboard {
    // SAFETY: the name, if given, is a string; the constant is AppKit's.
    if unsafe { name.as_ref() }.is_some_and(|name| name == unsafe { NSPasteboardNameGeneral }) {
        return pasteboard();
    }
    let Some(&original) = WITH_NAME.get() else {
        return std::ptr::null_mut();
    };
    // SAFETY: the original has this signature.
    let original: extern "C-unwind" fn(*mut AnyObject, Sel, *mut NSString) -> *mut NSPasteboard =
        unsafe { std::mem::transmute(original) };
    original(class, cmd, name)
}

fn file_url(url: &NSURL) -> Option<PathBuf> {
    url.path().map(|path| PathBuf::from(path.to_string()))
}

/// Plugins pass nil where a URL or a path failed to parse; AppKit opens nothing then.
extern "C-unwind" fn open_url(_this: &NSWorkspace, _cmd: Sel, url: *mut NSURL) -> Bool {
    // SAFETY: the URL, if given, is a URL.
    let Some(url) = unsafe { url.as_ref() }.and_then(NSURL::absoluteString) else {
        return Bool::NO;
    };
    open(url.to_string());
    Bool::YES
}

extern "C-unwind" fn open_url_configured(
    this: &NSWorkspace,
    cmd: Sel,
    url: *mut NSURL,
    _configuration: *mut AnyObject,
    handler: *mut Block<dyn Fn(*mut AnyObject, *mut AnyObject)>,
) {
    open_url(this, cmd, url);
    // SAFETY: a handler, if given, takes the application and the error, both of which may be
    // none: it opened without either.
    if let Some(handler) = unsafe { handler.as_ref() } {
        handler.call((std::ptr::null_mut(), std::ptr::null_mut()));
    }
}

extern "C-unwind" fn open_file(_this: &NSWorkspace, _cmd: Sel, path: *mut NSString) -> Bool {
    // SAFETY: the path, if given, is a string.
    let Some(url) = unsafe { path.as_ref() }
        .map(NSURL::fileURLWithPath)
        .and_then(|url| url.absoluteString())
    else {
        return Bool::NO;
    };
    open(url.to_string());
    Bool::YES
}

extern "C-unwind" fn reveal_urls(_this: &NSWorkspace, _cmd: Sel, urls: *mut NSArray<NSURL>) {
    // SAFETY: the array, if given, holds URLs.
    if let Some(urls) = unsafe { urls.as_ref() } {
        reveal(urls.iter().filter_map(|url| file_url(&url)).collect());
    }
}

/// The root may be nil or empty, as AppKit allows; the file is shown in it.
extern "C-unwind" fn select_file(
    _this: &NSWorkspace,
    _cmd: Sel,
    file: *mut NSString,
    root: *mut NSString,
) -> Bool {
    // SAFETY: the file and the root, if given, are strings.
    let Some(path) = (unsafe { file.as_ref() }).or(unsafe { root.as_ref() }) else {
        return Bool::NO;
    };
    reveal(vec![PathBuf::from(path.to_string())]);
    Bool::YES
}

/// The event types the machine's mouse sends: buttons, moves, drags, the wheel, and a tracking
/// area's entry and exit, which have no `CGEventType` of their own (8 and 9).
const MACHINE_MOUSE: [u32; 13] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 22, 25, 26, 27];

/// The tap, which the window server disables when its thread answers late.
static TAP: OnceLock<usize> = OnceLock::new();

unsafe extern "C-unwind" fn machine_mouse(
    _proxy: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    _info: *mut c_void,
) -> *mut CGEvent {
    if kind == CGEventType::TapDisabledByTimeout || kind == CGEventType::TapDisabledByUserInput {
        if let Some(&tap) = TAP.get() {
            // SAFETY: the tap lives for the rest of the process's life.
            CGEvent::tap_enable(unsafe { &*(tap as *const CFMachPort) }, true);
        }
        return event.as_ptr();
    }
    if MACHINE_MOUSE.contains(&kind.0) {
        return std::ptr::null_mut();
    }
    event.as_ptr()
}

/// Keeps the machine's mouse from the helper's windows for the rest of the process's life, on a
/// thread of its own so the window server does not wait for the main thread. Returns whether it
/// could: without accessibility access the tap cannot drop events.
pub fn keep_machine_mouse() -> bool {
    let (made, was_made) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mask = MACHINE_MOUSE
            .iter()
            .fold(0u64, |mask, kind| mask | 1 << kind);
        // SAFETY: the callback has the tap's signature and takes no context.
        let tap = unsafe {
            CGEvent::tap_create_for_pid(
                std::process::id() as libc::pid_t,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                mask,
                Some(machine_mouse),
                std::ptr::null_mut(),
            )
        };
        let source = tap
            .as_ref()
            .and_then(|tap| CFMachPort::new_run_loop_source(None, Some(tap), 0));
        let (Some(tap), Some(source), Some(run_loop)) = (tap, source, CFRunLoop::current()) else {
            let _ = made.send(false);
            return;
        };
        run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
        let _ = TAP.set(CFRetained::into_raw(tap).as_ptr() as usize);
        let _ = made.send(true);
        CFRunLoop::run();
    });
    was_made.recv().unwrap_or(false)
}

/// Tells the session the plugin asked to open `url`.
pub fn open(url: String) {
    session::notify(Notice::open_url(url));
}

fn reveal(paths: Vec<PathBuf>) {
    session::notify(Notice::reveal_files(paths));
}

/// Replaces the methods for the rest of the process's life.
pub fn install() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let replace = |class: &AnyClass, sel: Sel, imp: Imp, types: &std::ffi::CStr| {
            // SAFETY: each replacement has the signature its type encoding states, which is the
            // replaced method's.
            unsafe {
                class_replaceMethod(
                    std::ptr::from_ref(class).cast_mut(),
                    sel,
                    imp,
                    types.as_ptr(),
                )
            }
        };
        let pasteboard = NSPasteboard::class().metaclass();
        let workspace = NSWorkspace::class();
        // SAFETY: the transmutes keep each function's signature, which its encoding states.
        unsafe {
            replace(
                pasteboard,
                sel!(generalPasteboard),
                std::mem::transmute::<
                    extern "C-unwind" fn(*mut AnyObject, Sel) -> *mut NSPasteboard,
                    Imp,
                >(general_pasteboard),
                c"@@:",
            );
            if let Some(original) = replace(
                pasteboard,
                sel!(pasteboardWithName:),
                std::mem::transmute::<
                    extern "C-unwind" fn(*mut AnyObject, Sel, *mut NSString) -> *mut NSPasteboard,
                    Imp,
                >(pasteboard_with_name),
                c"@@:@",
            ) {
                let _ = WITH_NAME.set(original as usize);
            }
            replace(
                workspace,
                sel!(openURL:),
                std::mem::transmute::<
                    extern "C-unwind" fn(&NSWorkspace, Sel, *mut NSURL) -> Bool,
                    Imp,
                >(open_url),
                c"B@:@",
            );
            replace(
                workspace,
                sel!(openURL:configuration:completionHandler:),
                std::mem::transmute::<
                    extern "C-unwind" fn(
                        &NSWorkspace,
                        Sel,
                        *mut NSURL,
                        *mut AnyObject,
                        *mut Block<dyn Fn(*mut AnyObject, *mut AnyObject)>,
                    ),
                    Imp,
                >(open_url_configured),
                c"v@:@@@?",
            );
            replace(
                workspace,
                sel!(openFile:),
                std::mem::transmute::<
                    extern "C-unwind" fn(&NSWorkspace, Sel, *mut NSString) -> Bool,
                    Imp,
                >(open_file),
                c"B@:@",
            );
            replace(
                workspace,
                sel!(activateFileViewerSelectingURLs:),
                std::mem::transmute::<
                    extern "C-unwind" fn(&NSWorkspace, Sel, *mut NSArray<NSURL>),
                    Imp,
                >(reveal_urls),
                c"v@:@",
            );
            replace(
                workspace,
                sel!(selectFile:inFileViewerRootedAtPath:),
                std::mem::transmute::<
                    extern "C-unwind" fn(&NSWorkspace, Sel, *mut NSString, *mut NSString) -> Bool,
                    Imp,
                >(select_file),
                c"B@:@@",
            );
        }
    });
}
