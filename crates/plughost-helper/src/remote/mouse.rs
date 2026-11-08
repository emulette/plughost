//! The helper's own mouse. The machine has one cursor for every session on it, so plugins that
//! ask where the mouse is, which buttons are down or which modifiers are held, outside the
//! events they get, are answered with what the remote session's input did: `+[NSEvent
//! mouseLocation]`, `pressedMouseButtons` and `modifierFlags` answer from this mouse. It follows
//! the mouse events the application takes from its queue, in whatever loop, as the real mouse
//! would move, and a plugin's warp moves it instead of the real cursor.

use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Once, OnceLock};

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::ffi::class_replaceMethod;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::sel;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSEventType};
use objc2_foundation::NSPoint;

/// Where the mouse is, as the bits of an AppKit screen point's coordinates.
static X: AtomicU64 = AtomicU64::new(0);
static Y: AtomicU64 = AtomicU64::new(0);
/// The buttons down, as `pressedMouseButtons` counts them: 1 the left, 2 the right, 4 the next.
static BUTTONS: AtomicUsize = AtomicUsize::new(0);
/// The modifiers held, as `NSEventModifierFlags` bits.
static FLAGS: AtomicUsize = AtomicUsize::new(0);

extern "C-unwind" fn mouse_location(_class: *mut AnyObject, _cmd: Sel) -> NSPoint {
    location()
}

extern "C-unwind" fn pressed_mouse_buttons(_class: *mut AnyObject, _cmd: Sel) -> usize {
    BUTTONS.load(Ordering::Relaxed)
}

extern "C-unwind" fn modifier_flags(_class: *mut AnyObject, _cmd: Sel) -> usize {
    FLAGS.load(Ordering::Relaxed)
}

/// `-[NSApplication nextEventMatchingMask:untilDate:inMode:dequeue:]` as AppKit has it.
type NextEvent = extern "C-unwind" fn(
    *mut AnyObject,
    Sel,
    u64,
    *mut AnyObject,
    *mut AnyObject,
    Bool,
) -> *mut NSEvent;

static NEXT_EVENT: OnceLock<usize> = OnceLock::new();
static DEQUEUED: OnceLock<fn(&NSEvent)> = OnceLock::new();

/// Every loop takes events through this, a control's or a dialog's that sends no event through
/// the application's monitors included.
extern "C-unwind" fn next_event(
    this: *mut AnyObject,
    cmd: Sel,
    mask: u64,
    until: *mut AnyObject,
    mode: *mut AnyObject,
    dequeue: Bool,
) -> *mut NSEvent {
    let Some(&original) = NEXT_EVENT.get() else {
        return std::ptr::null_mut();
    };
    // SAFETY: the original has this signature.
    let original = unsafe { std::mem::transmute::<usize, NextEvent>(original) };
    let event = original(this, cmd, mask, until, mode, dequeue);
    // SAFETY: the event, if any, is valid at least until the autorelease pool drains.
    if let Some(event) = unsafe { event.as_ref() }
        && dequeue.as_bool()
    {
        follow(event);
        if let Some(dequeued) = DEQUEUED.get() {
            dequeued(event);
        }
    }
    event
}

/// Answers the class methods from this mouse and follows the events the application takes from
/// its queue, for the rest of the process's life. `dequeued` is called with each one as it is
/// taken, and `handed` with each mouse, key and wheel event the application hands out before it
/// goes to its window, which tells whether it took the event, which then goes no further.
pub fn install(_mtm: MainThreadMarker, handed: fn(&NSEvent) -> bool, dequeued: fn(&NSEvent)) {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let Some(class) = AnyClass::get(c"NSEvent") else {
            return;
        };
        let metaclass = std::ptr::from_ref(class.metaclass()).cast_mut();
        // SAFETY: each replacement has the signature its type encoding states, which is the
        // replaced class method's.
        unsafe {
            class_replaceMethod(
                metaclass,
                sel!(mouseLocation),
                std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel) -> NSPoint, Imp>(
                    mouse_location,
                ),
                c"{CGPoint=dd}@:".as_ptr(),
            );
            class_replaceMethod(
                metaclass,
                sel!(pressedMouseButtons),
                std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel) -> usize, Imp>(
                    pressed_mouse_buttons,
                ),
                c"Q@:".as_ptr(),
            );
            class_replaceMethod(
                metaclass,
                sel!(modifierFlags),
                std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel) -> usize, Imp>(
                    modifier_flags,
                ),
                c"Q@:".as_ptr(),
            );
        }
        let _ = DEQUEUED.set(dequeued);
        if let Some(application) = AnyClass::get(c"NSApplication") {
            // SAFETY: the replacement has the signature its type encoding states, which is the
            // replaced method's; the original is kept before any event can reach the replacement.
            let original = unsafe {
                let method = objc2::ffi::class_getInstanceMethod(
                    application,
                    sel!(nextEventMatchingMask:untilDate:inMode:dequeue:),
                );
                objc2::ffi::method_getImplementation(method)
            };
            if let Some(original) = original {
                let _ = NEXT_EVENT.set(original as usize);
                // SAFETY: as above.
                unsafe {
                    class_replaceMethod(
                        std::ptr::from_ref(application).cast_mut(),
                        sel!(nextEventMatchingMask:untilDate:inMode:dequeue:),
                        std::mem::transmute::<NextEvent, Imp>(next_event),
                        c"@@:Q@@B".as_ptr(),
                    );
                }
            }
        }
        let follow = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: the monitor hands a valid event, on the main thread.
            let event_ref = unsafe { event.as_ref() };
            if handed(event_ref) {
                std::ptr::null_mut()
            } else {
                event.as_ptr()
            }
        });
        // SAFETY: the handler returns the event it was given, which goes on to its window, or
        // none.
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::LeftMouseDown
                    | NSEventMask::LeftMouseUp
                    | NSEventMask::LeftMouseDragged
                    | NSEventMask::RightMouseDown
                    | NSEventMask::RightMouseUp
                    | NSEventMask::RightMouseDragged
                    | NSEventMask::OtherMouseDown
                    | NSEventMask::OtherMouseUp
                    | NSEventMask::OtherMouseDragged
                    | NSEventMask::MouseMoved
                    | NSEventMask::KeyDown
                    | NSEventMask::KeyUp
                    | NSEventMask::ScrollWheel,
                &follow,
            )
        };
        // The monitor lives as long as the process.
        std::mem::forget(monitor);
    });
}

/// Follows a mouse, key or wheel event taken from the queue, on the main thread.
fn follow(event: &NSEvent) {
    if !matches!(
        event.r#type(),
        NSEventType::LeftMouseDown
            | NSEventType::LeftMouseUp
            | NSEventType::LeftMouseDragged
            | NSEventType::RightMouseDown
            | NSEventType::RightMouseUp
            | NSEventType::RightMouseDragged
            | NSEventType::OtherMouseDown
            | NSEventType::OtherMouseUp
            | NSEventType::OtherMouseDragged
            | NSEventType::MouseMoved
            | NSEventType::KeyDown
            | NSEventType::KeyUp
            | NSEventType::ScrollWheel
    ) {
        return;
    }
    set_modifiers(event.modifierFlags());
    if matches!(
        event.r#type(),
        NSEventType::KeyDown | NSEventType::KeyUp | NSEventType::ScrollWheel
    ) {
        return;
    }
    // SAFETY: events are taken from the queue on the main thread.
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    if let Some(window) = event.window(mtm) {
        move_to(window.convertPointToScreen(event.locationInWindow()));
    }
    let bit = match event.r#type() {
        NSEventType::LeftMouseDown | NSEventType::LeftMouseUp | NSEventType::LeftMouseDragged => 1,
        NSEventType::RightMouseDown
        | NSEventType::RightMouseUp
        | NSEventType::RightMouseDragged => 2,
        NSEventType::OtherMouseDown
        | NSEventType::OtherMouseUp
        | NSEventType::OtherMouseDragged => 4,
        _ => 0,
    };
    match event.r#type() {
        NSEventType::LeftMouseUp | NSEventType::RightMouseUp | NSEventType::OtherMouseUp => {
            BUTTONS.fetch_and(!bit, Ordering::Relaxed);
        }
        _ => {
            BUTTONS.fetch_or(bit, Ordering::Relaxed);
        }
    }
}

/// Moves the mouse to an AppKit screen point.
pub fn move_to(point: NSPoint) {
    X.store(point.x.to_bits(), Ordering::Relaxed);
    Y.store(point.y.to_bits(), Ordering::Relaxed);
}

/// Where the mouse is, in AppKit screen points.
pub fn location() -> NSPoint {
    NSPoint::new(
        f64::from_bits(X.load(Ordering::Relaxed)),
        f64::from_bits(Y.load(Ordering::Relaxed)),
    )
}

/// The modifiers held from now on.
pub fn set_modifiers(flags: NSEventModifierFlags) {
    FLAGS.store(flags.0, Ordering::Relaxed);
}
