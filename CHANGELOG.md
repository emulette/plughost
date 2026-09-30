# Changelog

All notable changes to plughost are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Releases follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- A version 2 Audio Unit that changes only the value strings of its parameters is reported
  with `ParameterEvent::MetadataChanged`. The system's v2 bridge keeps its parameter tree for
  such a change, so nothing reported it before.
- An Audio Unit editor window follows the size the unit's view gives itself, or the size the
  view controller of a version 3 unit in the application's process prefers, after the editor
  opens. macOS does not report such changes of an AUv3 running in its extension process.
- `Chain::recover` restores states that together exceed one message between the application
  and the helper. It used to fail with `Error::Crashed` after waiting for the helper to exit.
- A VST3 plugin's state saved before the plugin was ever prepared includes the edits made
  before it. `save_state` used to fail with a `NotPrepared` error.
- A CLAP plugin's parameter values read right after `set_parameter` include the edit.
- Automation of several VST3 parameters no longer costs each block time quadratic in its
  points (a 4,096-frame block with 4,096 points on two parameters: 2.7 ms before, 0.4 ms now).
- An Audio Unit instrument, generator or MIDI processor prepares with its audio input off even
  when the channel pairs it lists all have the input on, as JUCE's units list them.
- Discrete Audio Unit parameters reach the unit as whole values. 13 of 22 steps used to arrive
  as 12.999999, which units that truncate read as 12.
- An Audio Unit that asks for only some of the musical context gets it when the transport has
  those values. It used to get nothing unless tempo, beat position and time signature were all
  known.
- The Audio Unit timeline moves on after a block whose render failed, so the next block does not
  repeat its time.
- Every VST3 process call has a process context with the sample rate and continuous time, also
  without a transport. Many plugins read it without checking.
- A VST3 plugin that announces changed buses or asks for a reload while it activates no longer
  asks for the same preparation again forever.
- The first automation point of a VST3 parameter after preparation, a restored state or values
  the plugin changed is a step at its offset. It used to ramp from wherever the plugin was.
- A VST3 plugin's failed block leaves no output events for the next block, and a controller
  that fails to save its state fails `save_state`.
- Output events a VST3 or CLAP plugin places past the end of the block land on its last frame,
  as Audio Units' already did. They used to fail the next plugin's block or leave the chain out
  of the block.
- A CLAP note end, which only reports a voice that ended, no longer counts as an undeliverable
  event in diagnostics.
- Channel messages reach CLAP ports that take only MIDI with MPE, which the event port list
  already reported as taking MIDI.
- A CLAP timer that another timer removed in the same tick is not called.

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
