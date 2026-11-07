//! The cursor a plugin sets, which the application draws over what it reads. `-[NSCursor set]`
//! and the ways to hide the cursor (`+[NSCursor hide]`, `unhide`, `setHiddenUntilMouseMoves:`,
//! and through the symbol rebinding `CGDisplayHideCursor` and `CGDisplayShowCursor`) change this
//! cursor instead of the machine's, which every session shares. A cursor whose image has nothing
//! to see, as JUCE sets to hide it, counts as hidden.

use std::cell::RefCell;
use std::sync::Once;

use objc2::ffi::class_replaceMethod;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{AnyThread, MainThreadMarker, Message, sel};
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSCursor};
use objc2_core_graphics::{CGDataProvider, CGImage};
use objc2_foundation::NSDictionary;
use plughost_core::remote::{CursorImage, CursorShape, Notice, Point};

use super::session;

struct State {
    /// The cursor set last, which a plugin sets again on every move.
    set: Option<Retained<NSCursor>>,
    shape: CursorShape,
    /// The cursor set has nothing to see.
    blank: bool,
    /// Hides not yet balanced by shows.
    hides: u32,
    /// Hidden until the pointer moves.
    until_moved: bool,
    told: Option<(CursorShape, bool)>,
}

thread_local! {
    static STATE: RefCell<State> = const {
        RefCell::new(State {
            set: None,
            shape: CursorShape::Arrow,
            blank: false,
            hides: 0,
            until_moved: false,
            told: None,
        })
    };
}

extern "C-unwind" fn set(this: &NSCursor, _cmd: Sel) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    let same = STATE.with_borrow(|state| {
        state
            .set
            .as_ref()
            .is_some_and(|set| std::ptr::eq(&**set, this))
    });
    if same {
        return;
    }
    let (shape, blank) = describe(this);
    STATE.with_borrow_mut(|state| {
        state.set = Some(this.retain());
        state.shape = shape;
        state.blank = blank;
    });
    changed();
}

extern "C-unwind" fn hide_class(_class: *mut AnyObject, _cmd: Sel) {
    hide();
}

extern "C-unwind" fn unhide_class(_class: *mut AnyObject, _cmd: Sel) {
    show();
}

extern "C-unwind" fn hidden_until_moved(_class: *mut AnyObject, _cmd: Sel, flag: Bool) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    STATE.with_borrow_mut(|state| state.until_moved = flag.as_bool());
    changed();
}

/// Replaces the methods for the rest of the process's life.
pub fn install() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let Some(class) = AnyClass::get(c"NSCursor") else {
            return;
        };
        let metaclass = std::ptr::from_ref(class.metaclass()).cast_mut();
        // SAFETY: each replacement has the signature its type encoding states, which is the
        // replaced method's.
        unsafe {
            class_replaceMethod(
                std::ptr::from_ref(class).cast_mut(),
                sel!(set),
                std::mem::transmute::<extern "C-unwind" fn(&NSCursor, Sel), Imp>(set),
                c"v@:".as_ptr(),
            );
            for (sel, imp) in [
                (sel!(hide), hide_class as extern "C-unwind" fn(_, _)),
                (sel!(unhide), unhide_class),
            ] {
                class_replaceMethod(
                    metaclass,
                    sel,
                    std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel), Imp>(imp),
                    c"v@:".as_ptr(),
                );
            }
            class_replaceMethod(
                metaclass,
                sel!(setHiddenUntilMouseMoves:),
                std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel, Bool), Imp>(
                    hidden_until_moved,
                ),
                c"v@:B".as_ptr(),
            );
        }
    });
}

/// The plugin hid the cursor.
pub fn hide() {
    if MainThreadMarker::new().is_none() {
        return;
    }
    STATE.with_borrow_mut(|state| state.hides += 1);
    changed();
}

/// The plugin showed the cursor it hid.
pub fn show() {
    if MainThreadMarker::new().is_none() {
        return;
    }
    STATE.with_borrow_mut(|state| state.hides = state.hides.saturating_sub(1));
    changed();
}

/// The pointer moved, which shows a cursor hidden until it does.
pub fn moved() {
    let until_moved = STATE.with_borrow_mut(|state| std::mem::take(&mut state.until_moved));
    if until_moved {
        changed();
    }
}

/// Tells a new session the current cursor.
pub fn tell_all() {
    STATE.with_borrow_mut(|state| state.told = None);
    changed();
}

fn changed() {
    STATE.with_borrow_mut(|state| {
        let now = (
            state.shape.clone(),
            state.blank || state.hides > 0 || state.until_moved,
        );
        if state.told.as_ref() != Some(&now) {
            session::notify(Notice::cursor(now.0.clone(), now.1));
            state.told = Some(now);
        }
    });
}

/// The cursor's shape, and whether its image has nothing to see.
fn describe(cursor: &NSCursor) -> (CursorShape, bool) {
    #[expect(
        deprecated,
        reason = "plugins built against earlier SDKs set these cursors, which are told by name"
    )]
    let system = [
        (NSCursor::arrowCursor(), CursorShape::Arrow),
        (NSCursor::IBeamCursor(), CursorShape::IBeam),
        (
            NSCursor::IBeamCursorForVerticalLayout(),
            CursorShape::IBeamVertical,
        ),
        (NSCursor::pointingHandCursor(), CursorShape::PointingHand),
        (NSCursor::crosshairCursor(), CursorShape::Crosshair),
        (NSCursor::openHandCursor(), CursorShape::OpenHand),
        (NSCursor::closedHandCursor(), CursorShape::ClosedHand),
        (NSCursor::resizeLeftCursor(), CursorShape::ResizeLeft),
        (NSCursor::resizeRightCursor(), CursorShape::ResizeRight),
        (
            NSCursor::resizeLeftRightCursor(),
            CursorShape::ResizeLeftRight,
        ),
        (NSCursor::resizeUpCursor(), CursorShape::ResizeUp),
        (NSCursor::resizeDownCursor(), CursorShape::ResizeDown),
        (NSCursor::resizeUpDownCursor(), CursorShape::ResizeUpDown),
        (NSCursor::dragLinkCursor(), CursorShape::DragLink),
        (NSCursor::dragCopyCursor(), CursorShape::DragCopy),
        (
            NSCursor::operationNotAllowedCursor(),
            CursorShape::OperationNotAllowed,
        ),
        (
            NSCursor::contextualMenuCursor(),
            CursorShape::ContextualMenu,
        ),
        (
            NSCursor::disappearingItemCursor(),
            CursorShape::DisappearingItem,
        ),
    ];
    if let Some((_, shape)) = system
        .into_iter()
        .find(|(known, _)| std::ptr::eq(&**known, cursor))
    {
        return (shape, false);
    }
    let image = cursor.image();
    let size = image.size();
    let hot_spot = cursor.hotSpot();
    // SAFETY: no rectangle, context or hints asks for the image's own best representation.
    let cg =
        unsafe { image.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) };
    let Some(cg) = cg else {
        return (CursorShape::Arrow, false);
    };
    let blank = CGDataProvider::data(CGImage::data_provider(Some(&cg)).as_deref())
        .is_some_and(|data| data.to_vec().iter().all(|byte| *byte == 0));
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
    // SAFETY: no properties are a valid property dictionary.
    let png = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .map(|data| data.to_vec())
    .unwrap_or_default();
    (
        CursorShape::Image(CursorImage::new(
            png,
            size.width,
            size.height,
            Point::new(hot_spot.x, hot_spot.y),
        )),
        blank,
    )
}
