//! The application's realtime endpoint, from the main thread: installing its ring for the
//! prepared chain, and starting, resynchronizing and stopping the worker. While the worker runs,
//! the requests that would reconfigure the chain under it are refused; the application refuses
//! them first. Replacing one slot's instance goes on between quanta.

use super::*;
use crate::errors::{
    REALTIME_NOT_INSTALLED, REALTIME_NOT_RUNNING, REALTIME_RING, REALTIME_RUNNING,
};
use crate::process::realtime::{Realtime, Transport};
use plughost_core::ipc::shared::realtime::RingDescriptor;

impl Host {
    pub(super) fn install_realtime(&mut self, ring: RingDescriptor, tokens: &[u64]) -> Response {
        // Received first, so that a refused ring's handles are not left behind.
        // SAFETY: the application transfers its ring's handles with this request, once.
        let received = unsafe { self.transfer.receive_ring(ring, tokens) };
        let ring = match received {
            Ok(ring) => ring,
            Err(error) => return failure(FailureKind::Protocol, error.to_string()),
        };
        if self.realtime_running {
            return failure(FailureKind::InvalidInput, REALTIME_RUNNING);
        }
        let Some(config) = &self.audio_config else {
            return failure(
                FailureKind::NotPrepared,
                plughost_core::messages::CHAIN_NOT_PREPARED,
            );
        };
        let slots = ring.descriptor().config;
        if slots.sample_format != config.sample_format
            || slots.input_channels != config.input_channels()
            || slots.output_channels != config.output_channels()
            || slots.max_frames > config.max_block_size
        {
            return failure(FailureKind::Configuration, REALTIME_RING);
        }
        let transport = match Transport::new(ring) {
            Ok(transport) => transport,
            Err(error) => return failure(FailureKind::Host, error.to_string()),
        };
        self.processing.set_bell(Some(transport.ring().control()));
        drop(
            self.processing
                .request(Command::Realtime(Realtime::Install(Some(Box::new(
                    transport,
                ))))),
        );
        self.realtime_installed = true;
        Response::Done
    }

    pub(super) fn start_realtime(&mut self) -> Response {
        if !self.realtime_installed {
            return failure(FailureKind::NotPrepared, REALTIME_NOT_INSTALLED);
        }
        if !self.realtime_running {
            drop(self.processing.request(Command::Realtime(Realtime::Start)));
            self.realtime_running = true;
        }
        Response::Done
    }

    pub(super) fn stop_realtime(&mut self) -> Response {
        if self.realtime_running {
            drop(self.processing.request(Command::Realtime(Realtime::Stop)));
            self.realtime_running = false;
        }
        Response::Done
    }

    /// Resynchronizes the running worker without stopping it. The application's callback no
    /// longer submits, so once the worker adopts the new generation it takes no quantum of the
    /// old one; it does so before the slots are reset, so none is processed between the resets.
    /// The slots are then reset and the alignment planned again as by `Reset`, which also ends
    /// the notes they hold, and the timing is the new generation's.
    pub(super) fn resync_realtime(&mut self) -> Response {
        if !self.realtime_running {
            return failure(FailureKind::InvalidInput, REALTIME_NOT_RUNNING);
        }
        drop(self.processing.request(Command::Realtime(Realtime::Resync)));
        match self.reset() {
            Response::Done => self.timing(),
            failed => failed,
        }
    }

    /// Removes the installed ring, which no longer matches a chain prepared again or replaced,
    /// stopping a running worker.
    pub(super) fn remove_realtime(&mut self) {
        if self.realtime_installed {
            drop(
                self.processing
                    .request(Command::Realtime(Realtime::Install(None))),
            );
            self.processing.set_bell(None);
            self.realtime_installed = false;
            self.realtime_running = false;
        }
    }

    /// Requests the running worker cannot serve between quanta: they reconfigure or reload the
    /// chain, or reset it outside a resync.
    pub(super) fn refused_while_running(&self, request: &Request<'_>) -> Option<Response> {
        let refused = matches!(
            request,
            Request::PrepareAudio { .. }
                | Request::Reset
                | Request::Load { .. }
                | Request::LoadState { .. }
        );
        (self.realtime_running && refused)
            .then(|| failure(FailureKind::InvalidInput, REALTIME_RUNNING))
    }
}

fn failure(kind: FailureKind, message: impl Into<String>) -> Response {
    Response::Failed {
        slot: None,
        failure: Failure::new(kind, message),
    }
}
