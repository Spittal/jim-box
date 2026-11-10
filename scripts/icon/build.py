"""Builds every app icon asset from geometry.py + render.py + head.png.

    python scripts/icon/build.py          # writes into src-tauri/icons/

Outputs
- AppIcon.icon/   Icon Composer document: a gradient fill under three layers
                  (the box's back, the head, the box's front). Tauri compiles
                  it with Xcode 26's actool into Assets.car, so macOS 26 draws
                  the icon itself, with its dark, tinted and clear variants.
- icon.icns       Pre-rendered fallback for machines without Xcode 26 and for
                  macOS before 26, on Apple's 824pt grid.
- *.png, icon.ico The remaining sizes Tauri references, and the favicon.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile

import numpy as np
from PIL import Image
from scipy import ndimage

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import render  # noqa: E402

ROOT = os.path.abspath(os.path.join(HERE, '..', '..'))
ICONS = os.path.join(ROOT, 'src-tauri', 'icons')


def p3ish(hex_):
    h = hex_.lstrip('#')
    r, g, b = (int(h[i:i + 2], 16) / 255 for i in (0, 2, 4))
    return f'srgb:{r:.5f},{g:.5f},{b:.5f},1.00000'


def write_icon_composer(named):
    out = os.path.join(ICONS, 'AppIcon.icon')
    shutil.rmtree(out, ignore_errors=True)
    os.makedirs(os.path.join(out, 'Assets'))
    images = dict(named)
    for name in ('box-back', 'head', 'box-front'):
        images[name].save(os.path.join(out, 'Assets', f'{name}.png'), optimize=True)

    def group(title, name):
        # Painted artwork, not glass: the layers carry their own shading.
        return {
            'name': title,
            'layers': [{'name': name, 'image-name': f'{name}.png', 'glass': False,
                        'position': {'scale': 1, 'translation-in-points': [0, 0]}}],
            'shadow': {'kind': 'none', 'opacity': 0.5},
            'specular': False,
            'translucency': {'enabled': False, 'value': 0.5},
        }

    doc = {
        'fill': {
            'linear-gradient': [p3ish(render.FILL_TOP), p3ish(render.FILL_BOTTOM)],
            'orientation': {'start': {'x': 0.5, 'y': 0}, 'stop': {'x': 0.5, 'y': 1}},
        },
        # Front to back.
        'groups': [group('Box front', 'box-front'), group('Head', 'head'), group('Box back', 'box-back')],
        'supported-platforms': {'squares': ['macOS']},
    }
    with open(os.path.join(out, 'icon.json'), 'w') as f:
        json.dump(doc, f, indent=2)
        f.write('\n')


def on_grid(art):
    """Places full-bleed artwork on Apple's 824pt-in-1024 grid with its shadow."""
    a = np.asarray(art.resize((824, 824), Image.LANCZOS)).astype(np.float32) / 255
    canvas = np.zeros((1024, 1024, 4), np.float32)
    alpha = np.zeros((1024, 1024), np.float32)
    alpha[100:924, 100:924] = a[..., 3]
    shadow = ndimage.gaussian_filter(ndimage.shift(alpha, (12, 0), order=1), 14) * 0.30
    canvas[..., 3] = shadow
    # Composite art over the shadow (straight alpha).
    fa = alpha
    out_a = fa + shadow * (1 - fa)
    rgb = np.zeros((1024, 1024, 3), np.float32)
    rgb[100:924, 100:924] = a[..., :3]
    out_rgb = rgb * fa[..., None] / np.maximum(out_a, 1e-6)[..., None]
    canvas = np.concatenate([out_rgb, out_a[..., None]], -1)
    return Image.fromarray((np.clip(canvas, 0, 1) * 255 + 0.5).astype(np.uint8), 'RGBA')


def write_bitmaps(art):
    grid = on_grid(art)
    tmp = tempfile.mkdtemp()
    iconset = os.path.join(tmp, 'icon.iconset')
    os.makedirs(iconset)
    for s in (16, 32, 128, 256, 512):
        grid.resize((s, s), Image.LANCZOS).save(os.path.join(iconset, f'icon_{s}x{s}.png'))
        grid.resize((s * 2, s * 2), Image.LANCZOS).save(os.path.join(iconset, f'icon_{s}x{s}@2x.png'))
    subprocess.run(['iconutil', '-c', 'icns', iconset, '-o', os.path.join(ICONS, 'icon.icns')], check=True)
    shutil.rmtree(tmp)

    grid.save(os.path.join(ICONS, 'icon.png'))
    for name, s in {'32x32.png': 32, '128x128.png': 128, '128x128@2x.png': 256}.items():
        grid.resize((s, s), Image.LANCZOS).save(os.path.join(ICONS, name))
    # Windows assets, kept consistent with the macOS artwork.
    art.save(os.path.join(ICONS, 'icon.ico'), sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (256, 256)])
    for s in (30, 44, 71, 89, 107, 142, 150, 284, 310):
        art.resize((s, s), Image.LANCZOS).save(os.path.join(ICONS, f'Square{s}x{s}Logo.png'))
    art.resize((50, 50), Image.LANCZOS).save(os.path.join(ICONS, 'StoreLogo.png'))
    grid.resize((128, 128), Image.LANCZOS).save(os.path.join(ROOT, 'public', 'favicon.png'))


def luminance(rgb):
    lin = np.where(rgb <= 0.04045, rgb / 12.92, ((rgb + 0.055) / 1.055) ** 2.4)
    return lin @ np.array([0.2126, 0.7152, 0.0722], np.float32)


DMG_W, DMG_H = 660, 400            # keep in sync with tauri.conf.json
DMG_ICON_Y = 170                   # icon centres
DMG_APP_X, DMG_APPS_X = 180, 480


def dmg_background(scale):
    """Mid-tone indigo, so Finder's labels (black in Light Mode, white in Dark
    Mode) stay readable either way, with an arrow and a caption."""
    import skia
    W, H = DMG_W * scale, DMG_H * scale
    y, x = np.mgrid[0:H, 0:W].astype(np.float32) / scale
    t = np.clip(y / DMG_H, 0, 1)[..., None]
    rgb = render.hex3('#6F78F0') * (1 - t) + render.hex3('#5536CF') * t
    for cx in (DMG_APP_X, DMG_APPS_X):
        g = np.exp(-(((x - cx) / 120) ** 2 + ((y - DMG_ICON_Y) / 110) ** 2))[..., None]
        rgb = rgb * (1 - 0.18 * g) + render.hex3('#9AA2FF') * 0.18 * g

    # Labels sit about 80pt below the icon centres: check contrast both ways.
    band = rgb[int((DMG_ICON_Y + 70) * scale):int((DMG_ICON_Y + 95) * scale)]
    L = luminance(band.reshape(-1, 3))
    worst = min(((L.min() + 0.05) / 0.05), (1.05 / (L.max() + 0.05)))
    assert worst >= 3.0, f'label contrast too low: {worst:.2f}'

    img = Image.fromarray((np.clip(rgb, 0, 1) * 255 + 0.5).astype(np.uint8), 'RGB').convert('RGBA')
    surf = skia.Surface(np.array(img))
    c = surf.getCanvas()
    c.scale(scale, scale)
    # Arrow from the app to Applications.
    pen = skia.Paint(AntiAlias=True, Color=skia.Color4f(1, 1, 1, 0.75), Style=skia.Paint.kStroke_Style,
                     StrokeWidth=5, StrokeCap=skia.Paint.kRound_Cap, StrokeJoin=skia.Paint.kRound_Join)
    x0, x1, yy = DMG_APP_X + 92, DMG_APPS_X - 92, DMG_ICON_Y
    path = skia.Path()
    path.moveTo(x0, yy)
    path.lineTo(x1, yy)
    path.moveTo(x1 - 16, yy - 16)
    path.lineTo(x1, yy)
    path.lineTo(x1 - 16, yy + 16)
    c.drawPath(path, pen)
    # Caption.
    tf = skia.Typeface('.AppleSystemUIFont')  # San Francisco
    font = skia.Font(tf, 15)
    text = 'Drag jim-box to Applications to install it'
    width = font.measureText(text)
    c.drawString(text, (DMG_W - width) / 2, DMG_H - 60, font,
                 skia.Paint(AntiAlias=True, Color=skia.Color4f(1, 1, 1, 0.85)))
    return Image.fromarray(surf.makeImageSnapshot().toarray(colorType=skia.kRGBA_8888_ColorType), 'RGBA')


def write_dmg_background():
    out_dir = os.path.join(ROOT, 'src-tauri', 'dmg')
    os.makedirs(out_dir, exist_ok=True)
    tmp = tempfile.mkdtemp()
    one, two = os.path.join(tmp, 'bg.png'), os.path.join(tmp, 'bg@2x.png')
    dmg_background(1).save(one)
    dmg_background(2).save(two)
    # One TIFF holding both resolutions, so Finder picks the sharp one on Retina.
    subprocess.run(['tiffutil', '-cathidpicheck', one, two, '-out', os.path.join(out_dir, 'background.tiff')],
                   check=True)
    shutil.copy(two, os.path.join(tmp, 'preview.png'))
    shutil.rmtree(tmp)


def main():
    named = render.layers()
    write_icon_composer(named)
    write_bitmaps(render.flatten(named))
    write_dmg_background()
    print(f'Icons written to {ICONS}, DMG background to src-tauri/dmg/')


if __name__ == '__main__':
    main()
