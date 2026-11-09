//! AppKit menus a plugin opens. Posted clicks do not choose their items: AppKit tracks a menu in
//! a loop of its own that reads the window server's events. The session is told each menu that
//! begins tracking and its items, and chooses an item by its index path, whose action runs once
//! the loop ended, or closes the menu. A menu with nothing to close it holds the main thread, so
//! one open past the session's menu timeout is closed, and the end of the session closes its
//! menus.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::sync::Once;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSControlStateValueMixed, NSControlStateValueOn, NSMenu, NSMenuDidBeginTrackingNotification,
    NSMenuDidEndTrackingNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter};
use plughost_core::remote::{
    MenuChoice, MenuId, MenuItem, MenuItemState, Notice, Rect, Window, WindowKind,
};

use super::session;
use crate::editor;

/// How long a menu waits for its window to show before the session is told it without a frame.
const FRAME_WAIT: Duration = Duration::from_millis(500);

struct Open {
    id: MenuId,
    menu: Retained<NSMenu>,
    opened: Instant,
    /// The menu windows on screen when it began tracking, none of them its own.
    before: Vec<u64>,
    /// Its window's frame once the session was told the menu: `Some(None)` when the window did
    /// not show in time, `None` before.
    told: Option<Option<Rect>>,
}

/// An item chosen in an open menu, run once that menu's tracking ended.
struct Chosen {
    root: Retained<NSMenu>,
    menu: Retained<NSMenu>,
    index: isize,
}

thread_local! {
    static OPEN: RefCell<Vec<Open>> = const { RefCell::new(Vec::new()) };
    static CHOSEN: RefCell<Option<Chosen>> = const { RefCell::new(None) };
    static NEXT: Cell<u64> = const { Cell::new(1) };
    /// How long a menu stays open without a choice.
    static TIMEOUT: Cell<Duration> = const { Cell::new(Duration::MAX) };
}

/// Follows the menus that begin and end tracking, for the rest of the process's life.
pub fn install(_mtm: MainThreadMarker) {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let began = RcBlock::new(|notification: NonNull<NSNotification>| {
            if let Some(menu) = menu_of(notification) {
                began(menu);
            }
        });
        let ended = RcBlock::new(|notification: NonNull<NSNotification>| {
            if let Some(menu) = menu_of(notification) {
                ended(&menu);
            }
        });
        let center = NSNotificationCenter::defaultCenter();
        // SAFETY: the notifications name menus; the observers live as long as the process.
        unsafe {
            for (name, block) in [
                (NSMenuDidBeginTrackingNotification, &began),
                (NSMenuDidEndTrackingNotification, &ended),
            ] {
                let observer = center.addObserverForName_object_queue_usingBlock(
                    Some(name),
                    None,
                    None,
                    block,
                );
                std::mem::forget(observer);
            }
        }
    });
}

fn menu_of(notification: NonNull<NSNotification>) -> Option<Retained<NSMenu>> {
    // SAFETY: the center hands a valid notification, on the thread that posted it.
    let notification = unsafe { notification.as_ref() };
    MainThreadMarker::new()?;
    notification.object()?.downcast::<NSMenu>().ok()
}

/// Whether `menu` is one of `open`'s submenus, at any depth.
fn within(menu: &NSMenu, open: &NSMenu) -> bool {
    // SAFETY: each supermenu is retained as it is read, while its submenu holds it.
    let mut parent = unsafe { menu.supermenu() };
    while let Some(menu) = parent {
        if &*menu == open {
            return true;
        }
        parent = unsafe { menu.supermenu() };
    }
    false
}

fn began(menu: Retained<NSMenu>) {
    let known = OPEN.with_borrow(|open| {
        open.iter()
            .any(|open| open.menu == menu || within(&menu, &open.menu))
    });
    if known {
        return;
    }
    let id = MenuId(NEXT.get());
    NEXT.set(id.0 + 1);
    // Its window shows after tracking began; the session is told the menu with that window's
    // frame (see `tell_opened`).
    OPEN.with_borrow_mut(|open| {
        open.push(Open {
            id,
            menu,
            opened: Instant::now(),
            before: editor::menu_windows(),
            told: None,
        });
    });
}

/// Tells the session the menus that began tracking, once each one's window shows among the
/// shown editor's `windows` or [`FRAME_WAIT`] passed. Its window is the back-most menu window
/// that was not on screen when it began: AppKit may open a submenu in front of it at once.
pub fn tell_opened(windows: &[Window]) {
    let notices: Vec<Notice> = OPEN.with_borrow_mut(|open| {
        open.iter_mut()
            .filter(|open| open.told.is_none())
            .filter_map(|open| {
                let frame = windows
                    .iter()
                    .rfind(|window| {
                        window.kind == WindowKind::Menu && !open.before.contains(&window.id)
                    })
                    .map(|window| window.frame);
                if frame.is_none() && open.opened.elapsed() < FRAME_WAIT {
                    return None;
                }
                open.told = Some(frame);
                Some(Notice::menu_opened(open.id, items(&open.menu, 0), frame))
            })
            .collect()
    });
    for notice in notices {
        session::notify(notice);
    }
}

fn ended(menu: &NSMenu) {
    // A menu the session was never told closes without telling it.
    let closed = OPEN.with_borrow_mut(|open| {
        let index = open.iter().position(|open| &*open.menu == menu)?;
        let closed = open.remove(index);
        closed.told.map(|_| closed.id)
    });
    if let Some(id) = closed {
        session::notify(Notice::menu_closed(id));
    }
    let chosen = CHOSEN.with_borrow_mut(|chosen| {
        chosen
            .take_if(|chosen| &*chosen.root == menu)
            .map(|chosen| (chosen.menu, chosen.index))
    });
    if let Some((menu, index)) = chosen {
        menu.performActionForItemAtIndex(index);
    }
}

/// The menu's items but the hidden ones, each with its index among all of them, `depth`
/// submenus down. Submenus past the protocol's depth are left out.
fn items(menu: &NSMenu, depth: usize) -> Vec<MenuItem> {
    menu.itemArray()
        .iter()
        .enumerate()
        .filter(|(_, item)| !item.isHidden())
        .map(|(index, item)| {
            let title = item.title().to_string();
            let title = if title.is_empty() {
                item.attributedTitle()
                    .map(|title| title.string().to_string())
                    .unwrap_or_default()
            } else {
                title
            };
            let state = match item.state() {
                state if state == NSControlStateValueOn => MenuItemState::On,
                state if state == NSControlStateValueMixed => MenuItemState::Mixed,
                _ => MenuItemState::Off,
            };
            let submenu = item
                .submenu()
                .filter(|_| depth < plughost_core::ipc::MAX_MENU_DEPTH)
                .map(|submenu| items(&submenu, depth + 1))
                .unwrap_or_default();
            MenuItem::new(
                index,
                title,
                item.isEnabled(),
                state,
                item.isSeparatorItem(),
                submenu,
            )
        })
        .collect()
}

fn open_menu(id: MenuId) -> Option<Retained<NSMenu>> {
    OPEN.with_borrow(|open| {
        open.iter()
            .find(|open| open.id == id)
            .map(|open| open.menu.clone())
    })
}

/// Chooses the item `choice` names: closes its menu and, once the menu's tracking ended, runs the
/// item's action, as AppKit does, so an action that runs a panel's or a dialog's loop does not
/// leave the menu up. Call it from a delivery of its own.
pub fn choose(choice: &MenuChoice) {
    let Some(root) = open_menu(choice.menu) else {
        return;
    };
    let Some((&last, leading)) = choice.path.split_last() else {
        return;
    };
    let mut menu = root.clone();
    for &index in leading {
        let Some(submenu) = item_at(&menu, index).and_then(|item| item.submenu()) else {
            return;
        };
        menu = submenu;
    }
    let Some(item) = item_at(&menu, last) else {
        return;
    };
    if !item.isEnabled() || item.isSeparatorItem() || item.hasSubmenu() || item.isHidden() {
        return;
    }
    CHOSEN.set(Some(Chosen {
        root: root.clone(),
        menu,
        index: last as isize,
    }));
    root.cancelTracking();
}

fn item_at(menu: &NSMenu, index: usize) -> Option<Retained<objc2_app_kit::NSMenuItem>> {
    (index < menu.numberOfItems() as usize)
        .then(|| menu.itemAtIndex(index as isize))
        .flatten()
}

/// Closes the menu without choosing.
pub fn close(id: MenuId) {
    if let Some(menu) = open_menu(id) {
        menu.cancelTracking();
    }
}

/// Closes every open menu: the session that could close them ended.
pub fn close_all() {
    let menus: Vec<_> =
        OPEN.with_borrow(|open| open.iter().map(|open| open.menu.clone()).collect());
    for menu in menus {
        menu.cancelTracking();
    }
}

/// Sets how long a menu stays open without a choice.
pub fn set_timeout(timeout: Duration) {
    TIMEOUT.set(timeout);
}

/// Closes the menus open past the timeout.
pub fn expire() {
    let timeout = TIMEOUT.get();
    let expired: Vec<_> = OPEN.with_borrow(|open| {
        open.iter()
            .filter(|open| open.opened.elapsed() > timeout)
            .map(|open| open.menu.clone())
            .collect()
    });
    for menu in expired {
        menu.cancelTracking();
    }
}

/// Tells a new session the open menus it was told; the others are told as their windows show.
pub fn tell_all() {
    let open: Vec<_> = OPEN.with_borrow(|open| {
        open.iter()
            .filter_map(|open| Some((open.id, open.menu.clone(), open.told?)))
            .collect()
    });
    for (id, menu, frame) in open {
        session::notify(Notice::menu_opened(id, items(&menu, 0), frame));
    }
}
