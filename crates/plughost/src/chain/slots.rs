use std::borrow::Cow;

use plughost_core::ipc::{Request, Response};

use super::{Chain, ChainSlot, validate_slot};
use crate::{Error, InputError, Tail};

impl Chain {
    /// Loads `plugin`'s class into a new slot at `slot` (at most the slot count; the slot count
    /// appends), restoring its state and taking its bypass before it goes in, as
    /// [`Chain::spawn`] does. The helper and the other slots' instances stay, with their states,
    /// open editors and a remote session, and slot numbers from `slot` on move one up. A prepared
    /// chain is left unprepared, its realtime endpoint gone with its preparation: prepare it
    /// again for the slots it now has. A plugin that cannot load or take its state leaves the
    /// chain as it was. Refused with [`Error::RealtimeRunning`] while the endpoint runs.
    pub fn insert_slot(&mut self, slot: usize, plugin: &ChainSlot) -> Result<(), Error> {
        self.refuse_while_realtime()?;
        self.check_position(slot, self.plugins.len())?;
        validate_slot(slot, plugin)?;
        let request = Request::InsertSlot {
            slot,
            plugin: plugin.plugin.clone(),
            state: plugin.state.as_ref().map(Cow::Borrowed),
            bypass: plugin.bypass,
        };
        self.change_slots(request)?;
        self.plugins.insert(slot, plugin.plugin.clone());
        Ok(())
    }

    /// Loads `plugin`'s class in place of the slot's plugin, as [`Chain::insert_slot`] loads
    /// one; the replaced plugin's editor closes. A plugin that cannot load or take its state
    /// leaves the slot's plugin, and the chain, as they were.
    pub fn replace_slot(&mut self, slot: usize, plugin: &ChainSlot) -> Result<(), Error> {
        self.refuse_while_realtime()?;
        validate_slot(slot, plugin)?;
        let request = Request::ReplaceSlot {
            slot,
            plugin: plugin.plugin.clone(),
            state: plugin.state.as_ref().map(Cow::Borrowed),
            bypass: plugin.bypass,
        };
        self.change_slots(request)?;
        self.plugins[slot] = plugin.plugin.clone();
        Ok(())
    }

    /// Removes the slot and its plugin, whose editor closes; slot numbers after it move one
    /// down. The others stay as [`Chain::insert_slot`] keeps them. The last slot is not removed
    /// (`InputError::EmptyChain`).
    pub fn remove_slot(&mut self, slot: usize) -> Result<(), Error> {
        self.refuse_while_realtime()?;
        self.change_slots(Request::RemoveSlot { slot })?;
        self.plugins.remove(slot);
        Ok(())
    }

    /// Moves the slot's plugin to `to`, with its state and open editor; the slots
    /// between move one toward `from`. The plugins stay loaded, as [`Chain::insert_slot`] keeps
    /// them, and the chain is left unprepared, unless `from` is `to`.
    pub fn move_slot(&mut self, from: usize, to: usize) -> Result<(), Error> {
        self.refuse_while_realtime()?;
        let last = self.plugins.len() - 1;
        self.check_position(from, last)?;
        self.check_position(to, last)?;
        self.change_slots(Request::MoveSlot { from, to })?;
        let plugin = self.plugins.remove(from);
        self.plugins.insert(to, plugin);
        Ok(())
    }

    /// Refuses a position past `last` before it reaches the helper.
    fn check_position(&self, position: usize, last: usize) -> Result<(), Error> {
        if position > last {
            return Err(Error::Input {
                slot: Some(position),
                error: InputError::Slot,
            });
        }
        Ok(())
    }

    /// Sends a change of the slots and takes the plugins the helper now holds; the helper left
    /// the chain unprepared.
    fn change_slots(&mut self, request: Request<'_>) -> Result<(), Error> {
        let moved_in_place = matches!(request, Request::MoveSlot { from, to } if from == to);
        match self.request(request, self.timeouts.load)? {
            Response::Loaded(infos) => {
                self.helper.set_slots(infos.len());
                self.infos = infos;
                if !moved_in_place {
                    self.shared = None;
                    self.config = None;
                    self.realtime = None;
                    self.latency = 0;
                    self.tail = Tail::Samples(0);
                }
                Ok(())
            }
            _ => Err(self.protocol_error()),
        }
    }
}
