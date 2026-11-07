//! Open and save panels a plugin runs. AppKit shows them in a system service's window on the
//! machine's main display, out of the session's reach, but the panel object and the calls that
//! run it are in the helper: `-[NSSavePanel runModal]`, `beginWithCompletionHandler:` and
//! `beginSheetModalForWindow:completionHandler:` run it, then `URL` (and `-[NSOpenPanel URLs]`)
//! give its answer. These ask the session instead and never show a panel: `runModal` runs the
//! run loop until the answer comes, as the real panel would, and the other two call their
//! handler then. The answer stays on the panel object for the plugin to read.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::Once;

use block2::{Block, RcBlock};
use objc2::ffi::{OBJC_ASSOCIATION_RETAIN_NONATOMIC, class_replaceMethod};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, Imp, Sel};
use objc2::{ClassType, MainThreadMarker, Message, sel};
use objc2_app_kit::{NSOpenPanel, NSSavePanel, NSWindow};
use objc2_core_foundation::{CFRunLoop, kCFRunLoopDefaultMode};
use objc2_foundation::{NSArray, NSString, NSURL};
use plughost_core::remote::{Notice, PanelAnswer, PanelId, PanelKind, PanelRequest};

use super::session;

/// `NSModalResponseOK` and `NSModalResponseCancel`.
const OK: isize = 1;
const CANCEL: isize = 0;

/// How the plugin waits for the answer.
enum Waits {
    /// In `runModal`, until the answer is set.
    Modal(Option<Option<Vec<PathBuf>>>),
    /// For its completion handler.
    Handler(RcBlock<dyn Fn(isize)>),
}

struct Waiting {
    id: PanelId,
    panel: Retained<NSSavePanel>,
    request: PanelRequest,
    waits: Waits,
}

thread_local! {
    static WAITING: RefCell<Vec<Waiting>> = const { RefCell::new(Vec::new()) };
    static NEXT: Cell<u64> = const { Cell::new(1) };
}

/// The key the answer is kept under on its panel.
static ANSWER: u8 = 0;

fn request(panel: &NSSavePanel) -> PanelRequest {
    let open = panel.downcast_ref::<NSOpenPanel>();
    #[expect(
        deprecated,
        reason = "plugins set the types with `setAllowedFileTypes:`, which this reads back"
    )]
    let types = panel
        .allowedFileTypes()
        .map(|types| types.iter().map(|kind| kind.to_string()).collect())
        .unwrap_or_default();
    PanelRequest::new(
        if open.is_some() {
            PanelKind::Open
        } else {
            PanelKind::Save
        },
        panel
            .directoryURL()
            .and_then(|url| url.path())
            .map(|path| PathBuf::from(path.to_string())),
        panel.nameFieldStringValue().to_string(),
        types,
        open.is_some_and(|open| open.allowsMultipleSelection()),
        open.is_some_and(|open| open.canChooseDirectories()),
        panel.message().to_string(),
    )
}

/// Tells the session the panel waits, and keeps it until the answer.
fn ask(panel: &NSSavePanel, waits: Waits) -> PanelId {
    let id = PanelId(NEXT.get());
    NEXT.set(id.0 + 1);
    let request = request(panel);
    session::notify(Notice::panel_requested(id, request.clone()));
    WAITING.with_borrow_mut(|waiting| {
        waiting.push(Waiting {
            id,
            panel: panel.retain(),
            request,
            waits,
        });
    });
    id
}

/// Keeps `paths` on the panel as its answer, and tells the plugin's response to it.
fn keep(panel: &NSSavePanel, paths: Option<&[PathBuf]>) -> isize {
    let urls: Vec<Retained<NSURL>> = paths
        .unwrap_or_default()
        .iter()
        .map(|path| NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy())))
        .collect();
    let urls = NSArray::from_retained_slice(&urls);
    // SAFETY: the key's address is unique and lives as long as the process; the panel retains
    // the array and releases it with itself.
    unsafe {
        objc2::ffi::objc_setAssociatedObject(
            std::ptr::from_ref(panel).cast_mut().cast(),
            std::ptr::from_ref(&ANSWER).cast(),
            Retained::as_ptr(&urls).cast_mut().cast(),
            OBJC_ASSOCIATION_RETAIN_NONATOMIC,
        );
    }
    if paths.is_some() { OK } else { CANCEL }
}

fn kept(panel: &NSSavePanel) -> Option<&NSArray<NSURL>> {
    // SAFETY: the key holds an array of URLs, if anything; the panel keeps it alive while the
    // caller holds the panel.
    unsafe {
        objc2::ffi::objc_getAssociatedObject(
            std::ptr::from_ref(panel).cast(),
            std::ptr::from_ref(&ANSWER).cast(),
        )
        .cast::<NSArray<NSURL>>()
        .as_ref()
    }
}

extern "C-unwind" fn run_modal(panel: &NSSavePanel, _cmd: Sel) -> isize {
    let id = ask(panel, Waits::Modal(None));
    let answer = loop {
        let answered = WAITING.with_borrow_mut(|waiting| {
            let index = waiting.iter().position(|waiting| waiting.id == id)?;
            match &mut waiting[index].waits {
                Waits::Modal(answer) => answer.take().inspect(|_| {
                    waiting.remove(index);
                }),
                Waits::Handler(_) => None,
            }
        });
        if let Some(answer) = answered {
            break answer;
        }
        // SAFETY: the default mode is a valid mode; the session's commands come in it.
        unsafe { CFRunLoop::run_in_mode(kCFRunLoopDefaultMode, 0.05, true) };
    };
    let response = keep(panel, answer.as_deref());
    session::notify(Notice::panel_finished(id));
    response
}

extern "C-unwind" fn begin(panel: &NSSavePanel, _cmd: Sel, handler: &Block<dyn Fn(isize)>) {
    ask(panel, Waits::Handler(handler.copy()));
}

extern "C-unwind" fn begin_sheet(
    panel: &NSSavePanel,
    _cmd: Sel,
    _window: *mut NSWindow,
    handler: &Block<dyn Fn(isize)>,
) {
    ask(panel, Waits::Handler(handler.copy()));
}

extern "C-unwind" fn url(panel: &NSSavePanel, _cmd: Sel) -> *mut NSURL {
    kept(panel)
        .and_then(|urls| urls.firstObject())
        .map_or(std::ptr::null_mut(), Retained::autorelease_ptr)
}

extern "C-unwind" fn urls(panel: &NSSavePanel, _cmd: Sel) -> *mut NSArray<NSURL> {
    match kept(panel) {
        Some(urls) => Retained::autorelease_ptr(urls.retain()),
        None => Retained::autorelease_ptr(NSArray::new()),
    }
}

/// Replaces the methods for the rest of the process's life.
pub fn install(_mtm: MainThreadMarker) {
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
            };
        };
        let save = NSSavePanel::class();
        // SAFETY: the transmutes keep each function's signature, which its encoding states.
        unsafe {
            replace(
                save,
                sel!(runModal),
                std::mem::transmute::<extern "C-unwind" fn(&NSSavePanel, Sel) -> isize, Imp>(
                    run_modal,
                ),
                c"q@:",
            );
            replace(
                save,
                sel!(beginWithCompletionHandler:),
                std::mem::transmute::<
                    extern "C-unwind" fn(&NSSavePanel, Sel, &Block<dyn Fn(isize)>),
                    Imp,
                >(begin),
                c"v@:@?",
            );
            replace(
                save,
                sel!(beginSheetModalForWindow:completionHandler:),
                std::mem::transmute::<
                    extern "C-unwind" fn(&NSSavePanel, Sel, *mut NSWindow, &Block<dyn Fn(isize)>),
                    Imp,
                >(begin_sheet),
                c"v@:@@?",
            );
            replace(
                save,
                sel!(URL),
                std::mem::transmute::<extern "C-unwind" fn(&NSSavePanel, Sel) -> *mut NSURL, Imp>(
                    url,
                ),
                c"@@:",
            );
            let urls = std::mem::transmute::<
                extern "C-unwind" fn(&NSSavePanel, Sel) -> *mut NSArray<NSURL>,
                Imp,
            >(urls);
            replace(save, sel!(URLs), urls, c"@@:");
            replace(NSOpenPanel::class(), sel!(URLs), urls, c"@@:");
        }
    });
}

/// Whether `paths` answer `request`: a save takes one path, an open one or, if it takes
/// several, more.
fn fits(request: &PanelRequest, paths: Option<&[PathBuf]>) -> bool {
    let Some(paths) = paths else {
        return true;
    };
    match request.kind {
        PanelKind::Save => paths.len() == 1,
        _ => !paths.is_empty() && (request.multiple || paths.len() == 1),
    }
}

/// Answers the panel. Call it from a delivery of its own: a completion handler is plugin code.
pub fn answer(answer: &PanelAnswer) {
    let handler = WAITING.with_borrow_mut(|waiting| {
        let index = waiting
            .iter()
            .position(|waiting| waiting.id == answer.panel)?;
        if !fits(&waiting[index].request, answer.paths.as_deref()) {
            return None;
        }
        match &mut waiting[index].waits {
            Waits::Modal(reply) => {
                *reply = Some(answer.paths.clone());
                None
            }
            Waits::Handler(_) => Some(waiting.remove(index)),
        }
    });
    if let Some(Waiting {
        id,
        panel,
        waits: Waits::Handler(handler),
        ..
    }) = handler
    {
        let response = keep(&panel, answer.paths.as_deref());
        handler.call((response,));
        session::notify(Notice::panel_finished(id));
    }
}

/// The panels waiting for an answer.
pub fn waiting() -> Vec<PanelId> {
    WAITING.with_borrow(|waiting| waiting.iter().map(|waiting| waiting.id).collect())
}

/// Whether the plugin waits in a panel's `runModal`, during which input to its windows is
/// dropped, as the real panel's modal loop drops it.
pub fn modal() -> bool {
    WAITING.with_borrow(|waiting| {
        waiting
            .iter()
            .any(|waiting| matches!(waiting.waits, Waits::Modal(_)))
    })
}

/// Tells a new session the waiting panels.
pub fn tell_all() {
    let requests: Vec<_> = WAITING.with_borrow(|waiting| {
        waiting
            .iter()
            .map(|waiting| Notice::panel_requested(waiting.id, waiting.request.clone()))
            .collect()
    });
    for notice in requests {
        session::notify(notice);
    }
}
