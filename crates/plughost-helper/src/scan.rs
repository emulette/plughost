//! Scan mode: load one VST3 or CLAP bundle and report its classes, or list the registered Audio Units. Each
//! message is progress, so loading the module and the work after it each get the application's
//! stall timeout, and a slow load does not cut off its enumeration and unloading.

use std::io;
use std::path::Path;

use plughost_core::HostIdentity;
use plughost_core::ipc::Response;
use plughost_formats::vst3::Module;

use crate::Responder;

pub fn scan(bundle: &Path, host: &HostIdentity, responder: &Responder) -> io::Result<()> {
    let is_clap = bundle
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("clap"));
    if is_clap {
        let mut loaded = Ok(());
        let classes = plughost_formats::clap::classes_once_loaded(bundle, || {
            loaded = responder.send(&Response::ModuleLoaded);
        });
        loaded?;
        match classes {
            Ok(classes) => send(classes, responder)?,
            Err(error) => {
                return responder.fail(None, plughost_formats::Error::from(error).failure());
            }
        }
    } else {
        let module = match Module::load_with_host(bundle, host) {
            Ok(module) => module,
            Err(error) => {
                return responder.fail(None, plughost_formats::Error::from(error).failure());
            }
        };
        responder.send(&Response::ModuleLoaded)?;
        send(module.classes(), responder)?;
        // The module unloads after its classes are reported.
        drop(module);
    }
    responder.send(&Response::Done)
}

fn send(classes: Vec<plughost_core::PluginInfo>, responder: &Responder) -> io::Result<()> {
    for class in classes {
        responder.send(&Response::Class(class))?;
    }
    Ok(())
}

/// Reports the Audio Units in the system registry. Reading the registry loads no plugin code.
pub fn list_audio_units(responder: &Responder) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    for class in plughost_formats::au::components() {
        responder.send(&Response::Class(class))?;
    }
    responder.send(&Response::Done)
}
