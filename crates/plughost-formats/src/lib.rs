//! In-process VST3, Audio Unit, and CLAP hosting for plughost.
//!
//! Plugins are loaded into the calling process. Load plugins, change their lifecycle, and use their
//! controllers on one thread, normally the main thread; plugins that show dialogs or editors need
//! that thread to run the OS event loop. Applications that need crash isolation use the plughost
//! crate, which runs these hosts in helper processes.
//!
//! [`load`] creates a plugin of any enabled format from a `PluginRef`; each format's `Plugin` can
//! also be created directly. Every plugin is used through [`HostedPlugin`] and processed from
//! another thread through the [`BlockProcessor`] it hands out. Format-specific failures are
//! variants of [`Error`].

#[cfg(all(feature = "au", target_os = "macos"))]
pub mod au;
#[cfg(feature = "clap")]
pub mod clap;
mod errors;
mod hosted;
mod parameter_choices;
pub mod preset;
#[cfg(feature = "clap")]
mod state_stream;
#[cfg(feature = "vst3")]
pub mod vst3;

pub use errors::Error;
pub use hosted::{BlockProcessor, EditorRequest, EditorView, HostedPlugin, ResizeRequest, load};
