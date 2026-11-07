# plughost-capture

Reads the region of a display where a [plughost](https://crates.io/crates/plughost) remote
editor shows, with only its helper's windows, as frames an application encodes and sends to
someone who is not in front of the machine (macOS only, with ScreenCaptureKit). Run every capture
in one process of the application: capture streams of processes with the same code signing
identifier break when one of them ends its stream.

The capturing application needs screen recording permission. The helper does not.

Licensed under either the Apache License, Version 2.0 or the MIT license, at your option.
