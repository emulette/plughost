//! Messages of the capture's errors.

pub(crate) const PERMISSION: &str =
    "the application has no screen recording permission (System Settings, Privacy & Security)";
pub(crate) const NO_DISPLAY: &str = "no display has the region's display identifier";
pub(crate) const NO_HELPER: &str =
    "the helper process has no window to capture yet; open its remote editor first";
pub(crate) const REGION: &str = "the region needs a finite position and a positive size within its display, and a capture cannot move to another display";
pub(crate) const START: &str = "ScreenCaptureKit could not start the capture";
pub(crate) const NO_ANSWER: &str = "ScreenCaptureKit did not answer in time";
pub(crate) const STOPPED: &str = "ScreenCaptureKit stopped the capture";
