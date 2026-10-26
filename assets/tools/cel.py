"""Tiny cairo toolkit for style L "cel-shaded comic": thick black ink, 2-tone fills,
halftone dots on the shaded side, a white specular blob. Shared by gen_art.py."""
import cairo, math, os, random

INK = (0, 0, 0)
WHITE = (1, 1, 1)
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))  # assets/

# Current canvas scale: design units per pixel (set by canvas()).
PX = 1.0
LW = 5.0


def hexc(h):
    h = h.lstrip('#')
    return tuple(int(h[i:i + 2], 16) / 255 for i in (0, 2, 4))


def mix(a, b, t):
    return tuple(x + (y - x) * t for x, y in zip(a, b))


def darker(c, k=0.72):
    return tuple(x * k for x in c)


def lighter(c, k=0.35):
    return mix(c, WHITE, k)


def canvas(w, h, unit=None, lw_px=5.0):
    """New transparent canvas. With `unit`, user space is centred, 1 unit = `unit` px."""
    global PX, LW
    s = cairo.ImageSurface(cairo.FORMAT_ARGB32, w, h)
    c = cairo.Context(s)
    c.set_line_join(cairo.LINE_JOIN_ROUND)
    c.set_line_cap(cairo.LINE_CAP_ROUND)
    if unit and unit != 1.0:  # unit 1.0 = plain pixel space, origin top-left
        c.translate(w / 2, h / 2)
        c.scale(unit, unit)
        PX = 1 / unit
    else:
        PX = 1.0
    LW = lw_px * PX
    return s, c


def save(s, rel):
    path = os.path.join(ROOT, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    s.write_to_png(path)
    return rel


# ---------- path builders: each returns f(ctx) that appends a path ----------

def poly(pts):
    def f(c):
        c.move_to(*pts[0])
        for p in pts[1:]:
            c.line_to(*p)
        c.close_path()
    return f


def circle(x, y, r):
    def f(c):
        c.new_sub_path()
        c.arc(x, y, r, 0, 2 * math.pi)
        c.close_path()
    return f


def ellipse(x, y, rx, ry, rot=0.0):
    def f(c):
        c.save(); c.translate(x, y); c.rotate(rot); c.scale(rx, ry)
        c.new_sub_path(); c.arc(0, 0, 1, 0, 2 * math.pi); c.close_path()
        c.restore()
    return f


def spline_pts(c, pts):
    n = len(pts)
    c.move_to(*pts[0])
    for i in range(n):
        p0, p1, p2, p3 = pts[i - 1], pts[i], pts[(i + 1) % n], pts[(i + 2) % n]
        c1 = (p1[0] + (p2[0] - p0[0]) / 6, p1[1] + (p2[1] - p0[1]) / 6)
        c2 = (p2[0] - (p3[0] - p1[0]) / 6, p2[1] - (p3[1] - p1[1]) / 6)
        c.curve_to(*c1, *c2, *p2)
    c.close_path()


def spline(pts):
    return lambda c: spline_pts(c, pts)


def blob(x, y, r, wob=0.12, seed=0, n=9):
    rnd = random.Random(seed)
    pts = []
    ph = rnd.uniform(0, 6.3)
    for i in range(n):
        a = ph + i / n * 2 * math.pi
        rr = r * (1 + rnd.uniform(-wob, wob))
        pts.append((x + math.cos(a) * rr, y + math.sin(a) * rr))
    return spline(pts)


def ngon(x, y, r, n, rot=0.0):
    return poly([(x + r * math.cos(rot + i * 2 * math.pi / n), y + r * math.sin(rot + i * 2 * math.pi / n))
                 for i in range(n)])


def rrect(x, y, w, h, r):
    """Rounded rect centred on x,y."""
    def f(c):
        x0, y0, x1, y1 = x - w / 2, y - h / 2, x + w / 2, y + h / 2
        rr = min(r, w / 2, h / 2)
        c.new_sub_path()
        c.arc(x1 - rr, y0 + rr, rr, -math.pi / 2, 0)
        c.arc(x1 - rr, y1 - rr, rr, 0, math.pi / 2)
        c.arc(x0 + rr, y1 - rr, rr, math.pi / 2, math.pi)
        c.arc(x0 + rr, y0 + rr, rr, math.pi, 1.5 * math.pi)
        c.close_path()
    return f


def star_burst(x, y, r_out, r_in, n, rot=0.0, jitter=0.0, seed=0):
    rnd = random.Random(seed)
    pts = []
    for i in range(2 * n):
        a = rot + i * math.pi / n
        rr = (r_out if i % 2 == 0 else r_in) * (1 + rnd.uniform(-jitter, jitter))
        pts.append((x + math.cos(a) * rr, y + math.sin(a) * rr))
    return poly(pts)


def mirror_half(half):
    """Half outline from nose (y=0) to tail (y=0) on the +y side → closed symmetric outline."""
    return half + [(x, -y) for (x, y) in reversed(half[1:-1])]


def flip_y(pts):
    return [(x, -y) for (x, y) in pts]


def union(*fs):
    def f(c):
        for g in fs:
            g(c)
    return f


# ---------- painting ----------

def dots(c, col, sp=4.6, r=1.25, alpha=1.0):
    x0, y0, x1, y1 = c.clip_extents()
    sp *= PX; r *= PX
    c.set_source_rgba(*col, alpha)
    row = 0
    y = y0 - sp
    while y < y1 + sp:
        x = x0 - sp + (sp / 2 if row % 2 else 0)
        while x < x1 + sp:
            c.new_sub_path(); c.arc(x, y, r, 0, 2 * math.pi)
            x += sp
        y += sp * 0.866; row += 1
    c.fill()


def ink(c, pathf, lw=None, col=INK):
    c.new_path(); pathf(c)
    c.set_source_rgb(*col); c.set_line_width(lw if lw is not None else LW); c.stroke()


def fill(c, pathf, col, alpha=1.0):
    c.new_path(); pathf(c)
    c.set_source_rgba(*col, alpha); c.fill()


def cel(c, pathf, light, dark, shift=None, light_dir=(-1, -0.9), ht=True, outline=True, lw=None,
        hl=None, hl_r=None):
    """Fill `pathf` 2-tone: the shape shifted toward the light is `light`, the remaining
    crescent is `dark` with halftone dots; then outline in ink."""
    if shift is None:
        shift = 10 * PX
    dx, dy = light_dir[0] * shift, light_dir[1] * shift
    c.save()
    c.new_path(); pathf(c); c.clip()
    c.set_source_rgb(*dark); c.paint()
    c.new_path(); c.save(); c.translate(dx, dy); pathf(c); c.restore()
    c.set_source_rgb(*light); c.fill()
    if ht:
        c.new_path(); c.rectangle(-1e4, -1e4, 2e4, 2e4)
        c.save(); c.translate(dx, dy); pathf(c); c.restore()
        c.set_fill_rule(cairo.FILL_RULE_EVEN_ODD); c.clip(); c.set_fill_rule(cairo.FILL_RULE_WINDING)
        dots(c, darker(dark, 0.7))
    c.restore()
    if hl:
        fill(c, circle(hl[0], hl[1], hl_r or 4 * PX), WHITE, 0.6)
    if outline:
        ink(c, pathf, lw)


def flat(c, pathf, col, outline=True, lw=None):
    fill(c, pathf, col)
    if outline:
        ink(c, pathf, lw)


def line(c, pts, lw, col=INK, dash=None):
    c.new_path(); c.move_to(*pts[0])
    for p in pts[1:]:
        c.line_to(*p)
    c.set_source_rgb(*col); c.set_line_width(lw)
    c.set_dash(dash or [])
    c.stroke(); c.set_dash([])


def inked_line(c, pts, w, col, lw=None):
    """A thick coloured line with an ink border (pipes, beams, struts)."""
    lw = lw if lw is not None else LW
    line(c, pts, w + 2 * lw * 0.75, INK)
    line(c, pts, w, col)


def text(c, s, x, y, size, fill_col=WHITE, stroke=None, font="Bangers", rot=0.0, anchor=0.5):
    stroke = stroke if stroke is not None else size * 0.16
    c.save(); c.translate(x, y); c.rotate(rot)
    c.select_font_face(font, cairo.FONT_SLANT_NORMAL, cairo.FONT_WEIGHT_NORMAL)
    c.set_font_size(size)
    ext = c.text_extents(s)
    c.move_to(-ext.width * anchor - ext.x_bearing, -ext.y_bearing - ext.height / 2)
    c.text_path(s)
    c.set_source_rgb(*INK); c.set_line_width(stroke); c.stroke_preserve()
    c.set_source_rgb(*fill_col); c.fill()
    c.restore()
