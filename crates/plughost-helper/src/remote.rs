//! Remote mode: the helper shows editors to someone who is not in front of the machine, while
//! other sessions and the machine's own user may be. It never activates itself: a plugin
//! framework may bring its application to the front on a click (JUCE does), which would take
//! the focus from whatever is in front. Frameworks that close their menus when their application
//! is not active (JUCE does) are told it is.

use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

mod cursor;
mod hover;
pub mod input;
mod isolation;
pub mod menu;
mod mouse;
pub mod panel;
pub mod rebind;
pub mod session;
pub mod watch;

use objc2::ffi::class_replaceMethod;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::sel;

extern "C-unwind" fn ignore(_this: *mut AnyObject, _cmd: Sel) {}

extern "C-unwind" fn ignore_flag(_this: *mut AnyObject, _cmd: Sel, _flag: Bool) {}

extern "C-unwind" fn ignore_options(_this: *mut AnyObject, _cmd: Sel, _options: usize) -> Bool {
    Bool::NO
}

extern "C-unwind" fn ignore_from(
    _this: *mut AnyObject,
    _cmd: Sel,
    _application: *mut AnyObject,
    _options: usize,
) -> Bool {
    Bool::NO
}

extern "C-unwind" fn active(_this: *mut AnyObject, _cmd: Sel) -> Bool {
    Bool::YES
}

/// Whether the machine's mouse is kept from the helper's windows; see [`isolation`].
static MACHINE_MOUSE_KEPT: AtomicBool = AtomicBool::new(false);

/// Whether the machine's mouse is kept from the helper's windows.
pub fn machine_mouse_kept() -> bool {
    MACHINE_MOUSE_KEPT.load(Ordering::Relaxed)
}

/// Puts the process in remote mode for the rest of its life: activating the application does
/// nothing, the application reports itself active, plugins move the helper's own mouse and set
/// its own cursor, not the machine's, they copy to a pasteboard of its own and open nothing on
/// the machine, and the machine's mouse does not reach its windows when it can be kept away.
pub fn enter() {
    rebind::install();
    cursor::install();
    isolation::install();
    MACHINE_MOUSE_KEPT.store(isolation::keep_machine_mouse(), Ordering::Relaxed);
    static ENTERED: Once = Once::new();
    ENTERED.call_once(|| {
        // SAFETY: each replacement has the signature its type encoding states, which is the
        // replaced method's.
        let replacements: [(&std::ffi::CStr, Sel, Imp, &std::ffi::CStr); 5] = unsafe {
            [
                (
                    c"NSApplication",
                    sel!(activateIgnoringOtherApps:),
                    std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel, Bool), Imp>(
                        ignore_flag,
                    ),
                    c"v@:B",
                ),
                (
                    c"NSApplication",
                    sel!(activate),
                    std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel), Imp>(ignore),
                    c"v@:",
                ),
                (
                    c"NSRunningApplication",
                    sel!(activateWithOptions:),
                    std::mem::transmute::<
                        extern "C-unwind" fn(*mut AnyObject, Sel, usize) -> Bool,
                        Imp,
                    >(ignore_options),
                    c"B@:Q",
                ),
                // Cooperative activation, which another application's yielding allows.
                (
                    c"NSRunningApplication",
                    sel!(activateFromApplication:options:),
                    std::mem::transmute::<
                        extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, usize) -> Bool,
                        Imp,
                    >(ignore_from),
                    c"B@:@Q",
                ),
                (
                    c"NSApplication",
                    sel!(isActive),
                    std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel) -> Bool, Imp>(
                        active,
                    ),
                    c"B@:",
                ),
            ]
        };
        for (class, sel, imp, types) in replacements {
            if let Some(class) = AnyClass::get(class) {
                // SAFETY: AppKit's classes are registered; the replacement keeps the encoding.
                unsafe {
                    class_replaceMethod(
                        std::ptr::from_ref(class).cast_mut(),
                        sel,
                        imp,
                        types.as_ptr(),
                    )
                };
            }
        }
    });
}
