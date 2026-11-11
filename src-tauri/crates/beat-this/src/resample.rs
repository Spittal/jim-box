//! Resampling to the rate the network hears at, with a windowed sinc.
//!
//! Written out rather than borrowed so an output sample's time is exactly
//! its index over the rate: a resampler's delay would move every beat.

/// Zero crossings of the sinc kept either side of its centre.
const ZEROS: f64 = 16.0;
/// The filter's cutoff, as a share of the lower rate's Nyquist frequency.
const ROLLOFF: f64 = 0.94;
const KAISER_BETA: f64 = 9.0;

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// The zeroth-order modified Bessel function, by its series.
fn bessel_i0(x: f64) -> f64 {
    let (mut term, mut sum) = (1.0, 1.0);
    for k in 1..40 {
        term *= (x / 2.0 / f64::from(k)).powi(2);
        sum += term;
    }
    sum
}

/// `samples` at `from` Hz, at `to` Hz.
pub fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || samples.is_empty() || from == 0 || to == 0 {
        return samples.to_vec();
    }
    // `to / from` in lowest terms: output sample m sits at input position
    // m · step / phases, so there are only `phases` different filters.
    let divisor = gcd(u64::from(from), u64::from(to));
    let (phases, step) = ((u64::from(to) / divisor) as usize, (u64::from(from) / divisor) as usize);
    // Cycles per input sample.
    let cutoff = 0.5 * ROLLOFF * (f64::from(to) / f64::from(from)).min(1.0);
    let reach = (ZEROS / (2.0 * cutoff)).ceil() as isize;
    let taps = (2 * reach + 1) as usize;
    let mut table = vec![0.0_f32; phases * taps];
    for (phase, filter) in table.chunks_exact_mut(taps).enumerate() {
        let fraction = phase as f64 / phases as f64;
        let mut weights: Vec<f64> = (-reach..=reach)
            .map(|tap| {
                let t = tap as f64 - fraction;
                let x = 2.0 * cutoff * t;
                let sinc = if x.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
                let across = t / (reach as f64 + 1.0);
                let window = if across.abs() < 1.0 { bessel_i0(KAISER_BETA * (1.0 - across * across).sqrt()) } else { 0.0 };
                sinc * window
            })
            .collect();
        let sum: f64 = weights.iter().sum();
        weights.iter_mut().for_each(|w| *w /= sum);
        for (to, from) in filter.iter_mut().zip(weights) {
            *to = from as f32;
        }
    }

    let length = (samples.len() as u64 * phases as u64).div_ceil(step as u64) as usize;
    let mut out = Vec::with_capacity(length);
    let last = samples.len() as isize - 1;
    for m in 0..length {
        let position = m * step;
        let (centre, phase) = ((position / phases) as isize, position % phases);
        let filter = &table[phase * taps..][..taps];
        let first = centre - reach;
        let value: f32 = if first >= 0 && first + taps as isize <= samples.len() as isize {
            filter.iter().zip(&samples[first as usize..]).map(|(w, s)| w * s).sum()
        } else {
            // Past either end there is silence.
            filter.iter().enumerate().map(|(i, w)| { let at = first + i as isize; if (0..=last).contains(&at) { w * samples[at as usize] } else { 0.0 } }).sum()
        };
        out.push(value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tone keeps its frequency, level and phase through the resampler.
    #[test]
    fn a_tone_survives() {
        for from in [44100_u32, 48000, 96000, 16000] {
            let hz = 1000.0_f64;
            let input: Vec<f32> = (0..from as usize).map(|i| (std::f64::consts::TAU * hz * i as f64 / f64::from(from)).sin() as f32).collect();
            let out = resample(&input, from, 22050);
            assert!((out.len() as i64 - 22050).abs() <= 1, "{} samples from {from}", out.len());
            for (i, got) in out.iter().enumerate().skip(2000).take(18000) {
                let want = (std::f64::consts::TAU * hz * i as f64 / 22050.0).sin() as f32;
                assert!((got - want).abs() < 2e-3, "sample {i} from {from}: {got} vs {want}");
            }
        }
    }

    /// Everything above the new Nyquist frequency is gone.
    #[test]
    fn aliases_are_filtered() {
        let input: Vec<f32> = (0..44100).map(|i| (std::f64::consts::TAU * 15000.0 * f64::from(i) / 44100.0).sin() as f32).collect();
        let out = resample(&input, 44100, 22050);
        let peak = out[1000..20000].iter().fold(0.0_f32, |m, v| m.max(v.abs()));
        assert!(peak < 1e-3, "a 15 kHz tone left {peak}");
    }
}
