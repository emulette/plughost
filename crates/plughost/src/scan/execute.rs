//! Native scan supervision, including connection/handshake cancellation and stall deadlines.
use std::time::Duration;

use plughost_core::ipc::{Request, Response};

use super::control::Work;
use super::{ScanOutcome, ScanStage, Scanner, outcome_of};
use crate::errors::Error;
use crate::helper::{Helper, Mode};

impl Scanner {
    pub(super) fn execute(
        &self,
        request: Request,
        timeout: Duration,
        work: &Work<'_>,
    ) -> ScanOutcome {
        if let Some(outcome) = work.stage(ScanStage::StartingHelper) {
            return outcome;
        }
        let stopped = || work.stopped().is_some();
        let mut helper = match Helper::spawn_controlled(&self.helper, Mode::Scan, &stopped) {
            Ok(Some(helper)) => helper,
            Ok(None) => return work.stopped().unwrap(),
            Err(error) => return outcome_of(error),
        };
        if let Some(outcome) = work.stage(ScanStage::Loading) {
            helper.kill();
            return outcome;
        }
        match helper.send_controlled(request, timeout, &stopped) {
            Ok(Some(())) => {}
            Ok(None) => return work.stopped().unwrap(),
            Err(error) => return outcome_of(error),
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
                Ok(Some(Response::Done)) => return ScanOutcome::Found(classes),
                Ok(Some(Response::Failed { failure, .. })) => return ScanOutcome::Failed(failure),
                Ok(None) => return work.stopped().unwrap(),
                Ok(Some(_)) => return outcome_of(Error::Protocol),
                Err(error) => return outcome_of(error),
            };
            if let Some(outcome) = work.stage(stage) {
                helper.kill();
                return outcome;
            }
        }
    }
}
