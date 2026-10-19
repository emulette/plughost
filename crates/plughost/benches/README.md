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
FILE`, `--assets DIR`, and `--write-pcm`. Relative paths resolve against `crates/plughost/`, the
working directory Cargo sets. `--help` lists them.

Each case appends one line to `results.jsonl` in the output directory (default: a new directory
under `target/benchmarks/`): its load, prepare, and state save/restore times, and per trial the
render time, audio seconds per wall second, process calls, frames, and host heap allocations. The
median render time is included. Every render is checked against the fixture's expected output
after timing ends. Timing covers `render_with_schedule`, including chain routing, IPC for the
isolated path, and the plugin's processing.

# PCM compatibility

`--write-pcm` saves each verified warm-up render as `case-NNN.f32le`, numbered in `results.jsonl`
order: every output channel in turn, as headerless little-endian f32, written outside the measured
interval. To confirm that a change leaves output untouched, run the same command in both checkouts
with the same fixture directory (copy `benches/offline/` into an older checkout first) and compare
the files byte for byte:

```sh
cargo bench -p plughost --bench offline --locked -- \
  --seconds 0.125 --trials 1 --blocks 64,128,512,4096 --write-pcm \
  --assets /absolute/path/to/target/test-plugins --output "$PWD/target/pcm"
for file in /path/to/other/target/pcm/*.f32le; do cmp "$file" "target/pcm/${file##*/}"; done
```

0.1.2 matched 0.1.1 in all 192 cases.

# Optimization workloads

Each workload checks its own results. They are ignored tests; run them in release mode as shown.
To compare with an earlier release, copy the test and its module declaration into that checkout
and run the same command with that checkout's helper. Times are 0.1.1 → 0.1.2.

- Ramp scheduling: 5,000 successive ramps over 60 s, 5,000 unsorted ramps on 500 targets, and
  2,048 ramps mixed with point edits and MIDI, with unchanged output fingerprints.
  `cargo bench -p plughost-core --bench timeline --locked`: 16,520 / 8.1 / 1,081 ms →
  94 / 1.7 / 21.5 ms.
- Render automation preflight: 1,000 preflights of 4,096 points on eight CLAP slots.
  `cargo test -p plughost --release --test render_validation render_validation_benchmark -- --ignored --nocapture`:
  212 → 77 ms, one validation request instead of eight parameter lists.
- Warm scans: 30 scans of the CLAP delay fixture in fresh helpers.
  `cargo test -p plughost --release --lib warm_scan_benchmark -- --ignored --nocapture`:
  1.61 → 0.17 s.
- Cached inspection and registry scheduling: 10,000 cached inspections of one bundle, and 30 scans
  of a bundle alongside the AU registry.
  `cargo test -p plughost --release --lib cached_bundle_benchmark -- --ignored --nocapture` and
  `cargo test -p plughost --release --test scan_parallel -- --ignored --nocapture`:
  160 → 69 ms and 3.8–4.0 → 0.77–0.89 s.
- Idle timing observation: 10,000 polls of eight prepared VST3 or CLAP slots.
  `cargo test -p plughost --release --test timing_poll -- --ignored --nocapture`:
  179 → 165 ms (VST3) and 176 → 168 ms (CLAP); native CLAP tail reads per slot 10,000 → 0.
- Control timing replies: 1,000 parameter edits, text edits and resets on a CLAP slot.
  `cargo test -p plughost --release --test control_timing -- --ignored --nocapture`:
  one request per operation instead of two.
- State transfer memory: restore and recovery of a 64 MiB state.
  `cargo test -p plughost --release --test state_memory state_transfer_benchmark -- --ignored --nocapture`:
  peak additional heap 140.5 → 64.1 MiB; retained heap 76.5 MiB → under 130 KiB.
- IPC framing: 20,000 small requests and four 16 MiB payloads over the supervised transport, and
  a bare socket exchange.
  `cargo test -p plughost --release --lib supervised_framing_benchmark -- --ignored --nocapture`
  and `cargo test -p plughost-core --release --lib socket_framing_benchmark -- --ignored --nocapture`:
  156 / 211 → 137 / 202 ms; read calls per endpoint 40,000 → 20,000.
- Native automation: 1,000 blocks of 4,096 gain and bypass points per routing fixture, and AUDelay
  with 512 points per block.
  `cargo test -p plughost-formats --release --test automation native_automation_benchmark -- --ignored --nocapture`:
  VST3 210 → 156 ms, CLAP 151–166 → 110–112 ms, AU 825–832 → 813–819 ms.
- VST3 input staging: 100,000 empty blocks with 4,096 held notes, and 1,000 blocks of 4,096
  mapped controllers.
  `cargo test -p plughost-formats --release --test vst3 input_event_staging_benchmark -- --ignored --nocapture`:
  83 / 275 → 38 / 190 ms.
- Output note tracking: 100,000 CLAP note-offs with 4,096 held notes.
  `cargo test -p plughost-formats --release --lib output_note_off_benchmark -- --ignored --nocapture`:
  915–952 → 3–13 ms.
- AU instantiation: 300 instances of a registered fixture.
  `cargo test -p plughost-formats --release --test au_state au_instantiation_benchmark -- --ignored --nocapture`:
  55 → 5.5 ms.
- AU idle waits: 100 ms waits on a non-main thread without run-loop sources.
  `cargo test -p plughost-formats --release --lib au_wait_benchmark -- --ignored --nocapture`:
  thread CPU time 100–105 → under 0.4 ms.
- Latency alignment: 100,000 blocks through 1-, 480- and 8,192-sample delays.
  `cargo test -p plughost-helper --release --lib delay_bank_benchmark -- --ignored --nocapture`:
  23–27 → 6.7 ms at 64 frames and 198–216 → 63 ms at 512 frames for the longer delays.
- Prepared channel views: 1,000,000 one- and eight-frame blocks routing three inputs to two
  outputs.
  `cargo test -p plughost-helper --release --lib prepared_channel_views_benchmark -- --ignored --nocapture`:
  434 / 406 → 67 / 47 ms.

# Soak runs

`cargo bench -p plughost --bench soak -- --plugin BUNDLE [--plugin BUNDLE ...] [--rounds 120]
[--seconds 60] [--helper FILE]` renders a chain of the first class of each effect bundle repeatedly, restoring
the initial states and resetting before every round, and reports reproducibility, render time,
and helper memory. In 120 one-minute rounds of a three-effect chain on both supported platforms,
no round failed and helper memory stayed level after the first rounds. `--helper` defaults to the
helper that `scripts/build-helper.sh` builds; a relative path resolves against `crates/plughost/`.

# Comparison with Pedalboard

`benches/compare` measures one installed plugin class in plughost, and `with_pedalboard.py`
repeats the measurement in [Pedalboard](https://github.com/spotify/pedalboard), whose plugin
hosting is JUCE's. Both use the same bundle, 48 kHz, stereo 32-bit input (the same white noise,
sample for sample), the same block sizes, five timed renders after one warm-up, and a reset
before every render; the median render is reported. plughost removes the reported latency and
appends no tail, so both render exactly the input length.

```sh
scripts/build-helper.sh            # Windows: pwsh -NoProfile -File scripts/build-helper.ps1
cargo bench -p plughost --bench compare -- --plugin BUNDLE [--class 0] [--seconds 10] [--trials 5] [--blocks 64,512,4096] [--helper FILE]
python crates/plughost/benches/compare/with_pedalboard.py BUNDLE [--seconds 10] [--trials 5] [--blocks 64,512,4096]
```

An Audio Unit is named `--plugin au:CLASS_ID` in the compare benchmark and by its `.component`
bundle in `with_pedalboard.py`. `--helper` works as in the soak run.

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

# Realtime gate

Calls the realtime endpoint of a chain of the VST3 routing fixture at 48 kHz from a synthetic
audio device: a realtime thread (time-constraint and `mach_wait_until` on macOS, MMCSS
"Pro Audio" and a high-resolution waitable timer on Windows) that wakes on an absolute grid of
quanta and skips to its next slot when it wakes late. Run it on real hardware; hosted CI
runners are virtual machines without performance guarantees.

```sh
scripts/build-test-plugins.sh      # Windows: pwsh -NoProfile -File scripts/build-test-plugins.ps1
scripts/build-helper.sh            # Windows: pwsh -NoProfile -File scripts/build-helper.ps1
cargo bench -p plughost --bench realtime --locked -- --quantum 128
```

Options: `--seconds` (default 600), `--quantum` (frames, default 128), `--latency` (quanta,
default 1), `--load` (keeps every core busy), `--automation` (eight gain points per quantum),
`--control` (a gain edit from the control thread every 10 ms and a state save every 100 ms, as
an editor and an autosave make), `--stall` with `--stall-ms` (the CLAP stall-in-process fixture
holds up one process call, default 100 ms), `--callback-limit` (microseconds at p99.9, default
200), `--helper FILE`, and `--assets DIR`. One of `--crash`, `--hang`, `--overflow` and
`--rate-change` injects faults, and `--reconfigure` replaces the plugin periodically, with
`--fault-interval` (seconds, default 20), `--faults` (at most so many) and `--hang-ms`
(default 100); see below.

It prints the driver's wake delay, which is not counted against plughost, the callback time and
the helper's completion time (from submission, against the latency budget) at p99.9 and
maximum, the endpoint's counters, and the CPU (percent of one core; the application's includes
`--load`'s threads) and memory of the application and the helper. The run fails with a non-zero
exit code when the helper completed a quantum later than the latency, the endpoint
resynchronized, callbacks at p99.9 took longer than the limit, or a callback took longer than
its quantum. The longest callback is recorded but not held to the limit: the system can stop any
thread for a few hundred microseconds, which the thread cannot tell from its own work. On Windows,
firmware interrupts and the hypervisor of virtualization-based security can. With `--stall`,
misses are expected: the control thread resynchronizes the endpoint with
`Chain::resync_realtime`, without stopping it, once the resync shows, and
only the callback limits apply, as with `--automation` and `--control`, whose results are
recorded. The limit is about three times the longest callback of the first measurement.

The fault-injection runs keep the callback on one endpoint while the control thread injects a
fault and brings the endpoint back, as an application does. The control thread saves a
snapshot (`Chain::snapshot`) every second while the helper has completed a quantum within the
last 20 ms. `--crash` kills the helper process every `--fault-interval` seconds after the run
started or the last recovery, notices its exit through `helper_monitor` and recovers with
`Chain::recover_realtime` from the last snapshot. `--hang` runs the CLAP stall-in-process
fixture, whose process call does not return until the bench releases it, and recovers once
`RealtimeStatus::stalled` has reached `--hang-ms`; it then releases the old helper and drops its
chain. The fixture stalls at its ninth process call in every helper it runs, so each recovered
helper hangs again within a few quanta: the run injects `--faults` hangs (default 10) back to
back, and runs healthy after them. `--rate-change` stops the endpoint every `--fault-interval`
seconds, prepares the chain again, alternating between 48 kHz at `--quantum` frames and 44.1 kHz
at one and a half times that, creates a new endpoint and starts it. The callback thread owns the
endpoint: its driver takes the next one from a channel between two callbacks, outside the time it
measures, and returns the old one for the control thread to free, so that no callback allocates
or locks. The driver builds the new profile's buffers and grid there; the endpoint it leaves
plays its stopped fallback until then. `--overflow` makes the callback thread submit one block
of more events than a block may hold (`MAX_BLOCK_EVENTS` + 1, built before the run) every
`--fault-interval` seconds; the endpoint finds it invalid, plays its fallback and sets
`resync`, and the control thread, seeing that, calls `Chain::resync_realtime` and waits for the
first processed output. `--crash`, `--overflow` and `--rate-change` inject faults until
`--faults` if given; the run ends after `--seconds` once the last fault is recovered from. They
do not combine with each other, `--reconfigure`, `--stall` or `--control`.

`--reconfigure` is not a fault run. Every `--fault-interval` seconds the control thread
replaces the routing fixture under the running endpoint, alternately with `restore_state` of a
saved state with another gain than the current one and with `reprepare_slot(0)`; the stream must
go on. The routing fixture keeps its latency, so no replacement may resynchronize the endpoint
or make a quantum late: the run is judged like the plain one, and fails also when a replacement
returns an error or none ran. It prints the replacements made.

Besides the usual lines, a fault run prints the faults injected and recovered, the recovery time
(from noticing the fault, or for `--rate-change` from the stop, to the endpoint's first processed
output of the new generation) and the callbacks that played the fallback from the injection to
that output, per fault at the median and maximum, and the snapshots saved. The helper's usage
is unavailable once it was replaced, and `--rate-change` counts only its last endpoint's late
quanta and overflows. The `--overflow` recovery time is from the control thread seeing `resync`
to the first processed output after `resync_realtime`. The callback limits apply as always, and a
fault run fails with a non-zero exit code also when a recovery fails or times out (10 seconds to
the first processed output) or when no fault was injected. Its misses, late quanta, resyncs and fallbacks are expected and are
not judged.

On Apple Silicon, 0.1.3 passed at 64 and 128 frames with a latency of one quantum, idle and with
every core busy: no quantum completed late, the helper took at most 137 µs per quantum, and
callbacks took at most 47 µs at p99.9, also through a 100 ms helper stall. The longest took
78 µs. On Windows x64, it passed at 64, 128 and 480 frames the same way: no quantum completed
late, the helper took at most 791 µs per quantum, and callbacks took at most 58 µs at p99.9, also
through the stall. The longest took 340 µs.

On Apple Silicon, 0.1.4 passed the same profiles: no quantum completed late, the helper took at
most 189 µs per quantum, and callbacks took at most 35 µs at p99.9, also through the stall and
with automation and control edits. At 128 frames, the ten-minute fault runs recovered every
fault: 29 crashes in a median of 177 ms, 10 hangs in 156 ms, 29 overflows resynchronized in 6 ms
and 29 rate changes in 15 ms, and 59 replacements went on without a late quantum or a resync.
Callbacks took at most 38 µs at p99.9 in every run; the longest took 104 µs.

On Windows x64, 0.1.4 passed the same profiles at 64, 128 and 480 frames: no quantum completed
late, the helper took at most 698 µs per quantum, and callbacks took at most 80 µs at p99.9, also
through the stall and with automation and control edits. At 128 frames, the ten-minute fault runs
recovered every fault: 29 crashes in a median of 29 ms, 10 hangs in 30 ms, 29 overflows
resynchronized in 5 ms and 29 rate changes in 13 ms, and 29 replacements went on without a late
quantum or a resync. Callbacks took at most 50 µs at p99.9 in the fault runs; the longest callback
of all runs took 371 µs.

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
RTSAN_OPTIONS=halt_on_error=false cargo test --workspace --locked -- --include-ignored --skip benchmark 2> audit.log
scripts/build-helper.sh
```

The `realtime-audit` feature of `plughost` audits the application's side the same way: the
realtime endpoint's callback. Its test drives every path of the callback through the usual
helper and fails on any allocation or free there; on macOS, RealtimeSanitizer also stops it at the
first allocation, lock or system call. CI runs it:

```sh
RTSAN_ENABLE=1 cargo test -p plughost --locked --features realtime-audit --test realtime_audit -- --include-ignored
```
