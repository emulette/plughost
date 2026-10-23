//! Event routing between the chain's event inputs, its slots and its event outputs. The plan
//! resolves native port IDs to the indices processors use, on the control thread; blocks reuse
//! storage reserved with it.
use plughost_core::{
    AudioDirection, EventPortInfo, EventSource, Events, InputError, RoutedChainConfig,
    validate_event_budget,
};

use crate::errors::EVENT_PORT_PLAN;

enum Source {
    External(usize),
    /// A native output index of the previous slot.
    Previous(usize),
}

struct SlotPlan {
    /// Native input index and its source, in route order.
    inputs: Vec<(usize, Source)>,
    /// Native output indices collected, in the order of the requested output IDs.
    outputs: Vec<usize>,
}

pub(crate) struct EventPlan {
    slots: Vec<SlotPlan>,
}

/// Per-slot events of one block: what each slot receives and what it produced, and the chain's
/// output. Capacity for a full block is reserved when the plan is made.
pub(crate) struct EventBuffers {
    pub inputs: Vec<Events>,
    pub produced: Vec<Events>,
    pub output: Events,
}

impl EventPlan {
    /// Plans validated routing against each prepared slot's native event ports.
    pub fn new(config: &RoutedChainConfig, ports: &[Vec<EventPortInfo>]) -> Result<Self, String> {
        let index = |slot: usize, direction, id: u64| {
            ports[slot]
                .iter()
                .find(|port| port.direction == direction && port.id == id)
                .map(|port| port.index as usize)
                .ok_or_else(|| format!("{EVENT_PORT_PLAN} (slot {slot}, port {id})"))
        };
        let slots = config
            .slots
            .iter()
            .enumerate()
            .map(|(slot, setup)| {
                let inputs = setup
                    .events
                    .inputs
                    .iter()
                    .map(|route| {
                        let source = match route.source {
                            EventSource::External { port } => Source::External(port),
                            EventSource::Previous { port } => {
                                Source::Previous(index(slot - 1, AudioDirection::Output, port)?)
                            }
                            _ => return Err(format!("{EVENT_PORT_PLAN} (slot {slot})")),
                        };
                        Ok((index(slot, AudioDirection::Input, route.port)?, source))
                    })
                    .collect::<Result<_, String>>()?;
                let outputs = setup
                    .events
                    .outputs
                    .iter()
                    .map(|&port| index(slot, AudioDirection::Output, port))
                    .collect::<Result<_, String>>()?;
                Ok(SlotPlan { inputs, outputs })
            })
            .collect::<Result<_, String>>()?;
        Ok(Self { slots })
    }

    pub fn buffers(&self) -> EventBuffers {
        let block = Events::for_block;
        EventBuffers {
            inputs: self.slots.iter().map(|_| block()).collect(),
            produced: self.slots.iter().map(|_| block()).collect(),
            output: block(),
        }
    }

    /// The events `slot` receives on its native input indices: each route's source events in
    /// route order, then ordered by offset, keeping that order at equal offsets. `previous` is
    /// what the previous slot produced in this block. They share the block's budget with the
    /// slot's `automation` points.
    pub fn gather(
        &self,
        slot: usize,
        chain: &Events,
        previous: &Events,
        automation: usize,
        into: &mut Events,
    ) -> Result<(), InputError> {
        into.clear();
        for (native, source) in &self.slots[slot].inputs {
            let (events, port) = match *source {
                Source::External(port) => (chain, port),
                Source::Previous(port) => (previous, port),
            };
            for event in events.iter().filter(|event| event.port == port) {
                into.push(event.on_port(*native))?;
            }
        }
        validate_event_budget(automation, into)?;
        into.sort_by_offset();
        Ok(())
    }

    /// The chain's output events: the last slot's collected outputs, numbered by their position
    /// in the requested outputs.
    pub fn output(&self, produced: &Events, into: &mut Events) {
        into.clear();
        let outputs = &self.slots[self.slots.len() - 1].outputs;
        for event in produced {
            if let Some(port) = outputs.iter().position(|&native| native == event.port) {
                // A subset of one block's events fits one block's room.
                let _ = into.push(event.on_port(port));
            }
        }
    }
}
