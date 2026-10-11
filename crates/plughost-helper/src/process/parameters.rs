//! Parameter metadata and its lookup index are replaced together between blocks.
use std::collections::HashMap;

use plughost_core::{InputError, ParameterInfo};

pub(crate) struct Parameters {
    infos: Vec<ParameterInfo>,
    indices: HashMap<u64, usize>,
}

impl Parameters {
    /// Built on the main thread, alongside native metadata discovery.
    pub fn new(infos: Vec<ParameterInfo>) -> Self {
        let mut indices = HashMap::with_capacity(infos.len());
        for (index, info) in infos.iter().enumerate() {
            indices.entry(info.id).or_insert(index);
        }
        Self { infos, indices }
    }

    pub fn get(&self, id: u64) -> Result<&ParameterInfo, InputError> {
        self.indices
            .get(&id)
            .map(|&index| &self.infos[index])
            .ok_or(InputError::UnknownParameter { id })
    }
}
