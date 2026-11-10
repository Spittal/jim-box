"""Draws the icon's layers on Icon Composer's full 1024 canvas.

`layers()` returns them back to front as RGBA images: the background, the
box's inside and flaps, the head, and the box's front. build.py writes them
into the .icon document and flattens them for the .icns fallback.
"""
import os

import numpy as np
import skia
from PIL import Image

from geometry import (BACK_FLAP, BACK_Y, FRONT, FRONT_FLAP, HEAD, LEFT_FLAP, OPENING, RIGHT_FLAP, RIM_Y, quad,
                      squircle, stamp)

N = 1024
SS = 2
HERE = os.path.dirname(os.path.abspath(__file__))
HEAD_PNG = os.path.join(HERE, 'head.png')

# Background gradient, top to bottom.
FILL_TOP = '#8A97FF'
FILL_BOTTOM = '#4319BD'


def hex3(h):
    h = h.lstrip('#')
    return np.array([int(h[i:i + 2], 16) / 255 for i in (0, 2, 4)], np.float32)


def color(h, a=1.0):
    r, g, b = hex3(h)
    return skia.Color4f(float(r), float(g), float(b), a)


class Layer:
    """A transparent 1024pt canvas, drawn at SS times the size."""

    def __init__(self):
        self.surface = skia.Surface(N * SS, N * SS)
        self.c = self.surface.getCanvas()
        self.c.clear(0)
        self.c.scale(SS, SS)

    def fill(self, shape, top, bottom=None, y0=0.0, y1=N, round_=10.0):
        """Fills a shape with a colour, or a top-to-bottom gradient between y0 and y1."""
        paint = skia.Paint(AntiAlias=True)
        if bottom is None:
            paint.setColor4f(color(top))
        else:
            paint.setShader(skia.GradientShader.MakeLinear(
                [skia.Point(0, y0), skia.Point(0, y1)], [color(top).toColor(), color(bottom).toColor()]))
        if round_:
            paint.setPathEffect(skia.CornerPathEffect.Make(round_))
        self.c.drawPath(shape.skia(), paint)

    def shade(self, shape, h, alpha, blur, dx=0.0, dy=0.0, clip=None):
        """A soft shadow of a shape, optionally kept inside another."""
        paint = skia.Paint(AntiAlias=True, Color4f=color(h, alpha),
                           MaskFilter=skia.MaskFilter.MakeBlur(skia.kNormal_BlurStyle, blur))
        self.c.save()
        if clip is not None:
            self.c.clipPath(clip.skia(), doAntiAlias=True)
        self.c.translate(dx, dy)
        self.c.drawPath(shape.skia(), paint)
        self.c.restore()

    def line(self, p0, p1, h, alpha, width):
        self.c.drawLine(*p0, *p1, skia.Paint(AntiAlias=True, Color4f=color(h, alpha), StrokeWidth=width,
                                             StrokeCap=skia.Paint.kRound_Cap))

    def image(self):
        arr = self.surface.makeImageSnapshot().toarray(colorType=skia.kRGBA_8888_ColorType,
                                                       alphaType=skia.kUnpremul_AlphaType)
        return Image.fromarray(arr, 'RGBA').resize((N, N), Image.LANCZOS)


def background():
    layer = Layer()
    layer.c.drawPaint(skia.Paint(Shader=skia.GradientShader.MakeLinear(
        [skia.Point(0, 0), skia.Point(0, N)], [color(FILL_TOP).toColor(), color(FILL_BOTTOM).toColor()])))
    # Light pooled behind the head.
    cx, top, height = HEAD
    layer.c.drawPaint(skia.Paint(Shader=skia.GradientShader.MakeRadial(
        skia.Point(cx, top + height * 0.5), 460, [color('#C9CCFF', 0.55).toColor(), color('#C9CCFF', 0).toColor()])))
    return layer.image()


def box_back():
    """What's behind the head: the ground shadow, the inside, and the back and side flaps."""
    layer = Layer()
    from geometry import Shape
    layer.shade(Shape().ellipse(512, 892, 300, 26), '#1B0B5A', 0.55, 16)
    layer.fill(quad(BACK_FLAP), '#E2B67F', '#C99A62', 452, BACK_Y)
    layer.fill(quad(OPENING), '#5A3A1E', '#7A5230', BACK_Y, RIM_Y, round_=4)
    for flap in (LEFT_FLAP, RIGHT_FLAP):
        layer.fill(quad(flap), '#F0CB96', '#DDB27A', 484, RIM_Y)
    # The back flap's fold, and the shade the side flaps catch near the box.
    layer.line(BACK_FLAP[3], BACK_FLAP[2], '#8A5E33', 0.55, 4)
    layer.shade(quad(OPENING), '#5A3A1E', 0.35, 14, clip=quad(LEFT_FLAP))
    layer.shade(quad(OPENING), '#5A3A1E', 0.35, 14, clip=quad(RIGHT_FLAP))
    return layer.image()


def head():
    """The cut-out head, with a sticker's white edge and a shadow on the box behind."""
    cx, top, height = HEAD
    src = Image.open(HEAD_PNG).convert('RGBA')
    h = round(height * SS)
    w = round(src.width * h / src.height)
    src = src.resize((w, h), Image.LANCZOS)
    full = Image.new('RGBA', (N * SS, N * SS), (0, 0, 0, 0))
    full.paste(src, (round(cx * SS - w / 2), round(top * SS)))

    from scipy import ndimage
    a = np.asarray(full, np.float32)[..., 3] / 255
    # Distance from the head's edge gives a smooth outline of even width.
    dist = ndimage.distance_transform_edt(a < 0.5) / SS
    edge = np.clip(9.5 - dist, 0, 1)
    shadow = ndimage.gaussian_filter(ndimage.shift(np.maximum(a, edge), (10 * SS, 0), order=1), 12 * SS) * 0.42
    rgb = np.asarray(full, np.float32)[..., :3] / 255
    # Shadow, then the white edge, then the photo.
    out_a = shadow
    out_rgb = np.broadcast_to(hex3('#1B0B5A'), rgb.shape).copy()
    for layer_rgb, layer_a in ((np.ones(3, np.float32), edge), (rgb, a)):
        la = layer_a[..., None]
        out_rgb = out_rgb * (1 - la) + layer_rgb * la
        out_a = layer_a + out_a * (1 - layer_a)
    arr = np.dstack([out_rgb, out_a])
    return Image.fromarray((np.clip(arr, 0, 1) * 255 + 0.5).astype(np.uint8), 'RGBA').resize((N, N), Image.LANCZOS)


def box_front():
    layer = Layer()
    layer.fill(quad(FRONT), '#C8975F', '#B07F49', RIM_Y, 892, round_=14)
    # The front flap, folded down, and the shadow under it.
    layer.shade(quad(FRONT_FLAP), '#5A3A1E', 0.5, 12, dy=12, clip=quad(FRONT))
    layer.fill(stamp(), '#8A5E33', round_=0)
    layer.fill(quad(FRONT_FLAP), '#F3D09C', '#E3B981', RIM_Y, 738)
    # Light catching the fold at the rim.
    layer.line((FRONT[0][0] + 8, RIM_Y + 3), (FRONT[1][0] - 8, RIM_Y + 3), '#FFE9C4', 0.9, 5)
    return layer.image()


def layers():
    return [('background', background()), ('box-back', box_back()), ('head', head()), ('box-front', box_front())]


def flatten(named):
    """The layers composited and clipped to the icon shape: full-bleed artwork."""
    out = Image.new('RGBA', (N, N), (0, 0, 0, 0))
    for _, image in named:
        out.alpha_composite(image)
    mask = Layer()
    mask.fill(squircle(), '#FFFFFF', round_=0)
    out.putalpha(mask.image().getchannel('A'))
    return out
