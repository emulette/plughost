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
