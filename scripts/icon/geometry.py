"""Shared icon geometry, in Icon Composer's 1024pt canvas (y down).

Every shape is a list of cubic-bezier commands so the same data feeds the
SVG layers of the .icon and the skia renderer of the fallback .icns.
"""
import math

K = 0.5522847498  # circle kappa


class Shape:
    def __init__(self):
        self.cmds = []  # ('M', x, y) | ('C', x1, y1, x2, y2, x, y) | ('L', x, y) | ('Z',)

    def M(self, x, y):
        self.cmds.append(('M', x, y)); return self

    def L(self, x, y):
        self.cmds.append(('L', x, y)); return self

    def C(self, x1, y1, x2, y2, x, y):
        self.cmds.append(('C', x1, y1, x2, y2, x, y)); return self

    def Z(self):
        self.cmds.append(('Z',)); return self

    def ellipse(self, cx, cy, rx, ry, rot_deg=0.0, reverse=False):
        a = math.radians(rot_deg)
        ca, sa = math.cos(a), math.sin(a)

        def t(x, y):
            return (cx + x * ca - y * sa, cy + x * sa + y * ca)

        # Four quarter arcs, clockwise in y-down space (or reversed for holes).
        pts = [(rx, 0), (0, ry), (-rx, 0), (0, -ry)]
        ctrl = [
            ((rx, K * ry), (K * rx, ry)),
            ((-K * rx, ry), (-rx, K * ry)),
            ((-rx, -K * ry), (-K * rx, -ry)),
            ((K * rx, -ry), (rx, -K * ry)),
        ]
        if reverse:
            pts = [(rx, 0), (0, -ry), (-rx, 0), (0, ry)]
            ctrl = [
                ((rx, -K * ry), (K * rx, -ry)),
                ((-K * rx, -ry), (-rx, -K * ry)),
                ((-rx, K * ry), (-K * rx, ry)),
                ((K * rx, ry), (rx, K * ry)),
            ]
        self.M(*t(*pts[0]))
        for i in range(4):
            (c1, c2) = ctrl[i]
            self.C(*t(*c1), *t(*c2), *t(*pts[(i + 1) % 4]))
        return self.Z()

    def circle(self, cx, cy, r, reverse=False):
        return self.ellipse(cx, cy, r, r, 0, reverse)

    def capsule(self, x0, y0, x1, y1, r):
        """Stadium from (x0,y0) to (x1,y1) with radius r (round caps)."""
        dx, dy = x1 - x0, y1 - y0
        ln = math.hypot(dx, dy)
        ux, uy = dx / ln, dy / ln
        nx, ny = -uy, ux
        p = lambda x, y, s, t: (x + ux * s + nx * t, y + uy * s + ny * t)
        self.M(*p(x0, y0, 0, r))
        self.L(*p(x1, y1, 0, r))
        # cap at end: half circle from +n to -n through +u
        self.C(*p(x1, y1, K * r, r), *p(x1, y1, r, K * r), *p(x1, y1, r, 0))
        self.C(*p(x1, y1, r, -K * r), *p(x1, y1, K * r, -r), *p(x1, y1, 0, -r))
        self.L(*p(x0, y0, 0, -r))
        self.C(*p(x0, y0, -K * r, -r), *p(x0, y0, -r, -K * r), *p(x0, y0, -r, 0))
        self.C(*p(x0, y0, -r, K * r), *p(x0, y0, -K * r, r), *p(x0, y0, 0, r))
        return self.Z()

    def extend(self, other):
        self.cmds += other.cmds
        return self

    def transformed(self, s, tx, ty):
        out = Shape()
        for c in self.cmds:
            if c[0] == 'Z':
                out.cmds.append(c)
            else:
                v = list(c[1:])
                out.cmds.append((c[0], *[(v[i] * s + (tx if i % 2 == 0 else ty)) for i in range(len(v))]))
        return out

    def svg_d(self):
        f = lambda v: f'{v:.2f}'.rstrip('0').rstrip('.')
        parts = []
        for c in self.cmds:
            parts.append(c[0] + ' '.join(f(v) for v in c[1:]))
        return ''.join(parts)

    def skia(self):
        import skia
        p = skia.Path()
        for c in self.cmds:
            if c[0] == 'M':
                p.moveTo(*c[1:])
            elif c[0] == 'L':
                p.lineTo(*c[1:])
            elif c[0] == 'C':
                p.cubicTo(*c[1:])
            else:
                p.close()
        p.setFillType(skia.PathFillType.kWinding)
        return p


# --- Composition --------------------------------------------------------------
# A cardboard box seen from slightly above, flaps open, with a head popping out.
# Each piece is a quad, clockwise from its top-left corner.

RIM_Y = 640.0             # front edge of the opening: the head sinks behind it
BACK_Y = 556.0            # back edge of the opening
FRONT = [(232, RIM_Y), (792, RIM_Y), (764, 892), (260, 892)]
OPENING = [(284, BACK_Y), (740, BACK_Y), (792, RIM_Y), (232, RIM_Y)]
BACK_FLAP = [(262, 452), (762, 452), (740, BACK_Y), (284, BACK_Y)]
LEFT_FLAP = [(118, 566), (176, 484), (284, BACK_Y), (232, RIM_Y)]
RIGHT_FLAP = [(1024 - x, y) for x, y in (LEFT_FLAP[1], LEFT_FLAP[0], LEFT_FLAP[3], LEFT_FLAP[2])]
FRONT_FLAP = [(232, RIM_Y), (792, RIM_Y), (814, 738), (210, 738)]

# The head: (centre x, top y, height). Its chin dips just below RIM_Y.
HEAD = (512.0, 118.0, 556.0)

NOTE_C = (0.0, 0.0)
STAMP = (512.0, 818.0, 0.31)   # the note stamped on the front: centre and scale


def quad(points):
    sh = Shape().M(*points[0])
    for p in points[1:]:
        sh.L(*p)
    return sh.Z()


def stamp():
    cx, cy, s = STAMP
    # note() is drawn around NOTE_C; its ink is centred near (+36, -8).
    return note().transformed(s, cx - 36 * s, cy + 8 * s)


def note():
    """An eighth note, about 190 wide and 350 tall, around NOTE_C."""
    cx, cy = NOTE_C
    # Notehead, tilted like engraved notation.
    hx, hy = cx - 42, cy + 96
    head = Shape().ellipse(hx, hy, 80, 60, -24)
    # Stem rises from the right of the head.
    sw = 36.0
    sx1 = hx + 80 * math.cos(math.radians(-24)) - 2  # right edge of the stem
    sx0 = sx1 - sw
    top = cy - 176
    bottom = hy - 8
    r = sw / 2
    stem = (Shape().M(sx0, bottom).L(sx0, top + r)
            .C(sx0, top + r * (1 - K), sx0 + r * (1 - K), top, sx0 + r, top)
            .L(sx1 - 2, top)
            .L(sx1, top + 6)
            .L(sx1, bottom).Z())
    # Flag: sweeps right and down from the stem top, tip curling back.
    S, T = sx1 - 1, top
    flag = (Shape().M(S, T)
            .C(S + 10, T + 58, S + 78, T + 82, S + 112, T + 132)
            .C(S + 142, T + 178, S + 136, T + 230, S + 114, T + 266)
            .C(S + 104, T + 284, S + 78, T + 276, S + 86, T + 254)
            .C(S + 100, T + 216, S + 96, T + 190, S + 70, T + 168)
            .C(S + 46, T + 150, S + 20, T + 142, S, T + 140)
            .Z())
    return head.extend(stem).extend(flag)


def squircle(size=1024.0, ox=0.0, oy=0.0, n=5.0, steps=720):
    """Superellipse approximating Apple's continuous-corner icon shape."""
    s = Shape()
    a = size / 2
    cx, cy = ox + a, oy + a
    for i in range(steps):
        t = 2 * math.pi * i / steps
        ct, st = math.cos(t), math.sin(t)
        x = cx + a * math.copysign(abs(ct) ** (2 / n), ct)
        y = cy + a * math.copysign(abs(st) ** (2 / n), st)
        (s.M if i == 0 else s.L)(x, y)
    return s.Z()
