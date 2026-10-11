//! Native scan supervision, including connection/handshake cancellation and stall deadlines.
use std::time::Duration;

use plughost_core::ipc::{Request, Response};

use super::control::Work;
use super::{ScanOutcome, ScanStage, Scanner, outcome_of};
use crate::errors::Error;
use crate::helper::{Helper, Mode};

impl Scanner {
    /// Errors before the scan request is sent cannot belong to the plugin. The caller reports
    /// them without replacing its cached native result or incrementing its failure count.
    pub(super) fn execute(
        &self,
        request: Request,
        timeout: Duration,
        work: &Work<'_>,
    ) -> Result<ScanOutcome, Error> {
        if let Some(outcome) = work.stage(ScanStage::StartingHelper) {
            return Ok(outcome);
        }
        let stopped = || work.stopped().is_some();
        let mut helper = match Helper::spawn_controlled(&self.helper, Mode::Scan, &stopped)? {
            Some(helper) => helper,
            None => return Ok(work.stopped().unwrap()),
        };
        if let Some(outcome) = work.stage(ScanStage::Loading) {
            helper.kill();
            return Ok(outcome);
        }
        match helper.send_controlled(request, timeout, &stopped)? {
            Some(()) => {}
            None => return Ok(work.stopped().unwrap()),
        }
        let mut classes = Vec::new();
        loop {
            let stage = match helper.receive_controlled(timeout, &stopped) {
                Ok(Some(Response::ModuleLoaded)) => ScanStage::ModuleLoaded,
                Ok(Some(Response::Class(class))) => {
                    if class.format == plughost_core::PluginFormat::AudioUnit
                        && !self.policy.allows_audio_unit(&class.class_id)
                    {
                        continue;
                    }
                    let stage = ScanStage::ClassFound(class.clone());
                    classes.push(class);
                    stage
                }
                Ok(Some(Response::Done)) => return Ok(ScanOutcome::Found(classes)),
                Ok(Some(Response::Failed { failure, .. })) => {
                    return Ok(ScanOutcome::Failed(failure));
                }
                Ok(None) => return Ok(work.stopped().unwrap()),
                Ok(Some(_)) => return Ok(outcome_of(Error::Protocol)),
                Err(error) => return Ok(outcome_of(error)),
            };
            if let Some(outcome) = work.stage(stage) {
                helper.kill();
                return Ok(outcome);
            }
        }
    }
}
