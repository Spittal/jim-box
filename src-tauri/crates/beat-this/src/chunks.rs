//! Running the network over a whole track, 30 seconds at a time.

use crate::model::{Model, BANDS};

/// Frames the network was trained on at once.
const CHUNK: usize = 1500;
/// Frames at each end of a chunk whose predictions are thrown away: the
/// network wasn't trained on its input's edges.
const BORDER: usize = 6;

/// Where each chunk starts, in frames; negative is padding before the piece.
fn starts(frames: usize) -> Vec<isize> {
    let (frames, step) = (frames as isize, (CHUNK - 2 * BORDER) as isize);
    let mut starts: Vec<isize> = (0..).map(|i| i * step - BORDER as isize).take_while(|&s| s < frames - BORDER as isize).collect();
    if starts.is_empty() {
        starts.push(-(BORDER as isize));
    }
    // Rather than a short last chunk, one that ends where the piece does.
    if frames > step {
        *starts.last_mut().expect("at least one chunk") = frames - (CHUNK - BORDER) as isize;
    }
    starts
}

/// Beat and downbeat logits for every frame of `spect` (`[frames][BANDS]`).
/// Chunks overlap by their borders, and where two cover a frame the earlier
/// one's prediction is kept. `threads` chunks run at once.
pub fn predict(model: &Model, spect: &[f32], threads: usize) -> Vec<[f32; 2]> {
    let frames = spect.len() / BANDS;
    if frames == 0 {
        return Vec::new();
    }
    let starts = starts(frames);
    let run = |start: isize| -> Vec<[f32; 2]> {
        let from = start.max(0) as usize;
        let to = ((start + CHUNK as isize) as usize).min(frames);
        let left = (-start).max(0) as usize;
        let right = (start + CHUNK as isize - frames as isize).clamp(0, BORDER as isize) as usize;
        let mut chunk = vec![0.0; (left + to - from + right) * BANDS];
        chunk[left * BANDS..][..(to - from) * BANDS].copy_from_slice(&spect[from * BANDS..to * BANDS]);
        model.forward(&chunk)
    };

    let mut predictions: Vec<Vec<[f32; 2]>> = Vec::with_capacity(starts.len());
    let threads = threads.clamp(1, starts.len());
    if threads == 1 {
        predictions.extend(starts.iter().map(|&start| run(start)));
    } else {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let done: std::sync::Mutex<Vec<(usize, Vec<[f32; 2]>)>> = std::sync::Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(&start) = starts.get(i) else { break };
                    let prediction = run(start);
                    done.lock().expect("chunk lock poisoned").push((i, prediction));
                });
            }
        });
        let mut done = done.into_inner().expect("chunk lock poisoned");
        done.sort_by_key(|(i, _)| *i);
        predictions.extend(done.into_iter().map(|(_, prediction)| prediction));
    }

    let mut out = vec![[-1000.0_f32; 2]; frames];
    // Last to first, so an earlier chunk overwrites a later one.
    for (&start, prediction) in starts.iter().zip(&predictions).rev() {
        let kept = &prediction[BORDER..prediction.len() - BORDER];
        let at = (start + BORDER as isize) as usize;
        let count = kept.len().min(frames - at);
        out[at..at + count].copy_from_slice(&kept[..count]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_cover_the_piece() {
        assert_eq!(starts(100), [-6]);
        assert_eq!(starts(1488), [-6]);
        assert_eq!(starts(1495), [-6, 1]);
        assert_eq!(starts(3000), [-6, 1482, 1506]);
        assert_eq!(starts(4470), [-6, 1482, 2970, 2976]);
    }
}
