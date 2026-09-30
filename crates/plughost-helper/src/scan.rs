//! Scan mode: load one VST3 or CLAP bundle and report its classes, or list the registered Audio Units. Each
//! message is progress, so the application can time out a stall without cutting off bundles with
//! many classes.

use std::io;
use std::path::Path;

use plughost_core::ipc::Response;
use plughost_formats::vst3::Module;

use crate::Responder;

pub fn scan(bundle: &Path, responder: &Responder) -> io::Result<()> {
    let is_clap = bundle
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("clap"));
    let classes = if is_clap {
        plughost_formats::clap::classes(bundle).map_err(plughost_formats::Error::from)
    } else {
        Module::load(bundle)
            .map(|module| module.classes())
            .map_err(plughost_formats::Error::from)
    };
    let classes = match classes {
        Ok(classes) => classes,
        Err(error) => return responder.fail(Some(0), error.failure()),
    };
    responder.send(&Response::ModuleLoaded)?;
    for class in classes {
        responder.send(&Response::Class(class))?;
    }
    responder.send(&Response::Done)
}

/// Reports the Audio Units in the system registry. Reading the registry loads no plugin code.
pub fn list_audio_units(responder: &Responder) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    for class in plughost_formats::au::components() {
        responder.send(&Response::Class(class))?;
    }
    responder.send(&Response::Done)
}
