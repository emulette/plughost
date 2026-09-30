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
and helper memory. [COMPATIBILITY.md](../../../COMPATIBILITY.md) lists results.

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

## Windows x64

Windows 11, plughost 0.0.1 (this repository), Pedalboard 0.9.25, 10 seconds of audio per render.
Render columns show milliseconds per render.

| Plugin | Host | Load ms | Block 64 | Block 512 | Block 4096 | State restore ms |
|---|---|---|---|---|---|---|
| FabFilter Pro-Q 4 4.1.3 (VST3) | plughost direct | 86.9 | 6.2 | 2.9 | 2.8 | 0.06 |
| | plughost isolated | 100.8 | 138.7 | 22.5 | 5.9 | 7.77 |
| | Pedalboard | 1516.3 | 8.8 | 2.8 | 2.2 | 1428.03 |
| FabFilter Pro-R 2 2.0.6 (VST3) | plughost direct | 79.8 | 195.8 | 145.4 | 143.6 | 0.03 |
| | plughost isolated | 87.5 | 359.2 | 173.6 | 150.5 | 10.74 |
| | Pedalboard | 354.4 | 213.7 | 159.7 | 162.0 | 280.61 |
| Blue Cat's Gain 3 (Stereo) 3.5 (VST3) | plughost direct | 53.3 | 7.6 | 2.9 | 2.4 | 0.32 |
| | plughost isolated | 77.2 | 157.0 | 22.8 | 5.7 | 9.41 |
| | Pedalboard | 95.6 | 16.2 | 3.4 | 1.8 | 24.71 |

Isolation added 18 to 22 µs per 64-frame block and 27 to 59 µs per 4096-frame block on this
machine; the largest value is from the heaviest plugin, where it is a small part of the render.
The cost dominates cheap plugins at small blocks and shrinks beside the plugin's own processing at
larger blocks or with heavier plugins. Direct processing is
comparable to Pedalboard. Pedalboard does not load CLAP plugins, so CLAP classes are measured in
plughost only.

## macOS (Apple Silicon)

macOS 15.8 on an Apple M3 Pro, plughost 0.0.1 (this repository), Pedalboard 0.9.25, 10 seconds
of audio per render. Blue Cat's Gain is not installed on this machine; the FabFilter plugins are
also measured as Audio Units.

| Plugin | Host | Load ms | Block 64 | Block 512 | Block 4096 | State restore ms |
|---|---|---|---|---|---|---|
| FabFilter Pro-Q 4 4.1.2 (VST3) | plughost direct | 48.3 | 4.2 | 1.3 | 1.3 | 0.04 |
| | plughost isolated | 87.4 | 75.4 | 10.4 | 2.7 | 5.75 |
| | Pedalboard | 1017.1 | 5.0 | 1.2 | 1.0 | 972.55 |
| FabFilter Pro-R 2 2.0.6 (VST3) | plughost direct | 41.0 | 207.9 | 149.0 | 148.4 | 0.02 |
| | plughost isolated | 81.0 | 284.2 | 162.8 | 154.2 | 8.03 |
| | Pedalboard | 225.3 | 208.4 | 148.3 | 152.3 | 184.14 |
| FabFilter Pro-Q 4 4.1.2 (AU) | plughost direct | 76.5 | 4.9 | 1.4 | 1.1 | 0.37 |
| | plughost isolated | 98.2 | 78.4 | 10.0 | 2.6 | 19.89 |
| | Pedalboard | crashed while loading | | | | |
| FabFilter Pro-R 2 2.0.6 (AU) | plughost direct | 56.6 | 216.7 | 158.6 | 147.4 | 0.14 |
| | plughost isolated | 86.0 | 291.3 | 160.5 | 151.6 | 8.18 |
| | Pedalboard | 287.3 | 210.1 | 158.3 | 147.3 | 203.56 |

Isolation added about 10 µs per 64-frame block and 12 to 49 µs per 4096-frame block; at larger
blocks with Pro-R 2 the difference is within the plugin's own variation between renders. Direct
processing is comparable to Pedalboard in both formats. Pedalboard crashed (segmentation fault)
loading the Pro-Q 4 Audio Unit on both attempts.

