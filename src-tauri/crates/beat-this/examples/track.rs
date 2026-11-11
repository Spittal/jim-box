//! Tracks the beats of raw mono little-endian float32 audio:
//! `ffmpeg -i track.flac -ac 1 -f f32le -ar 44100 track.f32`, then
//! `cargo run --release --example track -- track.f32 44100`.
//! Prints one beat a line, in seconds, with `1` after each downbeat.

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(rate)) = (args.next(), args.next().and_then(|rate| rate.parse::<u32>().ok())) else {
        eprintln!("usage: track <mono.f32> <sample rate>");
        std::process::exit(2);
    };
    let samples: Vec<f32> =
        std::fs::read(path).expect("readable file").chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let started = std::time::Instant::now();
    let tracked = beat_this::track(&samples, rate, 1);
    eprintln!("{} beats, {} downbeats in {:.2?}", tracked.beats.len(), tracked.downbeats.len(), started.elapsed());
    for beat in &tracked.beats {
        println!("{beat:.3}{}", if tracked.downbeats.contains(beat) { " 1" } else { "" });
    }
}
