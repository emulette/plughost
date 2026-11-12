//! A chain's slots changed in place: a plugin inserted, replaced, removed or moved without
//! starting another helper, so the other slots' instances, their open editors and a remote
//! session stay. A prepared chain is left unprepared, and the application prepares it again for
//! the slots it now has.
use super::*;
use crate::process::AudioThread;
use plughost_core::{PluginState, StatePurpose};

impl Host {
    pub(super) fn insert_slot(
        &mut self,
        slot: usize,
        plugin: &PluginRef,
        state: Option<&PluginState>,
        bypass: Option<bool>,
    ) -> Response {
        // A position past the slots names no slot the application may be told of.
        if slot > self.slots.len() {
            return Response::Rejected {
                slot: None,
                error: InputError::Slot,
            };
        }
        let candidate = match self.candidate(slot, plugin, state, bypass) {
            Ok(candidate) => candidate,
            Err(response) => return *response,
        };
        self.unprepare();
        self.slots.insert(slot, candidate);
        self.references.insert(slot, plugin.clone());
        self.editors.insert(slot, None);
        self.parameter_events.insert(slot, Default::default());
        self.renumber(|index| Some(if index >= slot { index + 1 } else { index }));
        self.publish_processors();
        self.slot_infos()
    }

    pub(super) fn replace_slot(
        &mut self,
        slot: usize,
        plugin: &PluginRef,
        state: Option<&PluginState>,
        bypass: Option<bool>,
    ) -> Response {
        let candidate = match self.candidate(slot, plugin, state, bypass) {
            Ok(candidate) => candidate,
            Err(response) => return *response,
        };
        self.close_slot_editor(slot);
        self.unprepare();
        let old = std::mem::replace(&mut self.slots[slot], candidate);
        self.references[slot] = plugin.clone();
        // Retire the old instance's processor before the instance is dropped.
        self.publish_processors();
        self.retain_diagnostics(slot, old.as_ref());
        self.invalidate_parameters(slot);
        drop(old);
        self.slot_infos()
    }

    pub(super) fn remove_slot(&mut self, slot: usize) -> Response {
        if self.slots.len() == 1 {
            return Response::Rejected {
                slot: Some(slot),
                error: InputError::EmptyChain,
            };
        }
        self.close_slot_editor(slot);
        // The session is told of the closed editor while the slot still has its number: the
        // panels that wait in the slots after it take that number next, and are not its own.
        #[cfg(target_os = "macos")]
        self.publish_remote();
        self.unprepare();
        let old = self.slots.remove(slot);
        self.references.remove(slot);
        self.editors.remove(slot);
        self.parameter_events.remove(slot);
        self.renumber(|index| match index.cmp(&slot) {
            std::cmp::Ordering::Less => Some(index),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(index - 1),
        });
        self.publish_processors();
        self.retain_diagnostics(slot, old.as_ref());
        drop(old);
        self.slot_infos()
    }

    pub(super) fn move_slot(&mut self, from: usize, to: usize) -> Response {
        let count = self.slots.len();
        if from >= count || to >= count {
            return Response::Rejected {
                slot: None,
                error: InputError::Slot,
            };
        }
        if from == to {
            return self.slot_infos();
        }
        self.unprepare();
        let plugin = self.slots.remove(from);
        self.slots.insert(to, plugin);
        let reference = self.references.remove(from);
        self.references.insert(to, reference);
        let editor = self.editors.remove(from);
        self.editors.insert(to, editor);
        let events = self.parameter_events.remove(from);
        self.parameter_events.insert(to, events);
        self.renumber(|index| {
            Some(if index == from {
                to
            } else if from < to && (from..=to).contains(&index) {
                index - 1
            } else if to < from && (to..=from).contains(&index) {
                index + 1
            } else {
                index
            })
        });
        self.publish_processors();
        self.slot_infos()
    }

    /// A fresh instance of `plugin` for `slot`, restored from `state` and taking `bypass`, or
    /// the response that tells why there is none. The active slots are left as they are.
    fn candidate(
        &mut self,
        slot: usize,
        plugin: &PluginRef,
        state: Option<&PluginState>,
        bypass: Option<bool>,
    ) -> Result<Box<dyn HostedPlugin>, Box<Response>> {
        let calls = self.calls.clone();
        let _call = calls.enter(Caller::Main, slot);
        let mut candidate = self
            .load_slot(slot, plugin, &self.identity)
            .map_err(|error| Box::new(failed(slot, &error)))?;
        if let Some(state) = state
            && !state::same_class(candidate.info(), state)
        {
            self.retain_diagnostics(slot, candidate.as_ref());
            return Err(Box::new(state::mismatch(slot)));
        }
        let applied = state
            .map(|state| candidate.restore_state(state, StatePurpose::Project))
            .transpose()
            .and_then(|_| {
                bypass
                    .map(|enabled| candidate.set_bypass(enabled))
                    .transpose()
            });
        if let Err(error) = applied {
            self.retain_diagnostics(slot, candidate.as_ref());
            return Err(Box::new(failed(slot, &error)));
        }
        // What the plugin reported while loading gives way to its first parameter read.
        let _ = candidate.take_parameter_events();
        Ok(candidate)
    }

    fn close_slot_editor(&mut self, slot: usize) {
        if let Some(window) = self.editors[slot].take() {
            window.close(self.slots[slot].as_mut());
        }
        #[cfg(target_os = "macos")]
        crate::remote::panel::give_up(Some(slot));
    }

    /// Gives each instance its slot again for its audio-thread calls, and moves the shown remote
    /// editor and the waiting panels with their slots, by `moved` from a slot before to its slot
    /// now (`None` for a slot that went).
    fn renumber(&mut self, moved: impl Fn(usize) -> Option<usize>) {
        let processing = Arc::clone(&self.processing);
        for (slot, plugin) in self.slots.iter_mut().enumerate() {
            plugin.set_audio_thread(AudioThread::new(&processing, slot));
        }
        if let Some(remote) = &mut self.remote {
            remote.renumber(&moved);
        }
        #[cfg(target_os = "macos")]
        crate::remote::panel::renumber(&moved);
    }

    /// Hands the processing thread the processors of the slots as they now are. The timing
    /// changes kept by slot start again: the chain is left unprepared, and its next preparation
    /// observes every slot under the number it then has.
    fn publish_processors(&self) {
        drop(self.processing.request(Command::Slots(
            self.slots.iter().map(|plugin| plugin.processor()).collect(),
        )));
    }

    fn slot_infos(&self) -> Response {
        Response::Loaded(
            self.slots
                .iter()
                .map(|plugin| plugin.info().clone())
                .collect(),
        )
    }
}
