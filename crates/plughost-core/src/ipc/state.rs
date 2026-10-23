//! Plugin states in the helper protocol. This form changes with [`super::PROTOCOL_VERSION`]; the
//! persisted form of `PluginState::to_bytes` does not depend on it.

use std::borrow::Cow;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::plugin::PluginId;
use crate::state::PluginState;

#[derive(Serialize)]
struct Outgoing<'a> {
    id: &'a PluginId,
    name: &'a str,
    vendor: &'a str,
    version: &'a str,
    component: &'a [u8],
    controller: Option<&'a [u8]>,
}

#[derive(Deserialize)]
struct Incoming {
    id: PluginId,
    name: String,
    vendor: String,
    version: String,
    component: Vec<u8>,
    controller: Option<Vec<u8>>,
}

pub fn serialize<S: Serializer>(state: &PluginState, serializer: S) -> Result<S::Ok, S::Error> {
    Outgoing {
        id: &state.id,
        name: &state.name,
        vendor: &state.vendor,
        version: &state.version,
        component: &state.component,
        controller: state.controller.as_deref(),
    }
    .serialize(serializer)
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PluginState, D::Error> {
    let state = Incoming::deserialize(deserializer)?;
    Ok(PluginState {
        id: state.id,
        name: state.name,
        vendor: state.vendor,
        version: state.version,
        component: state.component,
        controller: state.controller,
    })
}

/// A state a request borrows from the caller until it is sent; received, it is owned.
pub fn deserialize_cow<'de, 'a, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Cow<'a, PluginState>, D::Error> {
    deserialize(deserializer).map(Cow::Owned)
}
