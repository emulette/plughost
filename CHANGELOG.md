# Changelog

All notable changes to plughost are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Releases follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-10-01

### Added

- Notes with IDs and per-note expression. `EventData::NoteOn` and `EventData::NoteOff` carry a
  `Note` (channel, key, velocity in 0..=1, and an optional ID that tells overlapping notes on
  one key apart), and `EventData::Expression` a `NoteExpression` of the volume, pan, tuning,
  vibrato, expression, brightness or pressure of a note. CLAP plugins receive and send them as
  CLAP note and note expression events, and VST3 plugins as note events with their IDs, note
  expression values and poly pressure. Ports and formats that take only MIDI receive the MIDI
  1.0 form of notes and pressure (`EventData::to_midi`); other expressions do not reach them.
- Plugin output in a format's note events arrives as notes and expressions, at the next slot and
  at the application. It used to be counted in diagnostics as having no MIDI 1.0 form.
- `Process::begin_render` runs once as a render starts, before the latency and tail are read;
  its default does nothing.
- `plughost` re-exports `RenderError`, so applications match `Error::Render` without depending on
  `plughost-core`.
- `EventPortInfo::note_expression` and `EventPortInfo::mpe` tell whether notes keep their IDs
  and expressions arrive at a port (the CLAP dialect, or a VST3 controller's
  `INoteExpressionController`) and whether it takes MPE (CLAP's MIDI dialect with MPE, or an
  Audio Unit's `supportsMPE`).

### Changed

- `MidiEvent` is now `Event`, and `MidiData` is `EventData`. `MidiData::Channel` is
  `EventData::Midi` and `MidiEvent::channel` is `Event::midi`; `note_on`, `note_off`,
  `control_change`, `sysex` and `on_port` still build MIDI 1.0 events. `Event` is no longer
  `Eq`, and `bytes` and `byte_count` are gone: match on `EventData`, or use `to_midi` for
  a channel message's bytes. Move an event with `at(offset)` instead of struct update syntax.
- VST3 plugins' notes and poly pressure arrive as `EventData::NoteOn`, `NoteOff` and pressure
  expressions instead of MIDI. `to_midi` gives the MIDI 1.0 messages that used to arrive.
- MIDI input reaches plugins as before, and MIDI poly pressure now also reaches CLAP ports that
  take no MIDI, as pressure. MPE is MIDI 1.0 and is delivered as MIDI; it is not converted into
  notes with IDs or note expressions.
- A chain's latency holds until it is prepared again or reset. A latency a VST3 plugin announces
  with `kLatencyChanged`, or an Audio Unit's changed latency, no longer stops processing or a
  render: the plugin keeps the latency it was activated with, and `take_changes` reports
  `PluginTiming::latency_changed` until `reprepare` or `reset` applies it. Many plugins delay by
  the new latency at once, so a render does not start while a change is pending: it fails with
  the new `RenderError::LatencyNotApplied`. Renders used to fail with
  `RenderError::LatencyChanged` when the read latency changed.
- The helper reads VST3 latency and tail, which VST3 assigns to the UI thread, on its main thread
  instead of after every block on the processing thread; CLAP tails are read there too, and after
  a block in which the plugin reported a change. `PluginTiming` has the new field
  `latency_changed`.
- `TailPolicy::Reported` plans the tail reported when the render starts, and a tail reported
  later no longer fails the render. `RenderError::TailChanged` is gone.
- Types that later versions extend are `#[non_exhaustive]`, so additions do not break
  applications again. Matches on `Layout`, `PluginFormat`, `PluginKind`, `TailPolicy`,
  `RenderStatus`, `StatePurpose`, `PresetMetadata`, `FactoryPresetId`, `ParameterEvent` and
  `DiagnosticSeverity` need a wildcard arm. Build `RenderOptions`, `Transport`, `BlockContext`,
  `Timeouts`, `PluginInfo`, `EventPortInfo`, `AudioBusInfo`, `Capabilities`,
  `CapabilityReport`, `PluginTiming`, `ParameterInfo`, `ParameterFlags`, `ParameterDetails`,
  `FactoryPreset` and `PresetInfo` with their new `new` constructors (or `default`) and then
  set their public fields, instead of struct literals: for example
  `RenderOptions::new(TailPolicy::Reported, 0.0)`, `Transport::new(position, true)` with
  `transport.tempo = Some(120.0)`, `BlockContext::new(frames).with_transport(transport)`, and
  `let mut timeouts = Timeouts::default(); timeouts.process = duration;`. `Rendered`,
  `RenderProgress` and `Delivery` are read only.
- In `plughost-formats`, `HostedPlugin::latency` and `tail` are `HostedPlugin::timing`, which
  reads the timing on the plugin's thread, and `BlockProcessor::latency` and `tail` are
  `BlockProcessor::timing`, which returns what was last read there without calling the plugin.

## [0.0.5] - 2026-10-01

### Added

- Layouts for LCR, quad, 5.0, 7.0, 5.1.2, 5.1.4, 7.1.2, 7.1.4 and 9.1.6, and first- to
  fourth-order ambisonics (ACN order, SN3D normalization). Each has one channel order, listed on
  its `Layout` variant; `Layout::ALL` lists every layout.
- Audio Unit buses take these layouts with the system's channel layout tags. The host maps the
  channels of tags that list speakers in another order, such as the Atmos 7.1.4 tag.
- CLAP plugins with configurable audio ports are asked for the requested layouts when they are
  prepared without a selected port configuration, and ambisonic CLAP ports report their layout. CLAP has no wide speakers, so 9.1.6 is
  refused for CLAP plugins.

### Changed

- `Layout` has new variants, so exhaustive matches on it need new arms. Applications and helpers
  must use the same plughost version, as before.

## [0.0.4] - 2026-09-30

### Fixed

- On macOS, the application that was frontmost when an editor opened is active again once the
  helper's last window closes, so key input returns to it. The helper used to stay the active
  application with no window to type into.

## [0.0.3] - 2026-09-30

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

[0.1.0]: https://github.com/emulette/plughost/releases/tag/v0.1.0
[0.0.5]: https://github.com/emulette/plughost/releases/tag/v0.0.5
[0.0.4]: https://github.com/emulette/plughost/releases/tag/v0.0.4
[0.0.3]: https://github.com/emulette/plughost/releases/tag/v0.0.3
[0.0.2]: https://github.com/emulette/plughost/releases/tag/v0.0.2
[0.0.1]: https://github.com/emulette/plughost/releases/tag/v0.0.1
