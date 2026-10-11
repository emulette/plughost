# Offline processing benchmark

Renders the repository's VST3 routing fixture through `plughost-formats` directly and through an
isolated `plughost::Chain`. It never scans or loads installed plugins.

```sh
scripts/build-test-plugins.sh      # Windows: pwsh -NoProfile -File scripts/build-test-plugins.ps1
scripts/build-helper.sh            # Windows: pwsh -NoProfile -File scripts/build-helper.ps1
cargo bench -p plughost --bench offline --locked
```

The matrix has 192 cases: direct and isolated processing; main-bus stereo and multiple buses
(external mono key, stereo main, stereo monitor output); no automation, eight point changes, and
one ramp across the input; one and eight serial instances; 48 and 96 kHz; and maximum block sizes
of 64, 128, 512, and 4096 frames.

Options: `--seconds` (audio per render, default 1), `--trials` (timed renders per case after one
warm-up, default 3), `--blocks` (comma-separated maximum block sizes), `--output DIR`, `--helper
FILE`, and `--assets DIR`. Relative paths resolve against `crates/plughost/`, the working directory
Cargo sets. `--help` lists them.

Each case appends one line to `results.jsonl` in the output directory (default: a new directory
under `target/benchmarks/`): its load, prepare, and state save/restore times, and per trial the
render time, audio seconds per wall second, process calls, frames, and host heap allocations. The
median render time is included. Every render is checked against the fixture's expected output
after timing ends. Timing covers `render_with_schedule`, including chain routing, IPC for the
isolated path, and the plugin's processing.

# Timeline scheduling

```sh
cargo bench -p plughost-core --bench timeline --locked
```

This native-plugin-free harness expands 5,000 successive ramps over 60 seconds at 48 kHz,
5,000 unsorted ramps with 500 overlapping targets, and 2,048 ramps mixed with duplicate-time
point edits and MIDI. It checks the sample count and fingerprints of every automation point,
audio sample and output event against the 0.1.1 results. To compare with 0.1.1, copy
`crates/plughost-core/benches/timeline.rs` and its `[[bench]]` entry into that checkout and run the same
command. The scheduling change reduced these cases from 16,520 / 8.1 / 1,081 ms to
94 / 1.7 / 21.5 ms; the output fingerprints and block counts were unchanged.

# Latency alignment

```sh
cargo test -p plughost-helper --release --lib delay_bank_benchmark -- --ignored --nocapture
```

The fixture passes 100,000 blocks of 64 or 512 f64 samples through delays of 1, 480 and 8,192
samples, checking every output's fingerprint and the final block against the expected delayed
input. For 0.1.1, copy the benchmark function into its `process/audio_timing_tests.rs` and use
the same command. Replacing per-sample queue operations with ring-buffer slice swaps reduced
480/8,192-sample delay cases from 23–27 ms to 6.6–6.7 ms at 64 frames and from 198–216 ms to
63 ms at 512 frames. One-sample delays fell from 46/220 ms to 40/146 ms. All fingerprints matched.

# Prepared channel views

```sh
cargo test -p plughost-helper --release --lib prepared_channel_views_benchmark -- --ignored --nocapture
```

The fixture routes three input channels to two outputs for 1,000,000 one-frame and eight-frame
blocks, checking the f64 result. Copy this benchmark function into 0.1.1's
`process/prepared_tests.rs` for comparison. Reserving views for the actual channel counts reduced
these cases from 434/406 ms to 67/47 ms. Existing routing tests also verify that processing does
not allocate and that direct, adapted, silent and delayed routes retain their samples.

# Output note tracking

```sh
cargo test -p plughost-formats --release --lib output_note_off_benchmark -- --ignored --nocapture
```

The fixture emits 100,000 note-offs with 4,096 held notes, checking ID and address matching,
and repeats with wildcard IDs and an empty table. Copy the benchmark function into 0.1.1's
`clap/plugin/output_events.rs` for comparison. Direct removal by ID reduced the first case
from 915–952 ms to 3–13 ms. The empty-table case stayed near 1 ms after warm-up.

# VST3 input staging

```sh
cargo test -p plughost-formats --release --test vst3 input_event_staging_benchmark -- --ignored --nocapture
```

The routing fixture holds 4,096 notes over 100,000 empty one-frame blocks and then checks that
a wildcard note-off ends all of them. The delay fixture processes 1,000 one-frame blocks with
4,096 mapped controllers per block. Both check exact audio. Copy `tests/vst3/event_benchmark.rs`
and its module declaration in `tests/vst3.rs` into 0.1.1's formats crate for comparison.
Reusing translated inputs and skipping empty-block note copies reduced the two cases from
83/275 ms to 38/190 ms.

# AU instantiation

```sh
cargo test -p plughost-formats --release --test au_state au_instantiation_benchmark -- --ignored --nocapture
```

The native AU fixture is registered once, then instantiated 300 times while checking its
metadata and parameters. Copy `tests/au_state.rs` and its test target into 0.1.1's formats crate
for comparison. Direct component lookup reduced the case from 55 ms to 5.5 ms.

# Soak runs

`cargo bench -p plughost --bench soak -- --plugin BUNDLE [--plugin BUNDLE ...] [--rounds 120]
[--seconds 60]` renders a chain of the first class of each effect bundle repeatedly, restoring
the initial states and resetting before every round, and reports reproducibility, render time,
and helper memory. In 120 one-minute rounds of a three-effect chain on both supported platforms,
no round failed and helper memory stayed level after the first rounds.

# Comparison with Pedalboard

`benches/compare` measures one installed plugin class in plughost, and `with_pedalboard.py`
repeats the measurement in [Pedalboard](https://github.com/spotify/pedalboard), whose plugin
hosting is JUCE's. Both use the same bundle, 48 kHz, stereo 32-bit input (the same white noise,
sample for sample), the same block sizes, five timed renders after one warm-up, and a reset
before every render; the median render is reported. plughost removes the reported latency and
appends no tail, so both render exactly the input length.

```sh
scripts/build-helper.sh            # Windows: pwsh -NoProfile -File scripts/build-helper.ps1
cargo bench -p plughost --bench compare -- --plugin BUNDLE [--class 0] [--seconds 10] [--trials 5] [--blocks 64,512,4096]
python crates/plughost/benches/compare/with_pedalboard.py BUNDLE [--seconds 10] [--trials 5] [--blocks 64,512,4096]
```

An Audio Unit is named `--plugin au:CLASS_ID` in the compare benchmark and by its `.component`
bundle in `with_pedalboard.py`.

plughost reports two ways of running the same plugin: **direct**, in the calling process through
`plughost-formats` (like Pedalboard), and **isolated**, through a `Chain` in a helper process. The
difference between them divided by the number of blocks is the fixed IPC cost per block. Load
times include creating the plugin; the isolated load also starts the helper. An isolated state
restore creates, restores and prepares a fresh instance before replacing the active one; the
direct restore and Pedalboard's `raw_state` apply the state to the running instance.

On both supported platforms, direct processing took about as long as Pedalboard. Isolation added
a fixed cost of about 10 to 60 µs per block: it dominates cheap plugins at 64-frame blocks and is
a small part of the render with heavier plugins or larger blocks. plughost loaded plugins and
restored state faster than Pedalboard in every measured case.

# Real-time audit

`scripts/build-helper.sh --audit` (Windows: `build-helper.ps1 -Audit`) builds the helper with
the `realtime-audit` feature, in place of the usual one. On macOS, RealtimeSanitizer, whose
runtime the build downloads, writes each allocation, lock and system call that the helper's own
code makes while it processes a block, or makes a plugin's audio-thread calls between blocks, to
the helper's standard error with its stack. The plugins' own code is left out; the host
callbacks they make from it are watched. On both platforms the helper also counts the
allocations and frees of that code, and when the application shuts it down it writes them with
the number of plugin queries (tails, latencies, properties) it made there. Run a workload against
it, such as the tests, then build the usual helper again:

```sh
scripts/build-helper.sh --audit
RTSAN_OPTIONS=halt_on_error=false cargo test --workspace --locked -- --include-ignored 2> audit.log
scripts/build-helper.sh
```
