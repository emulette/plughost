//! CLAP hosting, through clack-host.

mod errors;
mod host;
mod plugin;

pub use errors::ClapError;
pub use plugin::{Plugin, Processor, classes, classes_once_loaded};

mod transport;

mod discovery;
pub use discovery::{discover_presets, preset_providers};
