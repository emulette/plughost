# Parser fuzzing

Install `cargo-fuzz` and a nightly Rust toolchain with a C++ compiler available:

```sh
rustup toolchain install nightly --profile minimal
cargo +nightly install cargo-fuzz --version 0.13.2 --locked
python3 fuzz/run.py --seconds 60
```

Run from a repository checkout. AU property-list parsing requires macOS; the runner skips that
one target elsewhere. `--toolchain nightly-YYYY-MM-DD` selects an installed nightly. Additional
positional arguments select individual targets, for example `moduleinfo shared_output`.

| Target | Production boundary and oracle |
|---|---|
| `vstpreset` | Native chunk/header validation; accepted chunks survive encode/decode |
| `moduleinfo` | JSON5 discovery metadata, including bounded nested values |
| `au_plist` | Foundation plist parsing, native FourCC validation, successful state/identity round trips |
| `binary_header` | Mach-O thin/fat and PE architecture inspection |
| `cache` | Native scan-cache JSON, schema and architecture filtering |
| `framing` | Actual IPC response decoding; fragmented and coalesced reads agree |
| `shared_output` | Helper-written raw shared bytes; rejected events never change caller PCM |

Versioned corpus files live beside their owning parsers under `src/**/corpus`. Normal tests assert
known decoded metadata, exact native chunks, invalid ranges, and transactional output behavior:

```sh
cargo test -p plughost -p plughost-core -p plughost-formats --lib --locked corpus
```

The runner places newly discovered inputs in `target/fuzz-corpus` and failures in
`target/fuzz-artifacts`. Each process uses AddressSanitizer, debug assertions, overflow checks,
a 128 KiB generated-input limit, a 10-second per-input timeout and a 2 GiB RSS limit. These are
fuzz-session limits, not library input-size contracts. Retain a minimized regression input in the
owning corpus and add an observable assertion when fixing a failure.

`shared_output` inputs consist of five little-endian 64-bit words (precision selector, generation,
frame count, output event count, available port count), followed by 128 PCM bytes, 64 raw event
records of 32 bytes each, and the SysEx arena. Missing payload bytes are zero. Both f32 and f64
paths are exercised through the production shared-output reader; the harness never calls a native
plugin. Other targets receive the native input bytes directly.

Private parser access uses `cfg(fuzzing)`, which cargo-fuzz sets for instrumented dependencies.
Those entry points are absent from ordinary application builds. Sanitizer success in a bounded
session does not establish that every possible input is safe.
