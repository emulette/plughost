# plughost-core

Shared types and offline render rules for [plughost](https://crates.io/crates/plughost): plugin
and parameter metadata, audio and processing configuration, events, state, errors, and the
renderer that drives any block processor.

It also contains the message protocol and shared-memory transport between `plughost` and
`plughost-helper`. Those modules are internal to the two crates and exempt from semver.

Applications normally depend on [`plughost`](https://crates.io/crates/plughost), which re-exports
the types they need.

Licensed under either the Apache License, Version 2.0 or the MIT license, at your option.
