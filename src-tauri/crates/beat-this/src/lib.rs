//! Beat and downbeat tracking with Beat This! (Foscarin, Schlüter and
//! Widmer, "Beat This! Accurate beat tracking without DBN postprocessing",
//! ISMIR 2024; code and weights MIT, <https://github.com/CPJKU/beat_this>).
//!
//! This is the small model (about two million weights), run on the CPU with
//! nothing but matrix products. It looks at the whole track, 30 seconds at
//! a time, and says for each 20 ms frame how likely a beat and a downbeat
//! are; peaks of that are the beats. Nothing here assumes a tempo or a
//! metre, so it follows a drummer who drifts, and it places a beat only to
//! the nearest frame.

mod chunks;
mod math;
mod mel;
mod model;
mod peaks;
mod resample;
mod weights;

use std::sync::OnceLock;

pub use mel::SAMPLE_RATE;

/// Frames a second the network answers at.
pub const FPS: f64 = SAMPLE_RATE as f64 / mel::HOP as f64;

static WEIGHTS: &[u8] = include_bytes!("../weights/small0.bin");

fn model() -> &'static model::Model {
    static MODEL: OnceLock<model::Model> = OnceLock::new();
    MODEL.get_or_init(|| model::Model::load(&weights::Weights::parse(WEIGHTS)))
}

/// What the network heard in a track.
#[derive(Debug, Clone, Default)]
pub struct Tracked {
    /// Every beat, in seconds from the start, to the nearest 20 ms.
    pub beats: Vec<f64>,
    /// The beats that start a bar.
    pub downbeats: Vec<f64>,
    /// `[beat, downbeat]` logits for each frame, [`FPS`] a second: above
    /// zero is more likely than not.
    pub logits: Vec<[f32; 2]>,
}

/// Tracks the beats of mono audio at any sample rate. `threads` is how many
/// 30-second stretches are worked on at once; each takes about a second of
/// one core and 100 MB.
pub fn track(samples: &[f32], sample_rate: u32, threads: usize) -> Tracked {
    let resampled;
    let samples = if sample_rate == SAMPLE_RATE {
        samples
    } else {
        resampled = resample::resample(samples, sample_rate, SAMPLE_RATE);
        &resampled
    };
    let spect = mel::spectrogram(samples);
    let logits = chunks::predict(model(), &spect, threads);
    let (beats, downbeats) = peaks::pick(&logits, FPS);
    Tracked { beats, downbeats, logits }
}

/// For the parity tests, which check each stage against the reference
/// implementation's.
#[doc(hidden)]
pub mod stages {
    pub fn resample(samples: &[f32], from: u32) -> Vec<f32> {
        crate::resample::resample(samples, from, crate::SAMPLE_RATE)
    }
    pub fn spectrogram(samples: &[f32]) -> Vec<f32> {
        crate::mel::spectrogram(samples)
    }
    pub fn logits(spect: &[f32], threads: usize) -> Vec<[f32; 2]> {
        crate::chunks::predict(crate::model(), spect, threads)
    }
}
