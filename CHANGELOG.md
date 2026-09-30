# Changelog

All notable changes to plughost are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Releases follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.0.2] - 2026-09-30

### Changed

- Releases are published from GitHub Actions through crates.io trusted publishing. The crates'
  code is unchanged from 0.0.1.
- The repository's test asset scripts keep the VST3 SDK archive, sources, and builds in
  `.vst3sdk/` instead of `target/`. Delete `target/VST_SDK`, `target/vst-sdk.zip`, and
  `target/vst3sdk-build*` from an existing checkout.

## [0.0.1] - 2026-09-30

First public release: offline hosting of VST3 and CLAP plugins on macOS (Apple Silicon) and
Windows x64, and of Audio Units on macOS, with each chain isolated in a helper process.

- Scanning: metadata discovery without loading plugin code, native inspection in helpers with a
  crash- and hang-tolerant cache, allow/block policy, cancellation, retries, and CLAP preset
  discovery.
- Chains: serial plugin chains with explicit bus routing, sidechains, multiple outputs,
  mono/stereo adaptation, latency alignment, 32-bit and 64-bit processing, block timing,
  transport, and sample-accurate automation.
- Events: MIDI 1.0 and system exclusive input and output on multiple event ports, routed between
  slots, with fixed per-block budgets.
- Rendering: offline rendering with latency removal and bounded tails, schedules with ramps and
  transport changes, streaming with cancellation, and multi-segment sessions.
- Parameters, state, and presets: native parameter metadata and text conversion, plugin edit
  notifications, project and preset state, `.vstpreset` and `.aupreset` files, and factory
  programs.
- Editors: native editor windows in the helper, including floating CLAP editors and editor
  requests plugins make themselves.
- Failures: typed errors with the failing slot, helper exit monitoring, diagnostics, and chain
  recovery in a new helper from saved states.

[0.0.2]: https://github.com/emulette/plughost/releases/tag/v0.0.2
[0.0.1]: https://github.com/emulette/plughost/releases/tag/v0.0.1
