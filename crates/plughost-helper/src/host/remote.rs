//! Remote editors of a helper started in remote mode: the region they show in and which of them
//! is shown. Each slot's editor can stay open while another is shown; hidden ones keep their
//! state.

use plughost_core::InputError;
use plughost_core::ipc::Response;
use plughost_core::remote::{EditorId, Opened, Region};
use plughost_core::{Failure, FailureKind};

use super::Host;
use crate::editor::{self, EditorWindow};
use crate::errors::{EXTENSION_EDITOR, NOT_REMOTE};

/// The remote editors of a helper started in remote mode.
#[derive(Default)]
pub(super) struct RemoteEditors {
    /// Where they show, once the application set it.
    region: Option<Region>,
    /// The slot whose editor was shown last; it may have closed since.
    shown: Option<usize>,
    /// The identifier the next editor that opens takes.
    next: u64,
}

impl RemoteEditors {
    /// Follows the shown editor's slot to where it moved, by `moved` from a slot before to its
    /// slot now; one whose slot went shows no editor.
    pub(super) fn renumber(&mut self, moved: impl Fn(usize) -> Option<usize>) {
        self.shown = self.shown.and_then(moved);
    }
}

fn unsupported(slot: Option<usize>, message: &str) -> Response {
    Response::Failed {
        slot,
        failure: Failure::new(FailureKind::Unsupported, message),
    }
}

fn rejected(slot: Option<usize>, error: InputError) -> Response {
    Response::Rejected { slot, error }
}

fn opened(window: &EditorWindow, id: EditorId) -> Response {
    let (width, height) = window.size();
    Response::RemoteEditorOpened(Opened::new(id, f64::from(width), f64::from(height)))
}

impl Host {
    /// Sets where remote editors show and moves those open there. Nothing changes when the
    /// region does not lie within an existing display.
    pub(super) fn set_remote_region(&mut self, region: Region) -> Response {
        let Some(remote) = &mut self.remote else {
            return unsupported(None, NOT_REMOTE);
        };
        if !region.is_well_formed() || !editor::region_fits(&region) {
            return rejected(None, InputError::RemoteRegion);
        }
        remote.region = Some(region);
        for window in self.editors.iter_mut().flatten() {
            window.set_region(region);
        }
        Response::Done
    }

    /// Opens the slot's editor remotely, or finds it open, and shows it.
    pub(super) fn open_remote_editor(&mut self, slot: usize) -> Response {
        let Some(remote) = &mut self.remote else {
            return unsupported(Some(slot), NOT_REMOTE);
        };
        let Some(region) = remote.region else {
            return rejected(Some(slot), InputError::RemoteRegion);
        };
        if let Some(window) = &self.editors[slot]
            && let Some(id) = window.remote_id()
        {
            let response = opened(window, id);
            self.show_remote(slot);
            return response;
        }
        if self.slots[slot].editor_in_extension() {
            return unsupported(Some(slot), EXTENSION_EDITOR);
        }
        let id = EditorId(remote.next);
        match EditorWindow::open_remote(self.slots[slot].as_mut(), region, id) {
            Ok(window) => {
                remote.next += 1;
                let response = opened(&window, id);
                self.editors[slot] = Some(window);
                self.show_remote(slot);
                response
            }
            Err(failure) => Response::Failed {
                slot: Some(slot),
                failure,
            },
        }
    }

    /// Shows the slot's open remote editor.
    pub(super) fn show_remote_editor(&mut self, slot: usize) -> Response {
        if self.remote.is_none() {
            return unsupported(Some(slot), NOT_REMOTE);
        }
        if self.editors[slot]
            .as_ref()
            .and_then(EditorWindow::remote_id)
            .is_none()
        {
            return rejected(Some(slot), InputError::RemoteEditor);
        }
        self.show_remote(slot);
        Response::Done
    }

    /// The slot whose remote editor is shown, if it is still open.
    pub(super) fn shown_remote(&self) -> Option<usize> {
        let shown = self.remote.as_ref()?.shown?;
        self.editors.get(shown)?.as_ref().map(|_| shown)
    }

    /// Refuses a remote session of a helper not in remote mode, first taking the pipes the
    /// application transferred, which the request owns either way.
    #[cfg(target_os = "macos")]
    pub(super) fn remote_session(&mut self, commands: u64, notices: u64) -> Response {
        for channel in [commands, notices] {
            if let Err(error) = self.transfer.receive_channel(channel) {
                return Response::Failed {
                    slot: None,
                    failure: Failure::new(FailureKind::Protocol, error.to_string()),
                };
            }
        }
        unsupported(None, NOT_REMOTE)
    }

    /// Records the open remote editors for the session's watch, which tells it of the editors
    /// that closed and follows the shown one's windows.
    #[cfg(target_os = "macos")]
    pub(super) fn publish_remote(&self) {
        if self.remote.is_none() {
            return;
        }
        let shown_slot = self.shown_remote();
        let mut editors = Vec::new();
        let mut shown = None;
        let mut others = Vec::new();
        for (slot, window) in self.editors.iter().enumerate() {
            let Some(window) = window else {
                continue;
            };
            match (window.remote_id(), window.region()) {
                (Some(id), Some(region)) if Some(slot) == shown_slot => {
                    editors.push((id, slot));
                    shown = Some(crate::remote::watch::Shown {
                        id,
                        window: window.native(),
                        region,
                    });
                }
                (Some(id), _) => {
                    editors.push((id, slot));
                    others.push(window.native());
                }
                (None, _) => others.push(window.native()),
            }
        }
        crate::remote::watch::update(editors, shown, others);
    }

    /// Shows the slot's remote editor and hides the one shown before.
    fn show_remote(&mut self, slot: usize) {
        let previous = self.shown_remote();
        let Some(remote) = &mut self.remote else {
            return;
        };
        remote.shown = Some(slot);
        if let Some(previous) = previous.filter(|previous| *previous != slot)
            && let Some(window) = &mut self.editors[previous]
        {
            window.hide(self.slots[previous].as_mut());
        }
        if let Some(window) = &mut self.editors[slot] {
            window.show(self.slots[slot].as_mut());
        }
    }
}
