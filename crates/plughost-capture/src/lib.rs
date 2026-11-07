//! Reads the region of a display where a plughost remote editor shows (see `plughost::remote`),
//! with only its helper's windows: menus and dialogs of the helper come with it, and other
//! sessions' windows next to it do not. Frames go to the application, which encodes and sends
//! them. macOS only, with ScreenCaptureKit; on other platforms the crate is empty.
//!
//! Run every capture in one process of the application. When processes with the same code
//! signing identifier capture at once and one of them ends its stream, the others' streams
//! break and no new capture starts until they end. Keep it in mind when a new version of the
//! application starts before the old one ends: stop the old captures first. The capturing
//! application needs screen recording permission; the helper does not.

#![warn(missing_docs)]

#[cfg(target_os = "macos")]
mod capture;
#[cfg(target_os = "macos")]
mod errors;

#[cfg(target_os = "macos")]
pub use capture::{Capture, CaptureError, CaptureOptions, Frame, PixelFormat, StopReason};
