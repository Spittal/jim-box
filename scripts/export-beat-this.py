#!/usr/bin/env python3
# Usage: export-beat-this.py small0.ckpt src-tauri/crates/beat-this/weights/small0.bin
#
# Converts a Beat This! checkpoint (https://github.com/CPJKU/beat_this, MIT) to the file the
# `beat-this` crate reads. Needs torch, and loads with `weights_only=True`, so no pickled code
# runs. Checkpoints: https://cloud.cp.jku.at/public.php/dav/files/7ik4RrBKTS273gp/<name>.ckpt
#
# Layout, little-endian: the magic `BTHS1\n`, then per tensor a u16 name length, the name,
# a u8 number of dimensions, each dimension as u32, and the values as float32, row-major.
# Batch-norm step counters are left out; the rotary frequencies, shared by every attention
# layer, are written once as `rotary.freqs`.
import struct, sys
import torch

state = torch.load(sys.argv[1], map_location="cpu", weights_only=True)["state_dict"]
with open(sys.argv[2], "wb") as out:
    out.write(b"BTHS1\n")
    count = 0
    wrote_freqs = False

    def write(name, tensor):
        global count
        encoded = name.encode()
        out.write(struct.pack("<H", len(encoded)) + encoded)
        out.write(struct.pack("<B", tensor.ndim) + struct.pack(f"<{tensor.ndim}I", *tensor.shape))
        out.write(tensor.to(torch.float32).contiguous().numpy().astype("<f4").tobytes())
        count += tensor.numel()

    for key, tensor in state.items():
        name = key.removeprefix("model.").replace("_orig_mod.", "")
        if name.endswith("num_batches_tracked"):
            continue
        if name.endswith("rotary_embed.freqs"):
            if not wrote_freqs:
                write("rotary.freqs", tensor)
                wrote_freqs = True
            continue
        write(name, tensor)
print(f"wrote {count} floats to {sys.argv[2]}")
