#!/usr/bin/env python3
# Usage: beat-this-reference.py <beat_this checkout> <small0.ckpt> <out dir> <name>=<file.wav> ...
#
# Runs the reference Beat This! (https://github.com/CPJKU/beat_this) on WAV files and dumps what
# `src-tauri/crates/beat-this/tests/reference.rs` checks the Rust port against: per file, the
# 22.05 kHz mono signal, the spectrogram and the logits as raw little-endian float32, and the
# beats as JSON. Needs torch, torchaudio, einops, rotary-embedding-torch, soxr and soundfile.
import json, sys, time
import numpy as np

sys.path.insert(0, sys.argv[1])
import soundfile as sf, soxr, torch
from beat_this.inference import Audio2Frames
from beat_this.model.postprocessor import Postprocessor

torch.set_num_threads(1)
frames = Audio2Frames(checkpoint_path=sys.argv[2], device="cpu")
pick = Postprocessor("minimal")
out = sys.argv[3]
for arg in sys.argv[4:]:
    name, path = arg.split("=", 1)
    signal, rate = sf.read(path, dtype="float64")
    if signal.ndim == 2:
        signal = signal.mean(1)
    signal = soxr.resample(signal, in_rate=rate, out_rate=22050) if rate != 22050 else signal
    spect = frames.spect(torch.tensor(signal, dtype=torch.float32))
    started = time.time()
    beat, downbeat = frames.spect2frames(spect)
    took = time.time() - started
    beats, downbeats = pick(beat, downbeat)
    signal.astype("<f4").tofile(f"{out}/{name}.sig22.f32")
    spect.numpy().astype("<f4").tofile(f"{out}/{name}.spect.f32")
    np.stack([beat.numpy(), downbeat.numpy()], 1).astype("<f4").tofile(f"{out}/{name}.logits.f32")
    with open(f"{out}/{name}.json", "w") as file:
        json.dump({"beats": list(map(float, beats)), "downbeats": list(map(float, downbeats)), "rate": rate}, file)
    print(f"{name}: {len(signal) / 22050:.0f} s, {len(beats)} beats, {len(downbeats)} downbeats, model {took:.1f} s on one thread")
