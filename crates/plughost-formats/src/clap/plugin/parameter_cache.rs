//! The plugin's parameter list, read again only after the plugin rescans its parameter info
//! (`params.rescan` with `INFO` or `ALL`).

use std::collections::HashMap;

use clack_extensions::params::{ParamInfoBuffer, ParamInfoFlags, PluginParams};
use clack_host::prelude::{ClapId, PluginMainThreadHandle};
use plughost_core::{ParameterFlags, ParameterInfo};

pub(super) struct Parameter {
    pub info: ParameterInfo,
    pub native: ClapId,
    pub min: f64,
    pub max: f64,
    pub module: String,
}

impl Parameter {
    /// The plain value for a normalized one. Stepped parameters take whole values.
    pub fn plain(&self, normalized: f64) -> f64 {
        let plain = self.min + normalized * (self.max - self.min);
        if self.info.flags.discrete {
            plain.round()
        } else {
            plain
        }
    }

    pub fn normalized(&self, plain: f64) -> f64 {
        if self.max > self.min {
            (plain - self.min) / (self.max - self.min)
        } else {
            0.0
        }
    }
}

#[derive(Default)]
pub(super) struct ParameterCache {
    parameters: Vec<Parameter>,
    index: HashMap<u64, usize>,
}

impl ParameterCache {
    /// Reads every parameter the plugin describes. Call on the main thread.
    pub fn read(params: Option<PluginParams>, handle: &PluginMainThreadHandle) -> ParameterCache {
        let Some(params) = params else {
            return ParameterCache::default();
        };
        let mut buffer = ParamInfoBuffer::new();
        let parameters: Vec<Parameter> = (0..params.count(handle))
            .filter_map(|index| {
                let info = params.get_info(handle, index, &mut buffer)?;
                let flags = info.flags;
                let stepped = flags.contains(ParamInfoFlags::IS_STEPPED);
                let name = String::from_utf8_lossy(info.name).into_owned();
                let mut parameter = Parameter {
                    info: {
                        let short_title = name.chars().take(8).collect();
                        let mut parameter_info = ParameterInfo::new(u64::from(info.id.get()), name);
                        parameter_info.short_title = short_title;
                        parameter_info.step_count = if stepped {
                            (info.max_value - info.min_value).round().max(0.0) as u32
                        } else {
                            0
                        };
                        parameter_info.flags = {
                            let mut parameter_flags = ParameterFlags::default();
                            parameter_flags.discrete = stepped;
                            parameter_flags.automatable =
                                flags.contains(ParamInfoFlags::IS_AUTOMATABLE);
                            parameter_flags.read_only = flags.contains(ParamInfoFlags::IS_READONLY);
                            parameter_flags.hidden = flags.contains(ParamInfoFlags::IS_HIDDEN);
                            parameter_flags.bypass = flags.contains(ParamInfoFlags::IS_BYPASS);
                            parameter_flags.list = flags.contains(ParamInfoFlags::IS_ENUM);
                            parameter_flags
                        };
                        parameter_info
                    },
                    native: info.id,
                    min: info.min_value,
                    max: info.max_value,
                    module: String::from_utf8_lossy(info.module).into_owned(),
                };
                parameter.info.default_value = Some(parameter.normalized(info.default_value));
                Some(parameter)
            })
            .collect();
        let index = parameters
            .iter()
            .enumerate()
            .map(|(index, parameter)| (parameter.info.id, index))
            .collect();
        ParameterCache { parameters, index }
    }

    pub fn get(&self, id: u64) -> Option<&Parameter> {
        self.index.get(&id).map(|&index| &self.parameters[index])
    }

    pub fn iter(&self) -> impl Iterator<Item = &Parameter> {
        self.parameters.iter()
    }
}
