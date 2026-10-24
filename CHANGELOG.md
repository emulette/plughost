# Changelog

All notable changes to plughost are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Releases follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- A realtime callback that began before `stop_realtime` or `recover_realtime` no longer submits
  to the old generation after the next start, which sent the new generation into a resync:
  stopping and recovering wait for a callback in progress and stop its generation's
  submissions, as `resync_realtime` does. `stop_realtime`, `recover_realtime`, and dropping or
  preparing again a chain with an endpoint now wait for that callback.
- A realtime callback that panics no longer leaves the endpoint's call count unbalanced, after
  which a resync either went on while a callback submitted or never returned.
- A `Fallback::Dry` endpoint called with more frames than the quantum plays the input its delay
  line still holds, delayed, and silence before it; it read overwritten history and, in a debug
  build, panicked in the callback.
- The helper no longer ends with a control timeout when timing changes keep arriving while a
  started realtime endpoint submits nothing, as when the device stopped or a resync is pending:
  its worker takes them as they come. A CLAP plugin reporting a new tail on every tick filled
  the queue in about 0.64 s.
- Audio Unit editors, parameter lists, parameter edits and lookups no longer hold up the
  processing thread: they call the unit without the lock it takes for each block. Opening an
  editor while the realtime endpoint ran could make the worker miss.
- A `restore_state`, preset, factory preset or `reprepare_slot` that changes the chain's latency
  while the realtime endpoint runs succeeds when the resync it starts does not complete; the
  endpoint plays its fallback with `resync` set meanwhile. It returned the resync's error though
  the slot was already replaced.
- The note offs a new realtime generation sends for the notes earlier outputs left on go out with
  its first callback that returns `Ok`. A first callback with invalid input dropped them with its
  error.
- `RealtimeStatus::late` no longer counts a quantum the helper could not read, and goes on across
  `recover_realtime` instead of starting again from 0.
- A failure the helper reports in a slot the chain does not have counts as a protocol failure
  without a slot.

## [0.2.0] - 2026-10-24

0.2.0 settles the public API: types later versions extend are non-exhaustive, states and plugin
identities have stable forms of their own, and events carry system exclusive data without an
allocation per message. Upgrading from 0.1:

- Rebuild and ship the helper with the application; the helper protocol is 34. Scan caches
  written by 0.1 are read as empty and their bundles scanned again.
- Store plugins as `PluginId` and states as `PluginState::to_bytes()`; the public types have no
  serde form, and 0.2 cannot read states 0.1 serialized with it, so re-create those. Read a
  state through `id()`, `component()` and the other accessors. Build references with
  `PluginRef::vst3`, `PluginRef::clap` or `PluginRef::audio_unit`, read `PluginInfo::id`, and
  read `Diagnostic::plugin` as an `Option<PluginId>`.
- Spawn chains with `Chain::spawn(helper, &[plugin.into()], &SpawnOptions::default())`, and
  recover them from `ChainSnapshot::slots`. `recover(&[])` no longer means the initial states:
  it fails with `InputError::EmptyChain`, so pass a `ChainSlot` per `chain.references()`.
- Pass events as `&Events` and produce them into a reused `Events::for_block()`;
  `Event::sysex` takes a slice, and an `Event` borrows its system exclusive bytes from the
  `Events` it was read from, so keep the `Events` to keep the events.
- Build routing, processing, render and preset types with their constructors instead of struct
  literals, and add a wildcard arm to matches on the enums that are now non-exhaustive. CLAP
  preset flags are a `PresetFlags`.
- Read parameter values as `Option<f64>`; `None` is a value the plugin does not give.
- Expect `RenderError::NonFiniteOutput` instead of `non_finite_samples`, and
  `RenderStatus::TailLimit` for a tail that reached its maximum.
- Match rejected render input as `Error::Input { error, .. }`, which plughost now reports
  instead of `Error::Render(RenderError::Input(..))`, and buffer mismatches as
  `InputError::AudioBuffers`: `Error::Buffers`, `RenderError::Events`, `InputChannels` and
  `InputLength` are removed.
- Match `plughost_formats::Error` by `kind()` and `input_error()`. Errors display only their own
  context; walk `source()` (or use `failure()`) to show their causes.
- Wrap the plughost-formats loaders in `unsafe` with the reason the plugin is trusted, and load a
  CLAP module as a `clap::Entry`. `HostedPlugin`, `BlockProcessor` and `Sample` are sealed, so
  implement `render::Process` for a processor of your own. `preset::supports` is `supported`,
  and `vst3::Preset::controller` and `DecodedPreset::controller` are an `Option`.
- Stop using the buffers and checks plughost-core exported for its own crates
  (`DiagnosticBuffer`, `events_fit`, `validate_normalized` and the like) and `sysex_bytes`.
- Write `let _ =` where a `take_*` drain discards its batch.

### Added

- `PluginId` names a plugin class independently of where it is installed: its `PluginFormat`
  and a `ClassId` kept in canonical form, VST3 and Audio Unit IDs in upper case and CLAP IDs as
  given, so IDs written in another case are one class. It supports serde, with format names
  `"vst3"`, `"audio-unit"` and `"clap"`, for storing with the work that uses the plugin.
  `Catalog::resolve(&id)` finds the installed class as a `PluginRef`, from the highest priority
  directory, and `BundleScan::plugins()` gives the references of one scanned bundle.
- Constructors for the routing, processing, render and preset types an application builds, which
  are now `#[non_exhaustive]`: `RoutedChainConfig::new`, `SlotAudioConfig::new`,
  `AudioConfig::new`, `AudioBusConfig::new`, `AudioInputRoute::new`, `SlotEventConfig::new`,
  `EventInputRoute::new`, `EventConfig::new`, `ProcessConfig::new`, `HostIdentity::new`,
  `ParameterChange::new`, `AutomationEvent::new`, `RenderInput::new`, `AutomationRamp::new`,
  `TransportChange::new`, `TimeSignature::new`, `LoopRegion::new`, `BarPosition::new`,
  `PresetDirectory::new`, `PresetFile::new`, `PresetDiscoveryTarget::new` and
  `DiscoveredPreset::new`, with `with_*` methods for their optional parts (mode, named
  configuration, events, event inputs, channel adaptation, render schedule) and
  `RenderSchedule::with_transport`, `with_automation` and `with_ramps`.
- `ChainSlot` (a `PluginRef` with an optional state and native bypass) and `SpawnOptions` (host
  identity and timeouts) describe what `Chain::spawn` loads, and `Chain::references()` gives the
  references a chain loaded.
- `PresetFlags` names the CLAP preset discovery flags.
- `PresetDiscoveryTarget::validate` checks a target's provider ID and location, failing with the
  new `InputError::PresetDiscoveryTarget`. `plughost::discover_presets` calls it before starting
  a helper, and the helper before loading the module, so invalid input never runs plugin code.
- `plughost_formats::clap::Entry` is a loaded CLAP module, as `vst3::Module` is for VST3:
  `Entry::load(bundle)` loads it once, and `classes()`, `clap::Plugin::new`,
  `clap::preset_providers` and `clap::discover_presets` use it instead of loading the module
  again on each call.
- `PluginState::to_bytes` and `PluginState::from_bytes` store a state in plughost's own
  versioned format, which later versions keep reading and which does not depend on the helper
  protocol; 0.2 cannot read 0.1 states, which had only a serde form. It is
  a container of tagged sections for the identity and the native payloads, with a VST3
  controller state kept apart from its component and absent, not empty, when the controller
  has none. Reading needs no plugin, writes a state back unchanged, and checks every
  length against `MAX_STATE_BYTES` before allocating; bytes it cannot read fail with the new
  `InputError::StateBytes`.
- `Events` holds a block's or a render's events as fixed records, with the system exclusive
  bytes of all of them in one area, so a system exclusive message no longer allocates on its own.
  `Events::for_block()` reserves one block's `MAX_BLOCK_EVENTS` events and
  `MAX_BLOCK_SYSEX_BYTES` bytes, within which `push` never allocates and beyond which it fails
  with `InputError::EventCapacity`, and `Events::try_for_block()` reports a failed reservation
  instead of aborting; `Events::new()` grows without that limit. It is built with `push` or by
  collecting `Event`s, read with `iter` (an `EventsIter`), `get` and `len`, and compares equal
  to a slice or array of `Event`. A clone of a block's `Events` reserves a block's room again.
- `RenderStatus::TailLimit` and `Rendered::status` tell a `TailPolicy::UntilSilence` tail that
  reached `max_tail_seconds` before the output stayed silent apart from one that ended in
  silence. `RenderProgress::status` from `render_stream` and `RenderSession::finish` reports it
  too, where such a render reported `Complete`; `Rendered` had no status.

### Changed

- Events are passed as `&Events` and produced into `&mut Events` instead of `&[Event]` and `&mut
  Vec<Event>`: in `Chain::process_audio_*`, `RealtimeEndpoint::process_audio_*`, `render`,
  `render_with_schedule`, `RenderInput`, `Delivery`, the `Process` trait, `BlockProcessor` and the
  format plugins' processing calls, and `Rendered::events` is an `Events`.
  `plughost_core::sysex_bytes` is removed; read `Events::sysex_bytes()`. Write `&Events::new()` for
  no events and `&Events::from_iter([Event::note_on(0, 0, 60, 100)])` for a literal list, and reuse
  an `Events::for_block()` for `produced`. `Event` and `EventData` are `Copy` with a lifetime:
  `EventData::SysEx` borrows its bytes, from the `Events` it was read from or from the caller, so
  `Event::sysex(offset, &bytes)` takes a slice and `.clone()` calls become copies.
- A realtime endpoint delivers the output's system exclusive events into `produced`, which it
  dropped and counted before. `RealtimeStatus::dropped_events` now counts only events beyond the
  room `produced` has reserved, so pass it an `Events::for_block()`. System exclusive output an
  Audio Unit sends over several calls is joined in room reserved when the unit is prepared,
  instead of in a buffer allocated per message on the render thread.
- The public types no longer implement serde's `Serialize` and `Deserialize`, apart from
  `PluginId`: persist states with `PluginState::to_bytes` and plugins by `PluginId`, and keep
  routing, configurations and other settings in the application's own model. Their serde form
  was the helper protocol's, which changes with every protocol version; the protocol now uses
  definitions of its own.
- `Chain::spawn(helper, slots, &options)` takes a `ChainSlot` per plugin, a `PluginRef` with an
  optional project state and native bypass, and a `SpawnOptions` with the host identity and the
  timeouts: write `Chain::spawn(helper, &[plugin.into()], &SpawnOptions::default())`, or
  `SpawnOptions::new(host, timeouts)`. Each plugin restores its state on the instance it loaded,
  which loads it once instead of twice as a `restore_state` after spawning does, and then takes
  its bypass. `recover` and `recover_realtime` take the same slots, which `ChainSnapshot::slots`
  holds in place of `states` and `bypass`, and `recover` now applies the recorded bypass too;
  recovering from states alone pairs each with its slot's plugin in a `ChainSlot`.
  `recover(&[])`, which recovered the initial states, fails with `InputError::EmptyChain`; pass
  a slot per `Chain::references()`, the new accessor for the references a chain loaded.
  `InputError::StateCount` is removed, since a slot carries its own state.
- `PluginState`'s fields are private and it has no serde form: store `to_bytes()` instead of
  serializing it. `id()` replaces `format` and `class_id`, and `name()`, `vendor()` and
  `version()` the fields of those names. A restore checks only the `PluginId`, not the plugin
  version. `vst3::Preset::controller` and `DecodedPreset::controller` are an `Option`, `None`
  for a preset without a `Cont` chunk; an empty `Cont` chunk is kept when a preset is written
  again, where it was dropped. An empty VST3 controller state is still not passed to the
  controller when restoring.
- An Audio Unit state over `MAX_STATE_BYTES` fails to save, as the other formats' states do.
- `PluginRef` holds a `PluginId` and the bundle, both private: build it with
  `PluginRef::vst3(bundle, class_id)`, `PluginRef::clap(bundle, class_id)` or
  `PluginRef::audio_unit(class_id)` and read it with `id()` and `bundle()`, so a VST3 or CLAP
  reference without a bundle can no longer be made; the error for one is removed.
  `PluginInfo::format` and `PluginInfo::class_id` are `PluginInfo::id`: read
  `info.id.format()` and `info.id.class_id().as_str()`, and build an ID read from storage with
  `PluginId::new(format, class_id)`. `PluginInfo::new` takes a `PluginId`, and
  `Diagnostic::plugin` is an `Option<PluginId>`.
- Class IDs compare in their canonical form everywhere. A state or preset whose VST3 or Audio
  Unit class ID differs from the plugin's only in case restores (an Audio Unit state failed with
  `FailureKind::StateMismatch`).
- The scan cache schema is 3: caches written by 0.1 are read as empty and their bundles
  inspected again.
- The internal helper protocol is now 34. Applications and helpers with different protocol
  versions fail the handshake; rebuild and distribute the helper with the application.
- `plughost_formats::Error` is one opaque struct for every format instead of an enum with a variant
  per format, and `Vst3Error`, `ClapError` and `AuError` are no longer public; the format modules'
  functions, such as `vst3::Module::load` and `clap::Entry::load`, return `plughost_formats::Error`.
  Match `kind()` for the `FailureKind` and `input_error()` for rejected caller input instead of
  variants; `format()` names the format whose host failed or that this build does not support (none
  for rejected caller input), `native_code()` gives the VST3 `tresult` or the Audio Unit `OSStatus`
  (or `NSError` code), and `is_output_event_overflow()` and `failure()` remain. It is no longer
  `Clone` or `PartialEq`; compare `kind()`.
- Errors keep their causes. A plughost-formats error and a `RenderError` display only their own
  context, and `source()` gives the input or render error, the module loader's error (VST3 on
  Windows, CLAP), the CLAP instance or GUI error, the I/O error of a CLAP preset location, or the
  error an Audio Unit returned behind it,
  which were dropped or turned into text before. `plughost_formats::Error::failure()` puts the
  whole chain in its message.
- An Audio Unit class ID that no registered unit has fails with `FailureKind::NotFound`, as a
  VST3 or CLAP class that a module does not have does, instead of `FailureKind::Load`.
- Each input contract has one error. Render events that are not valid, not in offset order or not
  before the end of the input are `RenderError::Input(InputError::Events)`; `RenderError::Events` is
  removed. plughost reports input that a render rejects as `Error::Input` with no slot, never as
  `Error::Render`. Audio buffers of the wrong channel count or length, or a block longer than the
  prepared maximum, are `InputError::AudioBuffers` in every crate and in rendering:
  `Error::Buffers`, `RenderError::InputChannels` and `RenderError::InputLength` are removed, so
  match `Error::Input { error: InputError::AudioBuffers, .. }` or
  `RenderError::Input(InputError::AudioBuffers)`. Buffers of another precision than the prepared one
  are `InputError::AudioSampleFormat` in the in-process VST3 and CLAP `process` calls too, where
  they were a VST3 sample format mismatch and a CLAP unsupported sample format. Parameter changes
  that the in-process `process` calls receive out of offset order, outside the block or outside
  `0..=1` fail with `InputError::Automation`, as automation does in a chain, rather than as a buffer
  error (or, for a VST3 value, `InputError::ParameterValue`).
- `plughost_formats::preset::supports(format)` is `supported(format)`, which returns the
  unsupported error instead of `false`.
- The types an application builds are `#[non_exhaustive]`, so later versions can add fields:
  build `RoutedChainConfig`, `SlotAudioConfig`, `AudioBusConfig`, `AudioInputRoute`,
  `SlotEventConfig`, `EventInputRoute`, `AudioConfig`, `EventConfig`, `HostIdentity`,
  `ParameterChange`, `AutomationEvent`, `ProcessConfig`, `RenderInput`, `RenderSchedule`,
  `AutomationRamp`, `TransportChange`, `TimeSignature`, `LoopRegion`, `BarPosition`,
  `PresetDirectory`, `PresetFile`, `PresetDiscoveryTarget` and `DiscoveredPreset` with their
  constructors, or start from `main_bus_config` or `default()`, and set public fields instead of
  writing struct literals. The output-only types `ChangeBatch`, `SlotChange`, `Catalog`,
  `BundleScan`, `Failure`, `Diagnostic`, `DiagnosticBatch`, `ParameterGroup`, `ParameterChoice`,
  `ParameterChoicePage`, `ParameterEventBatch`, `AudioConfiguration`, `BypassState`,
  `DecodedPreset`, `vst3::Preset` and the preset discovery results are `#[non_exhaustive]` too,
  as are the enums `Message`, `AudioBusRole`, `AudioDirection`, `AudioSource`, `EventSource`,
  `ChannelAdaptation`, `Support`, `SampleFormat`, `ProcessMode`, `Tail`, `SessionState`,
  `PresetLocation`, `EditorRequest` and `EditorView`, which need a wildcard arm in a `match`.
- plughost-core no longer exports from its root the buffers and checks the plughost crates
  share among themselves: `DiagnosticBuffer`, `ChangeBuffer`, `ParameterEventBuffer`,
  `events_fit`, `validate_event_budget`, `changes_fit`, `validate_normalized`,
  `validate_parameter_text` and `validate_choice_page` are internal, and `validate_event_count`
  is no longer public. The methods `RoutedChainConfig::slot_config`, `SlotEventConfig::native`
  and `ParameterInfo::validate_edit`, which only those crates call, are internal too.
- The plughost-formats functions that load a plugin's code into the calling process are
  `unsafe fn`, since that code can do anything the process can and nothing checks it:
  `plughost_formats::load`, `vst3::Module::load` and `load_with_host`, `au::Plugin::new` and
  `clap::Entry::load`. The functions that use a loaded plugin stay safe, as does all of the
  `plughost` crate, which loads plugins in its helper processes. `clap::Plugin::new`,
  `clap::preset_providers` and `clap::discover_presets` take a `clap::Entry` instead of a bundle
  path, `clap::classes(bundle)` is `Entry::classes`, and `clap::classes_once_loaded` is removed:
  load the `Entry`, then read its classes.
- Public constructors and builders that return their value (`new`, `with_*`, the `Event`,
  `PluginRef` and `Scanner` builders, `Event::at` and `on_port`), `RenderSession`,
  `DiagnosticBatch`, `ParameterEventBatch`, `ChangeBatch` and `HostedPlugin::take_editor_request`
  are `#[must_use]`: discarding one, as a `take_*` drain does, is a warning, so write `let _ =`
  where a drain is intended.
- `HostedPlugin`, `BlockProcessor` and `Sample` are sealed: the format plugins and processors
  and `f32` and `f64` implement them, and other crates cannot. `render::Process` and
  `AudioThread` remain for applications to implement. With `Sample` sealed, processing no longer
  compares type IDs to refuse a sample type claiming another precision.
  `HostedPlugin::set_editor_scale` is a default method on every platform, returning `None`
  outside Windows, where it was only declared on Windows.
- CLAP preset discovery flags are a `PresetFlags` instead of a `u32`
  (`PresetLocationInfo::flags`, `PresetSoundpack::flags`, `DiscoveredPreset::flags`); read the
  bits with `bits()`.
- `Chain::parameters` and `HostedPlugin::parameters` give each value as an `Option<f64>`, and
  `vst3::Plugin::parameter_value` returns one: a value the plugin does not give, that is not
  finite, or whose range is not, is `None` where it read as 0, and so is a VST3 ID the plugin
  does not have. A CLAP `get_value` that fails and an `AudioUnitGetParameter` that a v2 unit
  refuses are no longer read as 0 or, for the Audio Unit, as the `AUParameter`'s value. A CLAP
  or VST3 bypass whose value is unavailable or not finite reads as unknown rather than off, so
  recovery and slot replacement no longer switch such a bypass off. Values a little outside
  their range are still clamped into it.
- A NaN or infinite output sample fails a render with `RenderError::NonFiniteOutput { frame }`,
  the first output frame holding one, whatever the tail policy. Such samples were delivered
  and counted in `Rendered::non_finite_samples` and `RenderProgress::non_finite_samples`, which
  are removed, and as they are never silent they kept an `UntilSilence` tail going to its
  maximum.

### Fixed

- An Audio Unit v2 parameter edit that `AudioUnitSetParameter` refuses fails with
  `FailureKind::State` and the `OSStatus` as `native_code()`, and is neither kept as the host's
  edit nor announced to the unit's parameter listeners. It succeeded before, whatever the status.
- An Audio Unit that renders into other buffers than it was given fails the block with
  `FailureKind::Processing`; it was reported as invalid caller input, after the unit had
  processed the block.

## [0.1.6] - 2026-10-23

### Added

- `Chain::canceller` returns a `ChainCanceller` whose `cancel` terminates the chain's helper from
  any thread, which ends a call the chain is blocked in, such as a plugin that does not return
  from restoring state, loading a preset, opening its editor, preparing again or processing a
  block. That call, or the next one if none is blocked, returns the new `Error::Cancelled`
  (`FailureKind::Cancelled`) with the slot the helper was calling, and later calls return
  `Error::Crashed` as after a timeout.
- `HelperMonitor::end_reason` tells why a helper ended once its exit is observed: it crashed or
  exited on its own, plughost terminated it after a timeout, a protocol error or a cancellation,
  or its chain shut it down. The errors after the first one report `Error::Crashed` whatever the
  reason was.
- `Rendered::non_finite_samples` and `RenderProgress::non_finite_samples` count the NaN and
  infinite samples a render delivered, over all channels and excluding the removed latency;
  a render session reports them per `push` and `finish`. Rendering still neither rejects nor
  replaces them, and they still count as sound for `TailPolicy::UntilSilence`, so a plugin that
  keeps producing NaN renders up to the maximum tail.

### Changed

- The load timeout bounds each plugin of a chain rather than the whole chain: loading a chain,
  and preparing an already configured chain again, renew it after each plugin they load or
  prepare, also in `recover` and `recover_realtime`. A chain of instruments that each load
  within the timeout no longer times out together.
- A panic in the helper aborts it, so the application reports a crash of the slot the helper
  was calling. A panic on its processing thread made the request it served time out without
  naming a slot before.
- `RenderSession` reuses its render buffers from one `push` to the next instead of allocating
  and zero-filling block buffers and event storage for every segment, and trims the latency
  without copying each delivery's channel list. Output and progress are unchanged.
- The internal helper protocol is now 31. Applications and helpers with different protocol
  versions fail the handshake; rebuild and distribute the helper with the application.

### Fixed

- A VST3 editor whose view reports a size with coordinates far apart no longer panics the
  helper when it computes the window size.
- A scan no longer forgets the bundles of a scanned directory that is missing, such as one on a
  disconnected volume, so they are cache hits again when it returns instead of loading again,
  and a bundle that crashed stays excluded. Bundles removed from a directory that exists are
  still forgotten, except that an entry with failures keeps its count and exclusion until a
  retry, and a link to a bundle or folder on a disconnected volume keeps its entries. A retry no
  longer drops the entries of other removed bundles.
- In a prepared chain, `restore_state`, presets, factory presets and `reprepare_slot` fail with
  `FailureKind::Configuration` and keep the active slot when the fresh instance has other active
  audio buses or event ports than the slot was prepared with; `reprepare` routes them. The slot
  was replaced before, and processing went on routing by the replaced instance's native ports.
- A `reset` that fails at one slot after earlier slots were reset leaves the chain to be prepared
  again: blocks fail with `FailureKind::NotPrepared`. They went on with alignment delays that no
  longer matched the reset slots.
- A parameter edit or reset that took effect succeeds when the chain's timing cannot be read or
  planned afterwards; the chain keeps its last latency and tail, and the diagnostics record the
  failure. It failed with that error before, as if the edit had not been made.
- `Transport::advanced` no longer fails with `InputError::Transport` for a beat a few ulps
  below a bar line, where rounding placed the computed bar start after the beat; the beat now
  stays in the earlier bar. A scheduled render or session segment whose blocks reached such a
  beat failed partway through after the schedule had been accepted, and a session segment that
  stopped at one, by cancellation or at the end of a silent tail, returned the error while the
  session stayed `Active`.
- A CLAP plugin's output event whose header gives another size than its type's is counted in
  diagnostics as an event with no portable form. It was read as its type, past the end of an
  event the plugin declared shorter, and became a note or a parameter notification.
- VST3 and CLAP output system exclusive data is copied only once the size the plugin declares
  fits what is left of the block's budget; a larger one fails the block as more output than a
  block allows, before anything is read. It was copied at the declared size on the processing
  thread first, so a size past the plugin's buffer crashed the helper.
- The counts a CLAP plugin reports are bounded before they are followed. A plugin's first 65,536
  parameters are read, with a `PluginMisbehaving` diagnostic when it reports more;
  `audio_configurations` fails with the audio metadata error for more than 1,024 configurations
  or more than `MAX_AUDIO_BUSES` ports in a direction of one, as `audio_buses` does for the active
  ports; and capabilities are `Support::Unknown` where more note ports (beyond 65,535), audio ports
  or parameters would have to be looked at. A plugin reporting billions made the helper allocate
  without bound or run until the control timeout.
- Parameter values and ranges that `parameters` and `parameter_details` read follow one rule in
  every format: a value outside its range is clamped into it, and one that is not finite, or
  whose range is not, reads as 0. A CLAP parameter whose range is not finite has no default, and `set_parameter`
  and automation of a CLAP or Audio Unit parameter with such a range fail with the format's
  parameter result error (`FailureKind::Host`). `parameters` reported such values as NaN or
  outside 0 to 1, and edits sent NaN to the plugin.
- Text an Audio Unit gives (parameter, group, bus, MIDI output and preset names, parameter units,
  value text and error descriptions) is converted with U+FFFD in place of what is not valid
  UTF-16, such as an unpaired surrogate, as component names already were. Such text aborted the
  helper in debug builds and was undefined behaviour in release builds.
- A CLAP plugin that registers or unregisters a timer off the main thread is refused, with a
  `PluginMisbehaving` diagnostic. The host's timer list, which only the main thread may use, was
  changed from that thread.
- Host edits of a CLAP plugin's parameters stay queued until a block the plugin processes has
  delivered them. A block the plugin failed took them, so the plugin kept the old values.
- A CLAP plugin's output notes with IDs end with the note choke or note end that names them, as
  with a note off. They stayed among the 4,096 notes the host follows, and once those were taken,
  a later note's expressions naming it by ID alone counted as events with no portable form.

## [0.1.5] - 2026-10-23

### Added

- `Error::ProtocolVersion` reports a helper built with another plughost version than the
  application, with both internal protocol versions, instead of `Error::Handshake`, which now
  means only that the helper did not present the expected token. Scans record it as a protocol
  failure with the same message.
- `Scanner::host` sets the identity native inspection presents to plugins, which VST3 factories
  receive as their host context. Scans presented the default plughost identity before, while a
  chain presented the application's.

### Changed

- The internal helper protocol is now 30. Applications and helpers with different protocol
  versions fail the handshake; rebuild and distribute the helper with the application.

## [0.1.4] - 2026-10-20

### Added

- `Chain::resync_realtime` resynchronizes a running realtime endpoint without stopping it: the
  callbacks play the fallback while every plugin is reset as by `reset`, which ends the notes the
  plugins hold and applies an announced latency, and the helper's worker goes on with a new
  generation. It returns the endpoint's whole latency, as `start_realtime` does, and changes
  nothing on a stopped endpoint. Stopping the endpoint, resetting the chain and starting it again
  still works.
- `Chain::snapshot` returns a `ChainSnapshot` with each slot's project state and native bypass,
  also while the endpoint runs. `Chain::recover_realtime(&snapshot)` recovers the endpoint in a
  new helper: it prepares a chain from the snapshot, applies the bypass, and moves the same
  endpoint to it while the audio callback keeps the `RealtimeEndpoint` it has. The callback plays
  its fallback during the recovery, and after `latency` priming callbacks the new chain's output.
  Switch to the returned chain and drop the old one. A failed recovery leaves the endpoint with
  the old chain and retries nothing. `recover_realtime` loads with the load timeout and prepares
  with the control timeout.
- `RealtimeStatus::stalled` reports how long the helper's worker has gone without completing a
  quantum the callback submitted, which grows while a plugin hangs in its processing; a crashed
  helper shows through `helper_monitor`. Together they tell when to call `recover_realtime`.
- `Chain::reprepare_slot` prepares one slot again on a fresh instance holding the active one's
  project state, which drops a pending restart request, as `reprepare` does for the chain. It
  uses the load timeout and works while the realtime endpoint runs.

### Changed

- `restore_state`, `import_preset`, `load_discovered_preset` and `select_factory_preset` work
  while the realtime endpoint runs: the helper stages the fresh instance as it does offline and
  the worker swaps it in between quanta, and the stream goes on. A replacement that changes the
  chain's latency resynchronizes the endpoint right after, as `resync_realtime` does. Only
  `reset`, `prepare_audio`, `reprepare`, `realtime_endpoint`, processing through the chain and
  rendering still fail with `Error::RealtimeRunning`; use `resync_realtime` instead of `reset`,
  and `reprepare_slot` instead of `reprepare` for a restart request.
- The first callback of every new generation (a start, a resync or a recovery) ends the notes
  that earlier outputs delivered through `produced` and did not end, with note offs at offset 0;
  the endpoint keeps track of up to 256 notes, and events that do not fit count as
  `dropped_events`. Notes a replaced instance sent to later slots are still ended only by a
  reset or resync.
- `restore_state`, presets and factory presets on a slot with a pending VST3 or CLAP restart
  request now succeed, also offline, and drop the request with the replaced instance; they
  failed with `RestartRequired` for that slot before.
- The internal helper protocol is now 29. Applications and helpers with different protocol
  versions fail the handshake; rebuild and distribute the helper with the application.

### Fixed

- VST3 `reset`, the VST3 parameter flush before `prepare_audio` and Audio Unit `reset` also run
  with subnormal numbers flushed to zero, as 0.1.3 documented for every format.
- Discovery reads a VST3 bundle's `moduleinfo.json` in time and memory proportional to its size.
  The JSON5 parser copied a string or identifier again for each escape sequence in it, so a 256 KB
  file of escapes allocated 17 GB and took a third of a second; plughost now requires json5 1.3.3,
  which fixes it.

## [0.1.3] - 2026-10-18

### Changed

- Plugins process, reset and flush parameters with subnormal numbers flushed to zero, as inputs
  and as results (MXCSR FTZ and DAZ on x86-64, FPCR.FZ on AArch64), in the helper and through the
  direct formats API. Denormal arithmetic no longer slows plugins that do not protect themselves,
  and output changes only for samples at subnormal level (about −758 dBFS). The calling thread's
  mode is restored after each call; out-of-process AUv3 extensions keep their own.
- VST3 state saves, tail reads and bus, port and capability queries, and Audio Unit state saves,
  bypass, factory preset, bus and capability queries no longer hold the lock the processing thread
  takes for each block, as both formats let the owning thread make them while the plugin
  processes. A slow state save no longer holds up processing on another thread.

### Added

- Isolated realtime processing. `Chain::realtime_endpoint` creates a `RealtimeEndpoint` for the
  prepared chain, which the application's audio callback owns: each callback of the configured
  quantum hands its input to the helper through shared memory and plays the chain's output for
  the quantum it submitted one to three callbacks before, without waiting, locking or allocating.
  When that output is missing the callback plays silence or, for effects, the input delayed by the
  whole latency (`Fallback::Dry`), ramping into and out of it over one quantum. Without misses the
  output is the offline output, `latency * quantum` frames later. `Chain::start_realtime` and
  `stop_realtime` run the helper's worker, and `realtime_status` reports misses, overflows,
  callbacks of another size, failed quanta and late completions; `last_completion` says how long
  the helper took for the output a callback just played. After a resync, stop the endpoint,
  `reset` the chain and start it again. While the endpoint runs, `restore_state`, presets,
  `select_factory_preset`, `reset`, `prepare_audio`, `reprepare` and processing through the chain
  fail with `Error::RealtimeRunning`; parameter edits, state saves and editors go on.
  A VST3 or CLAP restart request while the endpoint runs is reported through `take_changes` and
  the stream goes on with the plugin's current activation; prepare the chain again after stopping.
  The helper's worker runs with the time-constraint policy in a workgroup interval of its own and
  keeps App Nap away on macOS, and in the MMCSS "Pro Audio" task on Windows.
- The `realtime` example plays a chain through the default output device with a realtime
  endpoint and recovers it after a resynchronization. It asks the device for callbacks of one
  quantum, or of the device's nearest callback size, such as WASAPI's shared-mode period. The
  `realtime` benchmark calls an endpoint at the audio rate from a realtime thread and fails when
  the helper completes a quantum late, the endpoint resynchronizes, callbacks at p99.9 take
  longer than their limit, or a callback takes longer than its quantum.
- The `realtime-audit` feature of `plughost` marks the realtime endpoint's callback for the
  real-time audit, and `plughost_core::audit::processing_allocations` returns the allocations and
  frees it counted.
- `plughost_formats::BlockProcessor::defer_restart` lets blocks go on while a VST3 or CLAP
  restart request waits, instead of failing with `FailureKind::RestartRequired`.
- `plughost_formats::clap::classes_once_loaded` lists a CLAP module's classes and reports when
  its entry has initialized, before the classes are read and the entry is deinitialized.

### Fixed

- VST3 `parameter_details` and `parameter_choices` no longer fail with a parameter group error
  for plugins that leave the root unit out of their unit list, as the VST3 SDK examples do. The
  group path ends below the implicit root.
- VST3 `set_bypass` works before `prepare_audio`, like `bypass`, `set_parameter` on the bypass
  parameter and the CLAP and Audio Unit bypass, instead of failing with `NotPrepared`.
- `audio_configurations` returns an empty list for CLAP plugins without the `audio-ports-config`
  extension, as for VST3 and Audio Units, instead of failing with an unsupported-audio error.
- `reset` succeeds for CLAP plugins that are not prepared yet, as for VST3 and Audio Units, so
  `Chain::reset` before `prepare_audio` no longer fails for chains with a CLAP slot.
- A `Timeouts` field too long to end at a representable instant, such as `Duration::MAX`, waits
  without a deadline instead of panicking in the application.
- Preparing a prepared chain again, including `reprepare()` after a CLAP restart request, no
  longer fails for plugins that cannot save their state, such as CLAP plugins without the `state`
  extension. Such a slot is prepared again in place after the others are staged; a failure from
  then on leaves the chain unprepared instead of preserving it.
- A failed reconfiguration keeps the diagnostics of every staged instance it discards, not
  only of the instance that failed, and a crash or hang while a reconfiguration releases replaced
  or discarded instances names their slot.
- Restoring state, importing or loading a preset and selecting a factory preset keep the slot's
  native bypass unless the state or preset sets it. Audio Unit bypass, which unit state does not
  hold, was lost on restore, and factory preset selection reset bypass in every format.
- A `TailPolicy::UntilSilence` hold close to the largest representable length no longer
  overflows and panics while the tail is scanned in large blocks.
- `RenderError::SessionClosed` is classified as `FailureKind::Closed` instead of `NotPrepared`:
  the session finished, was cancelled or failed, and preparing the processor again does not
  reopen it.
- Failures of `preset_providers` and `discover_presets` name no slot; they reported slot 0 of a
  chain that does not exist.
- A bundle that changed since it failed reports no failures and is not shown as excluded when its
  scan is interrupted or its helper cannot start; the stale cache entry still counted.
- The VST3 host reports `INoteExpressionController` as supported through
  `IPlugInterfaceSupport`, since it reads note expressions from it; plugins that check first no
  longer conclude that the host ignores them.
- Audio Unit class IDs load whatever the case of their hexadecimal digits, as `ScanPolicy` already
  matches them; a lowercase ID failed to instantiate.
- Audio Unit `parameter_to_plain` and `parameter_text` round discrete parameters to the whole
  value that `set_parameter` and automation select, instead of converting a fraction that units
  truncate to a different choice.
- A VST3 module whose entry succeeded but that provides no factory is exited with `bundleExit`
  or `ExitDll` before it is released, as the VST3 SDK hosts do.
- The `RenderError::Events` message no longer calls render events MIDI messages on existing ports;
  ports a processor lacks fail `Process::validate_events` instead.
- Scan helpers report `ScanStage::ModuleLoaded` as soon as a module has loaded, as documented,
  instead of after its classes were read and it was unloaded, so loading and the work after it
  each get the stall timeout instead of sharing one.
- Direct CLAP use: `HostedPlugin::prepare` after a `prepare_audio` that left ports out no longer
  fails with an unsupported-audio error for plugins without audio port activation, whose ports
  stayed active.
- Direct VST3 use: preparing a prepared plugin again holds the values block automation left,
  instead of the controller's older values, until the next block's first point.
- On macOS, a window size that a resizable editor refuses returns the window to the size the editor
  last accepted, as on Windows, instead of keeping the refused size and offering it again on every
  tick.
- On Windows, an editor smaller or larger than the system lets a window be keeps the window at the
  size the system gave it. The window used to be resized on every tick, and a resizable editor
  was offered that size each time.
- CLAP audio and note ports are scanned only while the plugin is inactive, as CLAP requires. An
  active plugin answers bus, event port and capability queries, including the helper's own after
  each preparation, with the ports it was activated with.

## [0.1.2] - 2026-10-11

### Added

- Complete public API rustdoc for the application, core, and format crates, including units,
  input limits, errors, and links to the integration guide.
- `HostedPlugin::poll_timing` offers an opportunistic owning-thread timing read. Its default
  returns `None`, preserving existing trait implementations; `timing` remains the fresh read.
- The main crate re-exports `ParameterFlags`, `Sample`, the render `Process` trait, and public
  audio, text, preset and event/diagnostic buffer limits, avoiding a separate core dependency.
- `Debug` for chains, scanners, helper monitors, scan controls, render sessions and native
  plugins, modules and processors. Formatting does not invoke plugin code or acquire an
  engine lock, and render sessions do not require the caller's processor to implement `Debug`.
- `render::Process::validate_events` lets processors reject unavailable event ports across the
  whole render before processing begins; its default preserves existing implementations.
- `InputError::PathEncoding` identifies bundle and preset paths that the helper's Unicode path
  protocol cannot represent. Chain loading and preset requests reject them before sending them to
  a helper; scans report them individually without caching them or interrupting other bundles.

### Changed

- Idle timing polls skip a busy processing engine and send only changed observations to the
  worker. CLAP reads tail after change notifications; VST3 and AU poll at a 100 ms cadence,
  retrying skipped reads. Explicit timing reads, control edits and render startup stay fresh.
- Native automation resolves and validates block edits once. CLAP and AU reuse prepared native
  point storage; VST3 resolves parameter queue indices once per assembly and holds queue locks
  while filling the block, releasing them before plugin callbacks. Control-only flushes and
  invalid-input rejection preserve pending edits, queue ordering and exact sample output.
- Unchanged scan cache entries are reused before reading executable headers. Parallel native
  scans query the Audio Unit registry within the same worker limit, alongside bundles; cache
  write failures also terminate an active registry helper.
- Render preflight sends each distinct automation target to the helper once, without fetching
  every slot's parameter list. The helper indexes metadata for both preflight and block checks;
  invalid targets anywhere in the render still fail before its first DSP call.
- Scan helpers initialize AppKit only when needed, and helper startup waits on socket/pipe
  readiness instead of sleeping between connection attempts. Interactive retries still
  initialize and activate the application before loading the plugin.
- Parameter edits, text edits and reset return the updated chain timing in their own reply,
  removing the follow-up timing request while preserving native parameter notifications.
- State and preset requests borrow the caller's payload until encoding, avoiding a full input
  copy. Encoding reserves each frame's exact size, so a large payload is held once. Completed or
  rejected writes retain at most 64 KiB of serialization scratch; closing the transport releases
  it after any pending Windows write has been cancelled and completed.
- IPC readers reuse initialized receive storage and retain bytes from subsequent frames. Large
  payloads accumulate received slices without zero-filling the unread body. Partial frames and
  deadlines remain resumable; `MessageReader` must be kept for the whole connection.
- AU completion and notification waits sleep when a non-main thread's run loop has no sources,
  instead of spinning. Completion callbacks still wake the wait and run-loop callbacks run.
- AU creation looks up its component description directly, avoiding enumeration and name
  queries for every unrelated registered component.
- VST3 converts each block's input events once into prepared scratch storage and skips copying
  held notes for empty input, preserving pre-DSP event expansion checks and automation order.
- CLAP output note-offs with an ID remove their matching note directly, preserving explicit
  address constraints. CLAP and VST3 skip wildcard note-off scans when no notes are tracked.
- Helper channel views are allocated for the negotiated channel counts during preparation
  and reused for processing and publication, avoiding repeated 512-channel stack arrays.
- Sidechain latency alignment uses bulk ring-buffer transfers while retaining lazy history
  initialization, exact samples and existing preparation memory limits.
- Offline ramp scheduling visits active ramps and indexes interval validation, preserving
  every per-sample point and equal-offset input order while avoiding scans of all ramps.
- `Error` and `PresetSearchError` display their context while `source()` retains the cause,
  avoiding duplicate messages when formatting an error chain. Format the source chain to
  include the full diagnostic; portable scan failures still retain it.
- The internal helper protocol is now 27. Applications and helpers with different protocol
  versions fail the handshake; rebuild and distribute the helper with the application.

### Fixed

- A finished scan or retry releases the scan cache lock even while a process being started on
  another thread still shares the lock file, so the next scan or retry no longer fails with
  `WouldBlock`.
- Malformed Unicode in an AU binary preset name returns a state error instead of aborting
  during conversion from Foundation strings.
- Excessively nested VST3 `moduleinfo.json` metadata is ignored before recursive parsing can
  exhaust the application stack during discovery.
- AU parameters marked `NonRealTime` remain editable but reject block automation.
- AU input rejects a system exclusive message longer than the 65,535 bytes an AU MIDI event
  holds with `InputError::EventCapacity` before scheduling the block, instead of truncating its
  length.
- Helper OS event processing and editor updates run on a 10 ms deadline even while control
  requests keep arriving, so a busy caller does not starve native timers and UI events.
- AU Cocoa editor metadata with a null bundle or no non-null class name is treated like other
  incomplete Cocoa view metadata, without constructing an invalid retained pointer or indexing an
  empty class list.
- AU preparation preserves parameter, editor and bypass edits made after an unprepared state
  restore or factory preset selection when the unit took that state, and reads timing after
  restoring it. A unit whose state did not change still receives the restored state again after
  allocation.
- Scan caches survive library upgrades when their schema and architecture are unchanged.
  Helper startup, handshake and pre-request failures preserve cached plugin failure counts.
- Plugin discovery follows each directory only once, preventing repeated scans through
  directory aliases or cyclic symlinks while retaining the caller's directory priority.
- VST3 inactive parameter flushes receive the same initialized process context as prepared
  flushes, and their output parameter changes reach the controller.
- VST3 MIDI assignment queries run outside the engine lock, allowing processing to continue
  while the controller responds.
- The VST3 host attribute list accepts empty binary values with null data and rejects null
  IDs, strings, nonempty binary data and output pointers without dereferencing them.
- Timing queries deliver pending CLAP and VST3 host edits before reading the tail, so
  `Chain::tail()` and reported-tail renders use parameter edits immediately.
- Failed CLAP processing starts preserve queued host edits for retry. MPE-only note ports
  advertise note input and output support consistently with their event ports.
- A CLAP block consumes its pending flush request, avoiding an extra audio-thread flush on
  the next idle tick.
- Event routes that exceed the first slot's input budget are rejected with
  `InputError::EventCapacity` before processing, allowing a retry without reset.
- Helper output events are validated for frame offsets, ordering, ports, note values and total
  SysEx bytes before caller audio or events change. Invalid slot reports and loaded plugin
  counts also terminate the helper as protocol errors; later requests report `Crashed`.
- Every unexpected control response terminates the helper. Losing it during any request
  invalidates shared processing state, so later processing and rendering report `NotPrepared`.
- Repreparation uses the load timeout for reloading plugins; reset uses the control timeout.
  The timeout documentation now includes editor opening and distinguishes initial preparation.
- `Chain` is `Send + Sync` on Windows as on macOS, including while a timed-out overlapped
  pipe write is pending.
- A disconnected helper socket does not raise SIGPIPE in a C or Swift embedding process
  that retains the signal's default action.
- Rendering an unprepared chain reports `NotPrepared` before checking its input buffers, for
  both sample precisions and continuous render sessions.
- Invalid event ports anywhere in a render are rejected before DSP state advances, including
  segments submitted through `RenderSession`.
- Automation ramps reach their exact endpoints and stay within their endpoint range despite
  floating-point rounding, so fades to zero or one do not fail during rendering.

## [0.1.1] - 2026-10-10

### Added

- `plughost_formats::AudioThread` and `HostedPlugin::set_audio_thread`: an owner that processes
  a plugin on a thread of its own makes the plugin's audio-thread calls between blocks there too
  (delivering edits to a prepared VST3 processor with a process call without audio, and CLAP
  `params.flush` while active, `reset` and `stop_processing`). Without one they run on the
  calling thread, as before.
- The `realtime-audit` feature of `plughost-helper` builds a helper that audits its processing
  thread: `scripts/build-helper.sh --audit` builds it, and the benchmark README describes the
  audit. It is not for shipping.
- `InputError::Events` for events that are not valid, not in offset order or not inside the
  block.

### Changed

- The internal helper protocol is 24. Applications and helpers with different protocol
  versions fail the handshake; distribute the matching helper with the application.
- The helper processes blocks on a thread of its own instead of the thread that reads the
  connection, and that thread owns the chain's processing state; the main thread changes it
  between blocks. The helper makes the audio-thread calls a plugin needs between blocks on that
  thread instead of its main thread, so a CLAP plugin's thread check sees each call on the
  thread CLAP assigns it.
- The documentation names macOS 15 as the oldest supported macOS, the oldest one tested.

### Fixed

- `set_parameter`, `set_parameter_text` and `reset` succeed while a plugin waits to be
  prepared again; they used to apply the change and still fail with `RestartRequired`. A
  `RestartRequired` failure of a block names the slot that asked to be prepared again.
- Events that are not valid, in offset order or inside the block fail with
  `InputError::Events` instead of `Error::Buffers`, in a chain and in the plugins of
  `plughost-formats`. Program change and channel pressure ignore their third byte as
  documented; a third byte above 127 used to make them invalid, and `to_midi()` now gives it
  as 0.
- The events a slot receives from the previous slot share the block's budget with that slot's
  automation. A block that exceeds it, or that a later slot cannot take, stops processing until
  `reset()`, as an output overflow does; the plugin used to refuse it as invalid input while
  the earlier slots had already processed it.
- `take_changes` waits for the helper as long as other control requests (`Timeouts::control`)
  instead of the load timeout.
- A native scan or retry drops the cache entries of bundles confirmed to be gone from disk,
  including when the last bundle was removed. Entries that cannot be checked because of an
  I/O error are preserved. Removed bundles used to stay in every saved result.
- The development helper app built by `scripts/build-helper.sh` reports the workspace version;
  it reported 0.0.5.
- The shared memory between an application and its helper is anonymous shared memory of a
  fixed size: a POSIX shared memory object on macOS and a section backed by the paging file on
  Windows. It used to be a temporary file, which a process that truncated it could make the
  other one crash with `SIGBUS` when it read the mapping.
- A VST3 plugin's request for a new bus configuration (`kIoChanged`, `kReloadComponent`) made
  before `reset` stays pending after it, so processing keeps failing with `RestartRequired`
  until the plugin is prepared again. `reset` used to clear it and process on the old buses.
- A VST3 block whose note offs and expressions without an ID, sent to every note on their key,
  are more native events than one block holds is refused with `InputError::EventCapacity`
  before the plugin processes it. The events beyond that used to be dropped, which could leave
  notes sounding. The helper preserves this typed input error for the first chain slot, which
  can be retried without a reset; a later slot's overflow still requires a reset.
- Rendering a VST3 plugin hosted directly with `plughost-formats` fails with
  `RenderError::LatencyNotApplied` while a latency the plugin announced is not applied yet, as a
  chain does.
- An Audio Unit's MIDI output keeps a system exclusive message that real-time messages
  interrupt, or that arrives over several calls; the rest of the output used to be dropped.
  Only complete messages are delivered. An unfinished message that cannot fit the byte budget
  with its terminator fails the block immediately, including when real-time bytes follow it.
- `plughost-formats` builds without unused audio-thread code warnings when VST3 and CLAP are
  disabled, including AU-only builds.

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

[Unreleased]: https://github.com/emulette/plughost/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/emulette/plughost/releases/tag/v0.2.0
[0.1.6]: https://github.com/emulette/plughost/releases/tag/v0.1.6
[0.1.5]: https://github.com/emulette/plughost/releases/tag/v0.1.5
[0.1.4]: https://github.com/emulette/plughost/releases/tag/v0.1.4
[0.1.3]: https://github.com/emulette/plughost/releases/tag/v0.1.3
[0.1.2]: https://github.com/emulette/plughost/releases/tag/v0.1.2
[0.1.1]: https://github.com/emulette/plughost/releases/tag/v0.1.1
[0.1.0]: https://github.com/emulette/plughost/releases/tag/v0.1.0
[0.0.5]: https://github.com/emulette/plughost/releases/tag/v0.0.5
[0.0.4]: https://github.com/emulette/plughost/releases/tag/v0.0.4
[0.0.3]: https://github.com/emulette/plughost/releases/tag/v0.0.3
[0.0.2]: https://github.com/emulette/plughost/releases/tag/v0.0.2
[0.0.1]: https://github.com/emulette/plughost/releases/tag/v0.0.1
