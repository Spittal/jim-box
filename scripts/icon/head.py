"""Cuts the head out of a portrait photo and gives it a flat, drawn look.

    swift scripts/icon/cutout.swift photo.png mask.png > landmarks.json
    python scripts/icon/head.py photo.png mask.png landmarks.json

Writes scripts/icon/head.png: the head levelled by its eye line, cut off along
the jaw, on a transparent background. build.py puts it in the box. The photo
itself isn't kept in the repo.
"""
import json
import math
import os
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageEnhance, ImageFilter
from scipy import ndimage

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, 'head.png')
K = 4            # the photo is small: work at 4x so edges come out smooth
TILT = 0.0       # degrees of the photo's head tilt to keep


def main(photo, mask, landmarks):
    marks = json.load(open(landmarks))
    img = Image.open(photo).convert('RGB')
    alpha = Image.open(mask).convert('L').resize(img.size, Image.BICUBIC)
    W, H = img.size

    # Level by the eye line, turning about the middle of the face.
    (lx, ly), (rx, ry) = marks['leftPupil'][0], marks['rightPupil'][0]
    if lx > rx:
        (lx, ly), (rx, ry) = (rx, ry), (lx, ly)
    roll = math.degrees(math.atan2(ry - ly, rx - lx)) - TILT
    bx, by, bw, bh = marks['box']
    cx, cy = bx + bw / 2, by + bh / 2
    C = int(max(W, H) * 1.5)
    ox, oy = C / 2 - cx, C / 2 - cy

    def level(im, fill):
        canvas = Image.new(im.mode, (C, C), fill)
        canvas.paste(im, (round(ox), round(oy)))
        canvas = canvas.resize((C * K, C * K), Image.LANCZOS)
        return canvas.rotate(roll, resample=Image.BICUBIC, center=(C * K / 2, C * K / 2))

    def to_level(p):
        x, y = p[0] - cx, p[1] - cy
        t = math.radians(roll)
        return ((C / 2 + x * math.cos(t) + y * math.sin(t)) * K, (C / 2 - x * math.sin(t) + y * math.cos(t)) * K)

    rgb, a = level(img, (0, 0, 0)), level(alpha, 0)

    # Keep what's above the jaw: the contour, pushed out a little, closed over
    # the top of the frame with room for the ears and hair.
    jaw = [to_level(p) for p in marks['contour']]
    jaw.sort(key=lambda p: p[0])
    mid = (C * K / 2, C * K / 2)
    grown = [(mid[0] + (x - mid[0]) * 1.04, mid[1] + (y - mid[1]) * 1.04) for x, y in jaw]
    ear = 0.16 * bw * K
    left, right = grown[0], grown[-1]
    keep = Image.new('L', rgb.size, 0)
    ImageDraw.Draw(keep).polygon(
        [(left[0] - ear, 0), (left[0] - ear, left[1]), *grown, (right[0] + ear, right[1]), (right[0] + ear, 0)], fill=255)
    keep = keep.filter(ImageFilter.GaussianBlur(1.5 * K))
    a = np.asarray(a, np.float32) / 255
    # Firm up the mask's soft edge, which also drops the people behind.
    a = np.clip((a - 0.62) / 0.3, 0, 1)
    a = ndimage.gaussian_filter(a, 1.2 * K)
    a = np.clip((a - 0.5) / 0.12 + 0.5, 0, 1) * (np.asarray(keep, np.float32) / 255)
    # Only the piece the face is in.
    labels, _ = ndimage.label(a > 0.5)
    a = a * (labels == labels[int(mid[1]), int(mid[0])])
    a = np.clip(ndimage.gaussian_filter(a, 0.5 * K) * 1.0, 0, 1)

    # Flatten the photo's noise into areas of colour, and lift it a little.
    flat = rgb
    for size in (9, 7):
        flat = flat.filter(ImageFilter.MedianFilter(size))
    flat = flat.filter(ImageFilter.UnsharpMask(radius=3 * K, percent=60, threshold=2))
    flat = ImageEnhance.Color(flat).enhance(1.22)
    flat = ImageEnhance.Contrast(flat).enhance(1.08)
    flat = ImageEnhance.Brightness(flat).enhance(1.06)

    out = np.dstack([np.asarray(flat), (a * 255 + 0.5).astype(np.uint8)])
    ys, xs = np.nonzero(a > 0.02)
    out = out[ys.min():ys.max() + 1, xs.min():xs.max() + 1]
    Image.fromarray(out, 'RGBA').save(OUT)
    print(f'Wrote {OUT} ({out.shape[1]}x{out.shape[0]})')


if __name__ == '__main__':
    main(*sys.argv[1:4])
