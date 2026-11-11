//! From the network's logits to beat times: its own "minimal"
//! post-processing, which only picks peaks and so assumes nothing about
//! tempo or metre.

/// Frames either side a peak must be the highest of: ±70 ms.
const REACH: usize = 3;

/// Frames that are the highest logit within [`REACH`] and more likely a
/// beat than not, with runs of adjacent ones merged into their mean.
fn peaks(logits: impl Fn(usize) -> f32, frames: usize) -> Vec<f64> {
    let mut found: Vec<f64> = Vec::new();
    let mut run: Option<(usize, f64, f64)> = None; // last frame, mean, count
    for frame in 0..frames {
        let value = logits(frame);
        let highest = (frame.saturating_sub(REACH)..(frame + REACH + 1).min(frames)).all(|other| logits(other) <= value);
        if !(highest && value > 0.0) {
            continue;
        }
        run = Some(match run {
            Some((last, mean, count)) if frame - last <= 1 => (frame, mean + (frame as f64 - mean) / (count + 1.0), count + 1.0),
            Some((_, mean, _)) => {
                found.push(mean);
                (frame, frame as f64, 1.0)
            }
            None => (frame, frame as f64, 1.0),
        });
    }
    if let Some((_, mean, _)) = run {
        found.push(mean);
    }
    found
}

/// Beat and downbeat times in seconds from per-frame `[beat, downbeat]`
/// logits at `fps`. Each downbeat is moved onto its nearest beat.
pub fn pick(logits: &[[f32; 2]], fps: f64) -> (Vec<f64>, Vec<f64>) {
    let beats: Vec<f64> = peaks(|frame| logits[frame][0], logits.len()).into_iter().map(|frame| frame / fps).collect();
    let mut downbeats: Vec<f64> = peaks(|frame| logits[frame][1], logits.len()).into_iter().map(|frame| frame / fps).collect();
    if !beats.is_empty() {
        for downbeat in &mut downbeats {
            let after = beats.partition_point(|beat| beat < downbeat);
            let nearest = [after.checked_sub(1), (after < beats.len()).then_some(after)]
                .into_iter()
                .flatten()
                .min_by(|&a, &b| (beats[a] - *downbeat).abs().total_cmp(&(beats[b] - *downbeat).abs()))
                .expect("beats is not empty");
            *downbeat = beats[nearest];
        }
        downbeats.dedup();
    }
    (beats, downbeats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_peaks_and_merges_neighbours() {
        let mut logits = vec![[-5.0_f32, -5.0]; 100];
        logits[10] = [3.0, 2.0];
        // Two equal neighbours are one beat between them.
        logits[35] = [2.0, -5.0];
        logits[36] = [2.0, -5.0];
        // The lower of two close peaks is dropped.
        logits[60] = [4.0, -5.0];
        logits[62] = [1.0, 1.5];
        // Below even odds.
        logits[80] = [-0.5, -5.0];
        let (beats, downbeats) = pick(&logits, 50.0);
        assert_eq!(beats, [0.2, 0.71, 1.2]);
        // The downbeat at frame 62 moves onto the beat at frame 60.
        assert_eq!(downbeats, [0.2, 1.2]);
    }
}
