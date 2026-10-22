# plughost

plughost loads VST3, Audio Unit, and CLAP plugins for offline and realtime audio processing in
Rust.
Each plugin chain runs in a helper process, isolating plugin crashes and hangs from the application.

## Installation

plughost 0.1.5 requires Rust 1.92 or newer. Supported platforms are macOS 15 or newer on Apple
Silicon (`aarch64-apple-darwin`) and Windows x64 (`x86_64-pc-windows-msvc`).

Add the application library to your application's `Cargo.toml`:

```toml
[dependencies]
plughost = "=0.1.5"
```

Your application also ships a separate helper executable. In that executable's package:

```toml
[dependencies]
plughost-helper = "=0.1.5"
```

Its `main.rs` is:

```rust
fn main() -> std::process::ExitCode {
    plughost_helper::run()
}
```

Build the application and helper with matching plughost versions. Pass the helper's executable
path to `Scanner` and `Chain`; the application library starts and manages the process.
On macOS, package the helper as a background `LSUIElement` application with Hardened Runtime and
`disable-library-validation` / `allow-unsigned-executable-memory` entitlements. The repository's
[packaging script](https://github.com/emulette/plughost/blob/v0.1.5/scripts/macos/package-app.sh) and
[entitlements](https://github.com/emulette/plughost/blob/v0.1.5/scripts/macos/helper.entitlements)
provide a local, ad-hoc signed example. Distribution signing and notarization belong to the
shipping application; the script does not perform them.

## Usage

Given a `PluginRef` from `Catalog::plugins()`, render stereo effect input through a fresh chain:

```rust
use std::path::Path;
use plughost::{Chain, HostIdentity, Layout, PluginRef, RenderOptions, Timeouts, render};

fn render_effect(
    helper: &Path,
    plugin: PluginRef,
    left: &[f32],
    right: &[f32],
) -> Result<Vec<Vec<f32>>, plughost::Error> {
    let mut chain = Chain::spawn(helper, &[plugin], &HostIdentity::default(), Timeouts::default())?;
    let config = chain.main_bus_config(48_000.0, 512, Layout::Stereo, &[Layout::Stereo])?;
    chain.prepare_audio(&config)?;
    let output = render(&mut chain, &[left, right], left.len(), &[], &RenderOptions::default())?;
    Ok(output.channels)
}
```

Input channels must have equal lengths and already use the configured sample rate. Rendering
removes reported latency and appends a bounded tail. For instruments, explicit buses, automation,
streaming, state, and recovery, see the [API documentation](https://docs.rs/plughost/0.1.5/plughost/)
and the [llms.txt reference](https://github.com/emulette/plughost/blob/v0.1.5/llms.txt).
The [render example](https://github.com/emulette/plughost/blob/v0.1.5/crates/plughost/examples/render.rs)
is a complete offline batch: it finds a plugin class, renders several jobs, and recovers the
chain from a state snapshot when the plugin crashes or hangs.

## Realtime processing

An audio callback drives a prepared chain through a realtime endpoint. Each callback hands its
quantum to the helper through shared memory and plays the chain's output for the quantum it
submitted `latency` callbacks before, without waiting for the helper, locking, or allocating:

```rust
use plughost::{Chain, Error, RealtimeConfig, RealtimeEndpoint};

/// Starts 128-frame callbacks, each played one quantum later. Returns the endpoint for the audio
/// callback and the whole latency in frames: the chain's latency plus 128.
fn start(chain: &mut Chain) -> Result<(RealtimeEndpoint, u32), Error> {
    let endpoint = chain.realtime_endpoint(&RealtimeConfig::new(128))?;
    let latency = chain.start_realtime()?;
    Ok((endpoint, latency))
}

// In the audio callback, with 128-frame stereo buffers:
// endpoint.process_audio_f32(&BlockContext::new(128), &[&left_in, &right_in],
//     &mut [&mut left_out, &mut right_out], &[], &[], &mut produced)?;
```

When the helper's output is late, the callback plays silence, or the delayed input for effects,
and ramps into and out of it. The control thread reads `realtime_status` and, after a
resynchronization, calls `resync_realtime`, which resets every plugin and starts a new generation
without stopping the endpoint. Replacing a slot with `restore_state`, a preset or
`reprepare_slot` goes on under a running endpoint. `RealtimeStatus::stalled` shows how long the
helper has gone without completing a quantum, so the application can tell a hang from a crash,
which `helper_monitor` reports. `snapshot` saves each slot's state and bypass, and
`recover_realtime` moves the same endpoint to a chain it prepares in a new helper, while the
audio callback keeps it. The
[realtime example](https://github.com/emulette/plughost/blob/v0.1.5/crates/plughost/examples/realtime.rs)
plays a chain through the default output device, which it asks for callbacks of one quantum,
resynchronizes it, and recovers it after its helper crashes or hangs.

Realtime processing is measured at 48 kHz with a latency of one quantum, on Apple Silicon with
quanta of 64 and 128 frames and on Windows x64 with quanta of 480 frames, the period of
WASAPI's shared mode. The endpoint's callback never waits, locks, or allocates; the
helper's processing, the plugins' own code, and the operating system can still take longer,
which the endpoint reports as late or missed quanta.

## Capabilities and limits

- VST3 and CLAP on both supported platforms; Audio Units on macOS.
- Block timing, caller transport, automation, parameters, state, and presets.
- Notes with IDs, per-note expression, MIDI 1.0, and system exclusive input and output on
  multiple event ports, routed between slots.
- Bus configuration, sidechains, multiple output buses, and explicit mono/stereo adaptation.
- Channel layouts from mono to 7.1.4 and 9.1.6, and first- to fourth-order ambisonics, each in
  one channel order across formats; 9.1.6 is not available to CLAP plugins.
- Native plugin editors, latency/tail aware rendering, streaming, and render cancellation.
- `f32` processing; `f64` where the format and plugin support it. Audio Units use `f32` only.

There is no audio device I/O or resampler: the application's audio callback drives realtime
processing.
Intel Macs, Rosetta, Linux, and cross-architecture plugin bridging are not supported.
Metadata discovery avoids loading plugin code; native scanning and processing can display plugin
license dialogs. Scans run serially unless explicitly configured otherwise. The application owns
scan directories, allow/block policy, cancellation, and recovery decisions. See the
[scanning contract](https://github.com/emulette/plughost/blob/v0.1.5/llms.txt)
for discovery, native inspection, retry, progress, and cache behavior.
Query plugin capabilities before relying on optional native behavior.

## Crates

| Crate | Role |
|---|---|
| `plughost` | Application API: scanning, helper management, chains, editors, state, rendering |
| `plughost-core` | Shared types and render rules |
| `plughost-formats` | In-process hosting; `vst3`, `au`, and `clap` features are enabled by default |
| `plughost-helper` | Helper process body; applications supply the executable |

Most applications depend on `plughost` and use `plughost-helper` in a separate executable.
`plughost-formats` executes plugin code in the caller's process and requires native lifecycle
calls and an OS event loop on the appropriate thread. Its feature selection applies to direct
users of that crate; `plughost-helper` builds with all three format features.

docs.rs documents each crate for `aarch64-apple-darwin` (the default) and `x86_64-pc-windows-msvc`.

## Development

These commands require a repository checkout; build scripts and test plugins are not included
in the published crate packages. Use the toolchain in `rust-toolchain.toml` with rustfmt and Clippy.
macOS test assets require full Xcode and CMake. Select Xcode with `DEVELOPER_DIR` if the system's
active developer directory points to Command Line Tools. Windows requires the x64 MSVC Rust
toolchain, Visual Studio C++ build tools, Windows SDK, CMake, and PowerShell 7. Dependency checks
use `cargo-deny`.

On macOS:

```sh
scripts/build-test-plugins.sh
scripts/build-helper.sh
cargo test --workspace --locked -- --include-ignored --skip benchmark
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked
cargo deny --locked --all-features check licenses bans sources
```

`scripts/verify-macos.sh` runs these commands in order. On Windows,
`pwsh -NoProfile -File scripts/verify-windows.ps1` runs the same commands. Both asset
scripts use the same pinned VST3 SDK archive, or an existing complete SDK supplied through
`VST3_SDK_DIR`.
Tests use repository fixtures and Apple system Audio Units, include native editor windows and
intentional crashes/hangs, and require a desktop session. They do not bulk-load commercial plugins.

With [clap-validator](https://github.com/free-audio/clap-validator) installed, independently validate
both the routing effect and the synth (without filtering validator tests):

```sh
clap-validator validate -j 1 target/test-plugins/plughost-test-routing.clap target/test-plugins/plughost-test-synth.clap
```

The delay, preset and `routing-faults` fixtures intentionally expose failure modes, including restart
loops, hangs and invalid output events; their expected failures are checked by the native host tests.

For repeatable offline performance measurements, build the test assets and helper above, then run
`cargo bench -p plughost --bench offline --locked`. The [benchmark guide](https://github.com/emulette/plughost/blob/v0.1.5/crates/plughost/benches/README.md)
describes its workload matrix, options, and output. It uses repository fixtures and does not scan
installed plugins. The same guide describes the comparison with Pedalboard for one installed plugin
(`cargo bench -p plughost --bench compare`), including direct, isolated and per-block IPC costs,
and summarizes its results, and the realtime gate (`cargo bench -p plughost --bench realtime`),
which calls a realtime endpoint at the audio rate for ten minutes.

Check the distributable crates with Python 3.9 or later after building the native fixtures:

```sh
python3 scripts/package-consumer.py
```

Use `python` on Windows. The script replaces `target/package-consumer`, runs `cargo package`, then
builds a clean application and helper from the extracted crates. It scans and renders both routing
fixtures and checks state restoration and exact output samples. `--allow-dirty` permits local work
before committing.

Parser corpus tests run with the normal test suite. For bounded AddressSanitizer fuzz sessions,
install `cargo-fuzz` and run `python3 fuzz/run.py --seconds 60` with a nightly toolchain.

## License

Licensed under either the [Apache License, Version 2.0](https://github.com/emulette/plughost/blob/v0.1.5/LICENSE-APACHE)
or the [MIT license](https://github.com/emulette/plughost/blob/v0.1.5/LICENSE-MIT), at your option.
Both license texts are included in each published crate.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this project by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.

Applications distributing the helper must include the applicable third-party copyright and license
notices with their distribution. This includes the MIT-licensed
[VST3 SDK headers](https://github.com/steinbergmedia/vst3sdk/blob/master/LICENSE.txt) used by `vst3`
and [CLAP headers](https://github.com/free-audio/clap/blob/main/LICENSE) used by `clap-sys`, alongside
the licenses for the statically linked Rust dependencies. Use the notices from the dependency
versions shipped in the application.

VST is a registered trademark of Steinberg Media Technologies GmbH.
