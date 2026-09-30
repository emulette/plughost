# Compatibility

Results of the per-class compatibility check on installed plugins and VST3 SDK samples. They
describe what plughost observed on one machine; they are not vendor certification.

## What is checked

`scripts/check-plugins.ps1` (Windows) or `scripts/check-plugins.sh` (macOS) runs
`cargo run -p plughost-formats --example check` once per plugin class, each in its own process
with a time limit; Audio Units are named `au:<class ID>`. For each class the check:

- creates the plugin and prepares its declared main buses (an undeclared layout is tried as
  stereo), playing a note for instruments and a test signal for effects;
- processes at 44.1, 48, 88.2, 96, 176.4 and 192 kHz;
- edits a parameter, saves the state, restores it into a fresh instance, and compares the
  restored value or output with the original and with the unedited plugin (**state round
  trip**);
- resets the plugin and compares what it still outputs with its own idle output (**reset**).

A class passes when every rate processes, an edit round trip is verified, and reset clears
the output. Editor windows, long sessions and host-specific features such as sidechains and
events are covered by the repository's fixture tests, not by this table. Commercial plugins ran
in the license state they were installed in; the table does not distinguish trial and licensed
modes.

## Windows x64

Windows 11, plughost `e7688c6`, 2026-09-30. 38 of 42 classes pass.

| Plugin | Vendor | Version | Format | Main buses | 44.1–192 kHz | State round trip | Reset | Result |
|---|---|---|---|---|---|---|---|---|
| Blue Cat's Chorus 4 (Mono) | Blue Cat Audio | 4.5 | VST3 | Mono in, Mono out | ok | ok | cleared | PASS |
| Blue Cat's Chorus 4 (Stereo) | Blue Cat Audio | 4.5 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Blue Cat's Flanger 3 (Mono) | Blue Cat Audio | 3.5 | VST3 | Mono in, Mono out | ok | ok | cleared | PASS |
| Blue Cat's Flanger 3 (Stereo) | Blue Cat Audio | 3.5 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Blue Cat's Free Amp | Blue Cat Audio | 1.3 | VST3 | Stereo in, Stereo out | ok | ok | residue | FAIL |
| Blue Cat's Freeceiver | Blue Cat Audio | 1.2 | VST3 | None in, Stereo out | ok | ok | cleared | PASS |
| Blue Cat's FreqAnalyst 2 (Mono) | Blue Cat Audio | 2.5 | VST3 | Mono in, Mono out | ok | ok | cleared | PASS |
| Blue Cat's FreqAnalyst 2 (Stereo) | Blue Cat Audio | 2.5 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Blue Cat's Gain 3 (Dual) | Blue Cat Audio | 3.5 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Blue Cat's Gain 3 (Mono) | Blue Cat Audio | 3.5 | VST3 | Mono in, Mono out | ok | ok | cleared | PASS |
| Blue Cat's Gain 3 (Stereo) | Blue Cat Audio | 3.5 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Blue Cat's Phaser 3 (Mono) | Blue Cat Audio | 3.5 | VST3 | Mono in, Mono out | ok | ok | cleared | PASS |
| Blue Cat's Phaser 3 (Stereo) | Blue Cat Audio | 3.5 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Blue Cat's Triple EQ 4 (Dual) | Blue Cat Audio | 4.5 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Blue Cat's Triple EQ 4 (Mono) | Blue Cat Audio | 4.5 | VST3 | Mono in, Mono out | ok | ok | cleared | PASS |
| Blue Cat's Triple EQ 4 (Stereo) | Blue Cat Audio | 4.5 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-C 3 | FabFilter | 3.02 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-C 3 | FabFilter | 3.0.2.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-DS | FabFilter | 1.32 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-DS | FabFilter | 1.3.2.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-G | FabFilter | 1.42 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-G | FabFilter | 1.4.2.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-L 2 | FabFilter | 2.26 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-L 2 | FabFilter | 2.2.6.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-MB | FabFilter | 1.33 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-MB | FabFilter | 1.3.3.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-Q 4 | FabFilter | 4.13 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-Q 4 | FabFilter | 4.1.3.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-R 2 | FabFilter | 2.06 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-R 2 | FabFilter | 2.0.6.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Saturn 2 | FabFilter | 2.13 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Saturn 2 | FabFilter | 2.1.3.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Oxid | Full Bucket Music | 1.0.4 | CLAP | None in, Stereo out | ok | ok | residue | FAIL |
| Oxid | Full Bucket Music | 1.0.4 | VST3 | None in, Stereo out | ok | ok | residue | FAIL |
| ADelay | Steinberg Media Technologies | 3.8.1.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AGain SideChain VST3 | Steinberg Media Technologies | 3.8.1.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AGain VST3 | Steinberg Media Technologies | 3.8.1.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AGainSimple VST3 | Steinberg Media Technologies | 3.8.1.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Note Expression Synth | Steinberg Media Technologies | 3.8.1.0 | VST3 | None in, Stereo out | ok | ok | cleared | PASS |
| Note Expression Synth With UI | Steinberg Media Technologies | 3.8.1.0 | VST3 | None in, Stereo out | ok | ok | cleared | PASS |
| VST3 Host Checker | Steinberg Media Technologies | 3.8.1.0 | VST3 | None in, Stereo out | ok | ok | cleared | PASS |
| UTF16Name öüäéèê-やあ-مرحبًا | Steinberg Media Technologies - öüäéèê-やあ-مرحبًا | 3.8.1.0 | VST3 | Stereo in, Stereo out | ok | not verified | cleared | FAIL |

Notes on the classes that fail:

- **Blue Cat's Free Amp** and **Oxid** (both formats) keep producing audio after the host's
  reset: their reset does not clear internal state. The host's reset call is the same one that
  clears the other classes.
- **UTF16Name** is an SDK sample for Unicode names; its parameters change neither its output nor
  its saved state, so no edit can be verified.

### Long and repeated runs

`cargo bench -p plughost --bench soak -- --plugin BUNDLE ...` renders one chain of the given
effects repeatedly: every round restores the states saved before the first round, resets the
chain, and renders the same noise again, reporting whether the round reproduces the first, its
render time, and the helper's memory: its working set on Windows, and its physical footprint on
macOS, where resident size also counts plugin binaries and resources mapped from disk.

| Chain | Rounds × audio | Wall time | Rounds reproduce round 1 | Helper memory |
|---|---|---|---|---|
| Pro-Q 4 (CLAP) → Pro-R 2 (VST3) → Blue Cat's Chorus 4 Stereo (VST3) | 120 × 60 s | 151.8 s | no, max difference 6.9e-2 | 53.0 MiB at start, 64 to 78 MiB from round 10 on |
| Pro-Q 4 (CLAP) | 60 × 10 s | 3.5 s | yes, bit for bit | 32.2 MiB at round 30, 32.3 MiB at round 60 |
| Pro-R 2 (VST3) | 60 × 10 s | 13.5 s | no, max difference 6.6e-2 | 45.3 MiB at rounds 30 and 60 |
| Blue Cat's Chorus 4 Stereo (VST3) | 60 × 10 s | 3.9 s | yes, bit for bit | 22.9 MiB at rounds 30 and 60 |

No round failed. Pro-R 2's output differs between rounds after a reset in Pedalboard as well
(maximum difference 5.7e-2 for the same input), so the variation is the plugin's own. Helper
memory grows over the first rounds and then stays level.

## macOS (Apple Silicon)

macOS 15.8 on an Apple M3 Pro, plughost `d6f6225` with the Audio Unit MIDI output change,
2026-09-30. 72 of 79 classes pass. The Audio Units are the FabFilter set and the Apple units
every Mac has, all version 2; the only installed version 3 units are speech synthesizers, which
are not a hosted type. Everything here was checked automatically: editor windows only by the
repository's tests on this desktop, not by hand. The helper was ad-hoc signed, not notarized.

| Plugin | Vendor | Version | Format | Main buses | 44.1–192 kHz | State round trip | Reset | Result |
|---|---|---|---|---|---|---|---|---|
| AUBandpass | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AUDelay | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AUDistortion | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AUDynamicsProcessor | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AUFilter | Apple | 2.1.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AUGraphicEQ | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AUHighShelfFilter | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AUHipass | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AULowpass | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AULowShelfFilter | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AUMatrixReverb | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | residue | FAIL |
| AUMIDISynth | Apple | 1.0.0 | AU v2 | None in, Stereo out | ok | ok | cleared | PASS |
| AUMultibandCompressor | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AUNBandEQ | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AUNetSend | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | no editable parameter | cleared | PASS |
| AUNewPitch | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AUParametricEQ | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AUPeakLimiter | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AUPitch | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AUReverb2 | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AURogerBeep | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AURoundTripAAC | Apple | 2.0.3 | AU v2 | Stereo in, Stereo out | ok | not run | not run | FAIL |
| AUSampleDelay | Apple | 1.6.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AUSampler | Apple | 1.0.0 | AU v2 | None in, Stereo out | ok | ok | cleared | PASS |
| AUSoundIsolation | Apple | 1.6.0 | AU v2 | Mono in, Mono out | ok | ok | cleared | PASS |
| DLSMusicDevice | Apple | 1.6.0 | AU v2 | None in, Stereo out | ok | ok | residue | FAIL |
| Micro | FabFilter | 1.3.2 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Micro | FabFilter | 1.32 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Micro | FabFilter | 1.3.2.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| One | FabFilter | 3.5.2 | AU v2 | None in, Stereo out | ok | ok | cleared | PASS |
| One | FabFilter | 3.52 | CLAP | None in, Stereo out | ok | ok | cleared | PASS |
| One | FabFilter | 3.5.2.0 | VST3 | None in, Stereo out | ok | ok | cleared | PASS |
| Pro-C 2 | FabFilter | 2.2.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-C 2 | FabFilter | 2.20 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-C 2 | FabFilter | 2.2.0.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-C 3 | FabFilter | 3.0.2 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-C 3 | FabFilter | 3.02 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-C 3 | FabFilter | 3.0.2.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-DS | FabFilter | 1.3.2 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-DS | FabFilter | 1.32 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-DS | FabFilter | 1.3.2.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-G | FabFilter | 1.4.2 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-G | FabFilter | 1.42 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-G | FabFilter | 1.4.2.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-L 2 | FabFilter | 2.2.6 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-L 2 | FabFilter | 2.26 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-L 2 | FabFilter | 2.2.6.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-MB | FabFilter | 1.3.3 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-MB | FabFilter | 1.33 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-MB | FabFilter | 1.3.3.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-Q 4 | FabFilter | 4.1.2 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-Q 4 | FabFilter | 4.12 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-Q 4 | FabFilter | 4.1.2.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-R 2 | FabFilter | 2.0.6 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-R 2 | FabFilter | 2.06 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Pro-R 2 | FabFilter | 2.0.6.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Saturn 2 | FabFilter | 2.1.3 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Saturn 2 | FabFilter | 2.13 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Saturn 2 | FabFilter | 2.1.3.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Simplon | FabFilter | 1.4.2 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Simplon | FabFilter | 1.42 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Simplon | FabFilter | 1.4.2.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Timeless 3 | FabFilter | 3.1.0 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Timeless 3 | FabFilter | 3.10 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Timeless 3 | FabFilter | 3.1.0.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Twin 3 | FabFilter | 3.0.7 | AU v2 | None in, Stereo out | ok | ok | residue | FAIL |
| Twin 3 | FabFilter | 3.07 | CLAP | None in, Stereo out | ok | ok | residue | FAIL |
| Twin 3 | FabFilter | 3.0.7.0 | VST3 | None in, Stereo out | ok | ok | residue | FAIL |
| Volcano 3 | FabFilter | 3.0.9 | AU v2 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Volcano 3 | FabFilter | 3.09 | CLAP | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Volcano 3 | FabFilter | 3.0.9.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| ADelay | Steinberg Media Technologies | 3.8.1.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AGain SideChain VST3 | Steinberg Media Technologies | 3.8.1.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AGain VST3 | Steinberg Media Technologies | 3.8.1.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| AGainSimple VST3 | Steinberg Media Technologies | 3.8.1.0 | VST3 | Stereo in, Stereo out | ok | ok | cleared | PASS |
| Note Expression Synth | Steinberg Media Technologies | 3.8.1.0 | VST3 | None in, Stereo out | ok | ok | cleared | PASS |
| Note Expression Synth With UI | Steinberg Media Technologies | 3.8.1.0 | VST3 | None in, Stereo out | ok | ok | cleared | PASS |
| VST3 Host Checker | Steinberg Media Technologies | 3.8.1.0 | VST3 | None in, Stereo out | ok | ok | cleared | PASS |
| UTF16Name öüäéèê-やあ-مرحبًا | Steinberg Media Technologies - öüäéèê-やあ-مرحبًا | 3.8.1.0 | VST3 | Stereo in, Stereo out | ok | not verified | cleared | FAIL |

Notes on the classes that fail:

- **Twin 3** keeps producing audio after the host's reset in all three formats.
- **AUMatrixReverb** and **DLSMusicDevice** keep producing audio after the unit's own reset
  (`AUAudioUnit.reset`, `AudioUnitReset` for version 2 units), the call that clears the other
  Apple units. Deallocating and reallocating render resources clears both; Pedalboard's reset
  does that, and AUMatrixReverb has no residue there. plughost uses each format's native reset.
- **AURoundTripAAC** changes its reported latency partway through a render (3232 to 10464
  samples at 48 kHz). The render stops with an error, because compensation cannot follow a
  latency change within one render.
- **UTF16Name** fails as on Windows: its parameters change neither its output nor its state.

### Long and repeated runs

The soak benchmark as on Windows, with Saturn 2 in place of Blue Cat's Chorus, which is not
installed on this machine. Helper memory is the physical footprint.

| Chain | Rounds × audio | Wall time | Rounds reproduce round 1 | Helper memory |
|---|---|---|---|---|
| Pro-Q 4 (CLAP) → Pro-R 2 (VST3) → Saturn 2 (VST3) | 120 × 60 s | 155.8 s | no, max difference 7.5e-2 | 89.0 MiB at start, 12 to 35 MiB from round 10 on |
| Pro-Q 4 (CLAP) | 60 × 10 s | 1.3 s | yes, bit for bit | 18.9 MiB at round 30, 19.1 MiB at round 60 |
| Pro-R 2 (VST3) | 60 × 10 s | 10.6 s | no, max difference 6.3e-2 | 34.6 MiB at round 30, 32.0 MiB at round 60 |
| Saturn 2 (VST3) | 60 × 10 s | 4.0 s | no, max difference 6.7e-6 | 16.2 MiB at round 30, 27.9 MiB at round 60 |

No round failed, and helper memory does not grow over the run. Pro-R 2 varies between rounds as
on Windows.
