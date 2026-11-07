//! What a remote helper keeps from the machine every session shares. Its general pasteboard is
//! one of its own, so what a plugin copies stays with its session and copies of other sessions
//! or of the machine's user do not reach it. URLs and files a plugin opens, and files it shows
//! in the Finder, open nothing on the machine: the session is told instead
//! (`-[NSWorkspace openURL:]`, `openURL:configuration:completionHandler:`, `openFile:`,
//! `activateFileViewerSelectingURLs:`, `selectFile:inFileViewerRootedAtPath:`, and through the
//! symbol rebinding `LSOpenCFURLRef`).

use std::path::PathBuf;
use std::sync::{Once, OnceLock};

use block2::Block;
use objc2::ffi::class_replaceMethod;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{ClassType, sel};
use objc2_app_kit::{NSPasteboard, NSPasteboardNameGeneral, NSWorkspace};
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
    name: &NSString,
) -> *mut NSPasteboard {
    // SAFETY: the name constant is AppKit's.
    if name == unsafe { NSPasteboardNameGeneral } {
        return pasteboard();
    }
    let Some(&original) = WITH_NAME.get() else {
        return std::ptr::null_mut();
    };
    // SAFETY: the original has this signature.
    let original: extern "C-unwind" fn(*mut AnyObject, Sel, &NSString) -> *mut NSPasteboard =
        unsafe { std::mem::transmute(original) };
    original(class, cmd, name)
}

fn file_url(url: &NSURL) -> Option<PathBuf> {
    url.path().map(|path| PathBuf::from(path.to_string()))
}

extern "C-unwind" fn open_url(_this: &NSWorkspace, _cmd: Sel, url: &NSURL) -> Bool {
    if let Some(url) = url.absoluteString() {
        open(url.to_string());
    }
    Bool::YES
}

extern "C-unwind" fn open_url_configured(
    this: &NSWorkspace,
    cmd: Sel,
    url: &NSURL,
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

extern "C-unwind" fn open_file(_this: &NSWorkspace, _cmd: Sel, path: &NSString) -> Bool {
    let url = NSURL::fileURLWithPath(path);
    if let Some(url) = url.absoluteString() {
        open(url.to_string());
    }
    Bool::YES
}

extern "C-unwind" fn reveal_urls(_this: &NSWorkspace, _cmd: Sel, urls: &NSArray<NSURL>) {
    reveal(urls.iter().filter_map(|url| file_url(&url)).collect());
}

extern "C-unwind" fn select_file(
    _this: &NSWorkspace,
    _cmd: Sel,
    file: *mut NSString,
    root: &NSString,
) -> Bool {
    // SAFETY: the file, if given, is a string.
    let path = unsafe { file.as_ref() }.unwrap_or(root);
    reveal(vec![PathBuf::from(path.to_string())]);
    Bool::YES
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
                class_replaceMethod(std::ptr::from_ref(class).cast_mut(), sel, imp, types.as_ptr())
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
                    extern "C-unwind" fn(*mut AnyObject, Sel, &NSString) -> *mut NSPasteboard,
                    Imp,
                >(pasteboard_with_name),
                c"@@:@",
            ) {
                let _ = WITH_NAME.set(original as usize);
            }
            replace(
                workspace,
                sel!(openURL:),
                std::mem::transmute::<extern "C-unwind" fn(&NSWorkspace, Sel, &NSURL) -> Bool, Imp>(
                    open_url,
                ),
                c"B@:@",
            );
            replace(
                workspace,
                sel!(openURL:configuration:completionHandler:),
                std::mem::transmute::<
                    extern "C-unwind" fn(
                        &NSWorkspace,
                        Sel,
                        &NSURL,
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
                    extern "C-unwind" fn(&NSWorkspace, Sel, &NSString) -> Bool,
                    Imp,
                >(open_file),
                c"B@:@",
            );
            replace(
                workspace,
                sel!(activateFileViewerSelectingURLs:),
                std::mem::transmute::<extern "C-unwind" fn(&NSWorkspace, Sel, &NSArray<NSURL>), Imp>(
                    reveal_urls,
                ),
                c"v@:@",
            );
            replace(
                workspace,
                sel!(selectFile:inFileViewerRootedAtPath:),
                std::mem::transmute::<
                    extern "C-unwind" fn(&NSWorkspace, Sel, *mut NSString, &NSString) -> Bool,
                    Imp,
                >(select_file),
                c"B@:@@",
            );
        }
    });
}
