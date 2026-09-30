# plughost

plughost loads VST3, Audio Unit, and CLAP plugins for offline audio processing in Rust.
Each plugin chain runs in a helper process, isolating plugin crashes and hangs from the application.

## Installation

plughost 0.0.2 requires Rust 1.92 or newer. Supported platforms are macOS on Apple Silicon
(`aarch64-apple-darwin`) and Windows x64 (`x86_64-pc-windows-msvc`).

Add the application library to your application's `Cargo.toml`:

```toml
[dependencies]
plughost = "=0.0.2"
```

Your application also ships a separate helper executable. In that executable's package:

```toml
[dependencies]
plughost-helper = "=0.0.2"
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
[packaging script](https://github.com/emulette/plughost/blob/main/scripts/macos/package-app.sh) and
[entitlements](https://github.com/emulette/plughost/blob/main/scripts/macos/helper.entitlements)
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
streaming, state, and recovery, see the [API documentation](https://docs.rs/plughost/0.0.2/plughost/)
and the [llms.txt reference](https://github.com/emulette/plughost/blob/main/llms.txt).
The [render example](https://github.com/emulette/plughost/blob/main/crates/plughost/examples/render.rs)
is a complete offline batch: it finds a plugin class, renders several jobs, and recovers the
chain from a state snapshot when the plugin crashes or hangs.

## Capabilities and limits

- VST3 and CLAP on both supported platforms; Audio Units on macOS.
- Block timing, caller transport, automation, parameters, state, and presets.
- MIDI 1.0 and system exclusive input and output on multiple event ports, routed between slots.
- Bus configuration, sidechains, multiple output buses, and explicit mono/stereo adaptation.
- Native plugin editors, latency/tail aware rendering, streaming, and render cancellation.
- `f32` processing; `f64` where the format and plugin support it. Audio Units use `f32` only.

Processing is offline; there is no real-time audio thread, audio device I/O, or resampler.
Intel Macs, Rosetta, Linux, and cross-architecture plugin bridging are not supported.
`Scanner::discover()` reads metadata without loading plugin code; bundles without metadata are
`ScanOutcome::NotInspected`.
`Scanner::scan()` inspects native classes serially by default and can display plugin license
dialogs. Choose scan directories explicitly; parallel loading requires `Scanner::jobs(n)`.
Use `scan_controlled`, `discover_controlled`, or `retry_controlled` with a `ScanControl`
to cancel from another thread. Progress events identify the current item and stage; retain
the event's `ScanItem` to skip that item. Cancellation reaps active helpers and stops new
admissions, preserving old cache entries without counting cancelled/skipped items as failures.
Native scan results are saved atomically before completion callbacks, so an interrupted scan
keeps completed work; the cache is synced to disk once when the scan ends. Retry selected paths with `retry_controlled`; one writer may use a cache
path at a time, and cache access failures return `Error::Cache`.
Use `Scanner::policy(ScanPolicy)` to explicitly allow or block bundles and registered Audio Units.
Blocked bundles report `ScanOutcome::Blocked` and remain blocked during retry. The policy is
independent of automatic failure exclusion and is owned by the calling application.
AUv3 state restoration has known compatibility failures with the VST3 SDK's Audio Unit samples.
Query plugin capabilities before relying on optional native behavior.
[COMPATIBILITY.md](https://github.com/emulette/plughost/blob/main/COMPATIBILITY.md) lists the
installed plugins and SDK samples checked on each platform and what they passed.

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
cargo test --workspace --locked -- --include-ignored
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

For repeatable offline performance measurements, build the test assets and helper above, then run
`cargo bench -p plughost --bench offline --locked`. The [benchmark guide](https://github.com/emulette/plughost/blob/main/crates/plughost/benches/README.md)
describes its 192-case matrix, options, and output. It uses repository fixtures and does not scan
installed plugins. The same guide describes the comparison with Pedalboard for one installed plugin
(`cargo bench -p plughost --bench compare`), including direct, isolated and per-block IPC costs,
and lists its results.

Check the distributable crates together with the command below. Each crate then builds from its
packaged sources alone. Use a fresh target directory: Cargo reuses builds of an earlier package of
the same version.

```sh
cargo package --locked --target-dir target/package-check -p plughost-core -p plughost-formats -p plughost-helper -p plughost
```

## License

Licensed under either the [Apache License, Version 2.0](https://github.com/emulette/plughost/blob/main/LICENSE-APACHE)
or the [MIT license](https://github.com/emulette/plughost/blob/main/LICENSE-MIT), at your option.
Both license texts are included in each published crate.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this project by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.

VST is a registered trademark of Steinberg Media Technologies GmbH.
