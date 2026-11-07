//! Hover. AppKit messages the owners of tracking areas when the window server reports the real
//! cursor moving; a move posted to the application's queue does not take that path. The owners
//! of the areas under the helper's mouse are messaged directly instead (`mouseEntered:`,
//! `mouseMoved:`, `mouseExited:`, and `cursorUpdate:` on entering an area that asks for it, where
//! FabFilter's and VSTGUI's views set their cursors), as AppKit would.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{Message, msg_send};
use objc2_app_kit::{
    NSEvent, NSEventModifierFlags, NSEventType, NSTrackingAreaOptions, NSView, NSWindow,
};
use objc2_foundation::{NSPoint, NSProcessInfo};

use super::mouse;

thread_local! {
    /// The window the mouse was last over, and the tracking areas there it was inside.
    static OVER: RefCell<Option<(Retained<NSWindow>, Vec<usize>)>> = const { RefCell::new(None) };
}

fn views(view: &NSView, out: &mut Vec<Retained<NSView>>) {
    out.push(view.retain());
    for subview in view.subviews() {
        views(&subview, out);
    }
}

/// Moves the mouse to `point` of the screen over `window`, and messages the owners of the
/// tracking areas it enters, moves in and leaves, in `window` and in the window it was over.
pub fn moved(window: &NSWindow, point: NSPoint, flags: NSEventModifierFlags) {
    mouse::move_to(point);
    let before = OVER.take();
    let mut inside = Vec::new();
    if let Some((previous, mut areas)) = before {
        if &*previous == window {
            inside = areas;
        } else {
            // Away from every area of the window it left.
            visit(&previous, None, &mut areas, flags);
        }
    }
    let frame = window.frame();
    let local = NSPoint::new(point.x - frame.origin.x, point.y - frame.origin.y);
    visit(window, Some(local), &mut inside, flags);
    OVER.set(Some((window.retain(), inside)));
}

/// Messages the owners of `window`'s tracking areas for the mouse at `point` of the window, or
/// away from it, and leaves in `inside` the areas the mouse is in.
fn visit(
    window: &NSWindow,
    point: Option<NSPoint>,
    inside: &mut Vec<usize>,
    flags: NSEventModifierFlags,
) {
    let Some(content) = window.contentView() else {
        return;
    };
    let mut all = Vec::new();
    views(&content, &mut all);
    let now = NSProcessInfo::processInfo().systemUptime();
    let number = window.windowNumber();
    let at = point.unwrap_or(NSPoint::new(-1.0, -1.0));
    let mut now_inside = Vec::new();
    for view in all {
        for area in view.trackingAreas() {
            let options = area.options();
            let rect = if options.contains(NSTrackingAreaOptions::InVisibleRect) {
                view.visibleRect()
            } else {
                area.rect()
            };
            let local = view.convertPoint_fromView(at, None);
            let is_inside = point.is_some()
                && local.x >= rect.origin.x
                && local.y >= rect.origin.y
                && local.x < rect.origin.x + rect.size.width
                && local.y < rect.origin.y + rect.size.height;
            let key = Retained::as_ptr(&area) as usize;
            let was_inside = inside.contains(&key);
            // SAFETY: AppKit keeps the owner alive while its area is installed.
            let Some(owner) = area.owner() else {
                continue;
            };
            let owner: &AnyObject = &owner;
            if options.contains(NSTrackingAreaOptions::CursorUpdate) && is_inside && !was_inside {
                // SAFETY: a cursor update event names no user data.
                let event = unsafe {
                    NSEvent::enterExitEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_trackingNumber_userData(
                        NSEventType::CursorUpdate, at, flags, now, number, None, 0, key as isize, std::ptr::null_mut(),
                    )
                };
                if let Some(event) = event {
                    // SAFETY: a tracking area owner that asks for cursor updates answers it.
                    let _: () = unsafe { msg_send![owner, cursorUpdate: &*event] };
                }
            }
            if options.contains(NSTrackingAreaOptions::MouseEnteredAndExited)
                && is_inside != was_inside
            {
                let kind = if is_inside {
                    NSEventType::MouseEntered
                } else {
                    NSEventType::MouseExited
                };
                // SAFETY: an enter or exit event names no user data.
                let event = unsafe {
                    NSEvent::enterExitEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_trackingNumber_userData(
                        kind, at, flags, now, number, None, 0, key as isize, std::ptr::null_mut(),
                    )
                };
                if let Some(event) = event {
                    // SAFETY: tracking area owners answer these messages.
                    unsafe {
                        if is_inside {
                            let _: () = msg_send![owner, mouseEntered: &*event];
                        } else {
                            let _: () = msg_send![owner, mouseExited: &*event];
                        }
                    }
                }
            }
            if is_inside && options.contains(NSTrackingAreaOptions::MouseMoved) {
                let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                    NSEventType::MouseMoved, at, flags, now, number, None, 0, 0, 0.0,
                );
                if let Some(event) = event {
                    // SAFETY: a tracking area owner that asks for moves answers the message.
                    let _: () = unsafe { msg_send![owner, mouseMoved: &*event] };
                }
            }
            if is_inside {
                now_inside.push(key);
            }
        }
    }
    *inside = now_inside;
}
