//! Checks against the reference implementation's own numbers for real
//! tracks, which aren't in the repository: set `BEAT_THIS_REFERENCE` to a
//! folder of `<name>.sig22.f32`, `<name>.spect.f32` and `<name>.logits.f32`
//! as a Python script dumps them. Skipped without it.

use std::path::{Path, PathBuf};
use std::time::Instant;

fn floats(path: &Path) -> Vec<f32> {
    std::fs::read(path).unwrap().chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

fn references() -> Vec<(String, PathBuf)> {
    let Ok(dir) = std::env::var("BEAT_THIS_REFERENCE") else { return Vec::new() };
    let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.strip_suffix(".sig22.f32").map(str::to_owned))
        .map(|name| (name, PathBuf::from(&dir)))
        .collect();
    found.sort();
    found
}

#[test]
fn matches_the_reference() {
    for (name, dir) in references() {
        let signal = floats(&dir.join(format!("{name}.sig22.f32")));
        let want_spect = floats(&dir.join(format!("{name}.spect.f32")));
        let want_logits = floats(&dir.join(format!("{name}.logits.f32")));

        let spect = beat_this::stages::spectrogram(&signal);
        assert_eq!(spect.len(), want_spect.len(), "{name}: spectrogram size");
        let worst = spect.iter().zip(&want_spect).map(|(a, b)| (a - b).abs()).fold(0.0_f32, f32::max);
        assert!(worst < 2e-3, "{name}: spectrogram off by {worst}");

        let started = Instant::now();
        let logits = beat_this::stages::logits(&want_spect, 1);
        let took = started.elapsed();
        assert_eq!(logits.len() * 2, want_logits.len(), "{name}: logit count");
        let worst = logits.iter().flatten().zip(&want_logits).map(|(a, b)| (a - b).abs()).fold(0.0_f32, f32::max);
        println!("{name}: {} frames in {took:.2?}, spectrogram ok, logits off by at most {worst}", logits.len());
        assert!(worst < 0.05, "{name}: logits off by {worst}");
    }
}
