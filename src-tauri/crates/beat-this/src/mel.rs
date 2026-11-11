//! The spectrogram the network reads: 128 mel bands from 30 Hz to 11 kHz,
//! 50 frames a second, as `ln(1 + 1000 · magnitude)`. It matches
//! torchaudio's `MelSpectrogram` with the settings Beat This was trained on.

use realfft::RealFftPlanner;

use crate::model::BANDS;

/// The rate the network hears at.
pub const SAMPLE_RATE: u32 = 22050;
const WINDOW: usize = 1024;
/// Samples between frames: 50 frames a second.
pub const HOP: usize = 441;
const BINS: usize = WINDOW / 2 + 1;
const LOWEST_HZ: f64 = 30.0;
const HIGHEST_HZ: f64 = 11000.0;

/// Slaney's mel scale: linear to 1 kHz, logarithmic above.
fn mel_of(hz: f64) -> f64 {
    if hz < 1000.0 {
        hz * 3.0 / 200.0
    } else {
        15.0 + (hz / 1000.0).ln() * 27.0 / 6.4_f64.ln()
    }
}

fn hz_of(mel: f64) -> f64 {
    if mel < 15.0 {
        mel * 200.0 / 3.0
    } else {
        1000.0 * ((mel - 15.0) * 6.4_f64.ln() / 27.0).exp()
    }
}

/// Triangular filters, evenly spaced in mel: per band, its first FFT bin and
/// its weights from there.
fn filters() -> Vec<(usize, Vec<f32>)> {
    let (low, high) = (mel_of(LOWEST_HZ), mel_of(HIGHEST_HZ));
    let edges: Vec<f64> = (0..BANDS + 2).map(|i| hz_of(low + (high - low) * i as f64 / (BANDS + 1) as f64)).collect();
    let bin_hz = f64::from(SAMPLE_RATE / 2) / (BINS - 1) as f64;
    (0..BANDS)
        .map(|band| {
            let (left, centre, right) = (edges[band], edges[band + 1], edges[band + 2]);
            let weight = |bin: usize| {
                let hz = bin as f64 * bin_hz;
                ((hz - left) / (centre - left)).min((right - hz) / (right - centre)).max(0.0) as f32
            };
            let first = (0..BINS).find(|&bin| weight(bin) > 0.0).unwrap_or(0);
            let weights: Vec<f32> = (first..BINS).map(weight).take_while(|&w| w > 0.0).collect();
            (first, weights)
        })
        .collect()
}

/// The spectrogram of mono audio at [`SAMPLE_RATE`], as `[frames][BANDS]`:
/// one frame every [`HOP`] samples, each centred on its sample.
pub fn spectrogram(samples: &[f32]) -> Vec<f32> {
    // Too short to mirror at its ends.
    if samples.len() <= WINDOW / 2 {
        return Vec::new();
    }
    let frames = 1 + samples.len() / HOP;
    let filters = filters();
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(WINDOW);
    let window: Vec<f32> = (0..WINDOW).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / WINDOW as f32).cos()).collect();
    let (mut input, mut spectrum) = (fft.make_input_vec(), fft.make_output_vec());
    let mut magnitude = [0.0_f32; BINS];
    let scale = (WINDOW as f32).sqrt().recip();
    let last = samples.len() as isize - 1;
    let mut out = vec![0.0; frames * BANDS];
    for (frame, bands) in out.chunks_exact_mut(BANDS).enumerate() {
        let start = (frame * HOP) as isize - (WINDOW / 2) as isize;
        for (i, value) in input.iter_mut().enumerate() {
            // Past either end the signal is mirrored about its last sample.
            let mut at = start + i as isize;
            if at < 0 {
                at = -at;
            }
            if at > last {
                at = 2 * last - at;
            }
            *value = samples[at.clamp(0, last) as usize] * window[i];
        }
        fft.process(&mut input, &mut spectrum).expect("buffers are the planned size");
        for (m, bin) in magnitude.iter_mut().zip(&spectrum) {
            *m = bin.norm() * scale;
        }
        for (band, (first, weights)) in bands.iter_mut().zip(&filters) {
            let energy: f32 = weights.iter().zip(&magnitude[*first..]).map(|(w, m)| w * m).sum();
            *band = (1000.0 * energy).ln_1p();
        }
    }
    out
}
