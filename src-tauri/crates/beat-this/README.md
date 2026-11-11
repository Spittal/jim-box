# beat-this

The [Beat This!](https://github.com/CPJKU/beat_this) beat and downbeat tracker, run on the CPU
in plain Rust: no Python, no model runtime.

Francesco Foscarin, Jan Schlüter and Gerhard Widmer, "Beat This! Accurate beat tracking without
DBN postprocessing", ISMIR 2024. Their code and weights are MIT licensed
(`weights/LICENSE`); this is a port of the inference path, not their code.

```rust
let tracked = beat_this::track(&mono_samples, sample_rate, 2);
// tracked.beats and tracked.downbeats, in seconds, to the nearest 20 ms
```

## What's here

- `weights/small0.bin`: the small model (2.1 million weights), trained on everything but GTZAN,
  converted from `small0.ckpt` by `scripts/export-beat-this.py` at the repository's root.
  SHA-256 of the checkpoint: `6074be2c4d490c5f6101fcc374a1ec72ae93456e23bb6019783b849f5dc7d47b`.
- `src/resample.rs`, `src/mel.rs`: audio to the 128-band, 50 frames a second spectrogram the
  network was trained on.
- `src/model.rs`, `src/chunks.rs`: the network, and running it over a track 30 seconds at a time.
- `src/peaks.rs`: the reference's "minimal" post-processing, which picks peaks and assumes
  nothing about tempo or metre.

Matrix products go to Accelerate on macOS and to `matrixmultiply` elsewhere; the `portable`
feature uses the latter on macOS too, to test what other systems run. A five-minute track takes
about four seconds of one core with Accelerate and about ten without.

## Checking it against the reference

`tests/reference.rs` compares each stage with the Python implementation's own numbers, which
aren't in the repository (they're megabytes a track, of music).
`scripts/beat-this-reference.py` at the repository's root makes them from WAV files and a
checkout of the reference; then:

```sh
BEAT_THIS_REFERENCE=/path/to/folder cargo test --release --test reference -- --nocapture
```

The logits agree to about 1e-4.
