use serde::{Deserialize, Serialize};

use crate::InputError;

/// Identity of the application hosting the plugins. VST3 receives the name; CLAP receives all
/// three fields. Audio Units have no equivalent identity callback in the current host API.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostIdentity {
    /// Non-empty, at most 127 UTF-16 code units (the VST3 String128 limit).
    pub name: String,
    pub vendor: String,
    pub version: String,
}

impl HostIdentity {
    /// Validates the common identity without truncating strings at a format boundary.
    pub fn validate(&self) -> Result<(), InputError> {
        if self.name.is_empty()
            || self.name.encode_utf16().count() > 127
            || [&self.name, &self.vendor, &self.version]
                .iter()
                .any(|s| s.contains('\0'))
        {
            return Err(InputError::HostIdentity);
        }
        Ok(())
    }
}

/// Identity for standalone plughost tools and tests. Applications should supply their own.
impl Default for HostIdentity {
    fn default() -> Self {
        Self {
            name: "plughost".to_owned(),
            vendor: "plughost".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}
