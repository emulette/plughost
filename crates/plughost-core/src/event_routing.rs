//! A plugin's event ports, and the routes that carry events from a chain's event inputs through
//! its slots to its event outputs.

use serde::{Deserialize, Serialize};

use crate::{AudioDirection, InputError, Support};

/// Maximum external event inputs of a chain, and event routes or collected outputs of one slot.
pub const MAX_EVENT_PORTS: usize = 16;

/// A native event port. IDs are scoped to a direction; `index` is the native position that
/// directly hosted plugins use as a [`Event::port`](crate::Event::port).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventPortInfo {
    pub id: u64,
    pub index: u32,
    pub name: String,
    pub direction: AudioDirection,
    /// MIDI 1.0 channel messages, natively or through the format's note events and parameter
    /// mapping. Which messages a plugin uses remains its own choice.
    pub midi: Support,
    /// System exclusive messages.
    pub sysex: Support,
    /// Notes keep their IDs and the expressions of single notes arrive: CLAP ports that take the
    /// CLAP dialect, and VST3 buses whose controller lists note expressions for them. Pressure
    /// also reaches ports without it, as poly pressure.
    pub note_expression: Support,
    /// MPE: MIDI 1.0 with each note on its own channel, whose pitch bend, channel pressure and
    /// controller 74 shape that note. plughost delivers it as MIDI either way.
    pub mpe: Support,
}

/// Where a slot's event input port takes events from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventSource {
    /// One of the chain's external event inputs, by index.
    External { port: usize },
    /// An event output port of the previous slot, by native ID; it must be one of that slot's
    /// collected outputs.
    Previous { port: u64 },
}

/// One source delivered to a native event input port. Several routes to one port merge in offset
/// order; at equal offsets, events keep the order of the routes, then their source order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventInputRoute {
    pub port: u64,
    pub source: EventSource,
}

/// A slot's event routing. Native event ports not named here are left inactive where the format
/// allows. The last slot's collected outputs, in this order, are the chain's event outputs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotEventConfig {
    pub inputs: Vec<EventInputRoute>,
    /// Native event output ports whose events are collected.
    pub outputs: Vec<u64>,
}

/// The event ports a slot's native preparation activates.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventConfig {
    pub inputs: Vec<u64>,
    pub outputs: Vec<u64>,
}

impl SlotEventConfig {
    /// The native input ports routes deliver to, each once, in route order.
    pub fn native(&self) -> EventConfig {
        let mut inputs: Vec<u64> = Vec::new();
        for route in &self.inputs {
            if !inputs.contains(&route.port) {
                inputs.push(route.port);
            }
        }
        EventConfig {
            inputs,
            outputs: self.outputs.clone(),
        }
    }

    /// Checks the counts, duplicate outputs, and each route's source; `previous` is the previous
    /// slot's routing, if any.
    pub(crate) fn validate(
        &self,
        slot: usize,
        event_inputs: usize,
        previous: Option<&SlotEventConfig>,
    ) -> Result<(), InputError> {
        if self.inputs.len() > MAX_EVENT_PORTS || self.outputs.len() > MAX_EVENT_PORTS {
            return Err(InputError::EventPortCount);
        }
        for (index, port) in self.outputs.iter().enumerate() {
            if self.outputs[..index].contains(port) {
                return Err(InputError::DuplicateEventPort { id: *port });
            }
        }
        for (index, route) in self.inputs.iter().enumerate() {
            let available = match route.source {
                EventSource::External { port } => port < event_inputs,
                EventSource::Previous { port } => {
                    previous.is_some_and(|previous| previous.outputs.contains(&port))
                }
            };
            if !available || self.inputs[..index].contains(route) {
                return Err(InputError::EventRoute {
                    slot,
                    port: route.port,
                });
            }
        }
        Ok(())
    }
}
