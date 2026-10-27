#!/usr/bin/env python3
"""Generate every sprite for KEPLER CLASH! (style L, cel-shaded comic) into assets/.
Deterministic: re-running produces identical PNGs. Writes assets/manifest.json.
Usage: python3 assets/tools/gen_art.py"""
import math, json, random, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import cel as C
from cel import (hexc, mix, darker, lighter, canvas, save, poly, circle, ellipse, spline, blob, ngon,
                 rrect, star_burst, mirror_half, flip_y, union, cel, flat, fill, ink, line, inked_line,
                 text, dots, INK, WHITE)
import cairo

MANIFEST = {"version": 1, "style": "L cel-shaded comic",
            "conventions": {}, "teams": {}, "skins": {}, "ships": {}, "stations": {}, "bodies": {},
            "fx": {}, "icons": {}, "ui": {}, "emblems": {}, "audio": {}}

# ---------------------------------------------------------------- palettes
TEAMS = {  # light, dark — must match crates/ui/src/theme.rs TEAMS / NEUTRAL
    "blue": ("#4cc9f0", "#3a86ff"),
    "red": ("#e63946", "#9d0208"),
    "green": ("#80ed99", "#38a169"),
    "yellow": ("#ffd23f", "#e09f00"),
    "neutral": ("#adb5bd", "#6c757d"),
}
SKINS = {
    "vanguard": dict(name="Vanguard Navy", blurb="Clean angular naval hulls, steel and glass.",
                     metal=("#e9ecef", "#8d99ae"), glass="#caf0f8", accent="#ffd23f"),
    "hive": dict(name="Hive Brood", blurb="Organic chitin curves, glowing acid eyes.",
                 metal=("#8e6c8a", "#4a3050"), glass="#c7f464", accent="#c7f464"),
    "prism": dict(name="Prism Covenant", blurb="Faceted crystal hulls with a magenta core.",
                  metal=("#f3eaff", "#b3a2d6"), glass="#ff4fd8", accent="#ff4fd8"),
    "junker": dict(name="Junker Clans", blurb="Bolted scrap plates, rust and hazard stripes.",
                   metal=("#d99a6c", "#8c4f2b"), glass="#ffe066", accent="#ff9f1c"),
}
for k, (a, b) in TEAMS.items():
    MANIFEST["teams"][k] = {"light": a, "dark": b}
for k, s in SKINS.items():
    MANIFEST["skins"][k] = {"name": s["name"], "blurb": s["blurb"], "metal": list(s["metal"]),
                            "glass": s["glass"], "accent": s["accent"],
                            "emblem": f"sprites/emblems/{k}.png"}
TEAM_ORDER = ["blue", "red", "green", "yellow", "neutral"]


def T(h):
    return hexc(h)


# ---------------------------------------------------------------- skin-styled primitives
class Drawer:
    """Draws ship/station parts in one skin's shape language and palette."""

    def __init__(self, c, skin, team, seed=1, icon=False):
        self.c, self.skin, self.sk = c, skin, SKINS[skin]
        self.rnd = random.Random(seed)
        self.icon = icon
        if icon:  # white silhouette (tintable) with ink
            w, g = (1, 1, 1), (0.82, 0.82, 0.82)
            self.team = (w, g); self.metal = (w, g); self.glass = g; self.accent = g
        else:
            self.team = tuple(T(x) for x in TEAMS[team])
            self.metal = tuple(T(x) for x in self.sk["metal"])
            self.glass = T(self.sk["glass"]); self.accent = T(self.sk["accent"])
        self.engines, self.guns = [], []

    # shapes in this skin's language
    def jit(self, pts, amt=0.035):
        return [(x + self.rnd.uniform(-amt, amt), y + self.rnd.uniform(-amt, amt)) for x, y in pts]

    def outline_closed(self, half):
        if self.skin == "junker":
            top = self.jit(half[1:-1]); bot = self.jit(half[1:-1])
            return [half[0]] + top + [half[-1]] + [(x, -y) for x, y in reversed(bot)]
        return mirror_half(half)

    def shape(self, pts):
        """Closed outline → path in skin style."""
        if self.skin == "hive":
            return spline(pts)
        if self.skin == "junker":
            return poly(pts)
        return poly(pts)

    def round_(self, x, y, r):
        if self.skin == "hive":
            return blob(x, y, r, 0.13, self.rnd.randint(0, 1 << 30))
        if self.skin == "prism":
            return ngon(x, y, r * 1.05, 6, math.pi / 6)
        if self.skin == "junker":
            return ngon(x, y, r * 1.03, 8, math.pi / 8)
        return circle(x, y, r)

    def box_(self, x, y, w, h):
        if self.skin == "hive":
            return rrect(x, y, w, h, min(w, h) / 2)
        if self.skin == "prism":
            k = min(w, h) * 0.3
            return poly([(x - w / 2 + k, y - h / 2), (x + w / 2 - k, y - h / 2), (x + w / 2, y - h / 2 + k),
                         (x + w / 2, y + h / 2 - k), (x + w / 2 - k, y + h / 2), (x - w / 2 + k, y + h / 2),
                         (x - w / 2, y + h / 2 - k), (x - w / 2, y - h / 2 + k)])
        if self.skin == "junker":
            return rrect(x, y, w, h, min(w, h) * 0.06)
        return rrect(x, y, w, h, min(w, h) * 0.18)

    def rivets(self, pts, inset=0.0, every=0.16):
        if self.icon:
            return
        c = self.c
        n = len(pts)
        for i in range(n):
            a, b = pts[i], pts[(i + 1) % n]
            L = math.dist(a, b)
            k = int(L / every)
            for j in range(1, k):
                t = j / k
                fill(c, circle(a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, 1.4 * C.PX), INK, 0.8)

    def part(self, pathf, cols, shift=None, hl=None, hl_r=None):
        if self.icon:
            cel(self.c, pathf, cols[0], cols[1], shift=shift, ht=False)
        else:
            cel(self.c, pathf, cols[0], cols[1], shift=shift, hl=hl, hl_r=hl_r)

    # ----- ship parts
    def hull(self, half, stripe=True):
        pts = self.outline_closed(half)
        pf = self.shape(pts)
        self.part(pf, self.team, shift=0.09)
        c = self.c
        if self.icon:
            return pts
        xs = [p[0] for p in half]
        nose, tail = max(xs), min(xs)
        if self.skin == "vanguard":
            line(c, [(nose * 0.75, 0), (tail * 0.85, 0)], 2 * C.PX)
            if stripe:
                for dx in (0.0, 0.09):
                    x0 = nose * 0.55 - dx
                    w = max(y for x, y in half if x <= x0 + 0.2) if any(x <= x0 + .2 for x, y in half) else .1
                    line(c, [(x0 - 0.08, -w * 0.55), (x0, 0), (x0 - 0.08, w * 0.55)], 3.5 * C.PX, WHITE)
        elif self.skin == "hive":
            for s in (1, -1):
                pts2 = [(nose * 0.6, 0), (nose * 0.2, s * 0.05), ((nose + tail) / 2, s * 0.09), (tail * 0.8, s * 0.04)]
                c.new_path(); c.move_to(*pts2[0])
                c.curve_to(*pts2[1], *pts2[2], *pts2[3])
                c.set_source_rgb(*darker(self.team[1], 0.6)); c.set_line_width(2 * C.PX); c.stroke()
            for i, t in enumerate((0.25, 0.5, 0.75)):
                x = nose + (tail - nose) * t
                fill(c, circle(x, 0, (3.2 - i * .5) * C.PX), self.accent)
                ink(c, circle(x, 0, (3.2 - i * .5) * C.PX), 1.4 * C.PX)
        elif self.skin == "prism":
            cx = nose * 0.15
            for p in pts:
                if abs(p[1]) > 0.02:
                    line(c, [(cx, 0), p], 1.6 * C.PX, darker(self.team[1], 0.75))
            g = ngon(cx, 0, 0.07, 4)
            fill(c, g, self.accent); ink(c, g, 2 * C.PX)
        elif self.skin == "junker":
            self.rivets([(x * 0.86, y * 0.8) for x, y in pts], every=0.17)
            px = (nose + tail) / 2 - 0.1
            pf2 = rrect(px, 0.03, 0.22, 0.12, 0.01)
            cel(c, pf2, *self.metal, shift=0.02, ht=False, lw=2.2 * C.PX)
            # hazard stripes near tail
            c.save(); rrect(tail * 0.7, 0, 0.12, 0.16, 0.0)(c); c.clip()
            fill(c, rrect(tail * 0.7, 0, 0.12, 0.16, 0), T("#ffd23f"))
            for k in range(-4, 5):
                line(c, [(tail * 0.7 + k * 0.05 - 0.1, -0.1), (tail * 0.7 + k * 0.05 + 0.1, 0.1)], 0.018, INK)
            c.restore(); ink(c, rrect(tail * 0.7, 0, 0.12, 0.16, 0), 2 * C.PX)
        return pts

    def wings(self, pts, team=True):
        for side in (1, -1):
            p = pts if side == 1 else flip_y(pts)
            if self.skin == "junker":
                p = self.jit(p, 0.03)
            pf = spline(p) if self.skin == "hive" else poly(p)
            self.part(pf, self.team if team else self.metal, shift=0.08)
            if self.icon:
                continue
            if self.skin == "prism":
                cx = sum(q[0] for q in p) / len(p); cy = sum(q[1] for q in p) / len(p)
                for q in p:
                    line(self.c, [(cx, cy), q], 1.4 * C.PX, darker(self.team[1], 0.75))
            if self.skin == "junker":
                self.rivets([(q[0] * 0.95, q[1] * 0.9) for q in p], every=0.2)
            if self.skin == "hive":  # claw tips
                tip = max(p, key=lambda q: abs(q[1]))
                fill(self.c, circle(tip[0], tip[1] * 0.92, 2.6 * C.PX), self.accent)

    def engine(self, x, y, r):
        self.engines.append((x - r * 0.9, y))
        pf = self.box_(x, y, r * 1.8, r * 2)
        self.part(pf, self.metal, shift=r * 0.35)
        g = ellipse(x - r * 0.9, y, r * 0.28, r * 0.7)
        fill(self.c, g, self.accent if not self.icon else (0.6, 0.6, 0.6)); ink(self.c, g, 2 * C.PX)

    def cockpit(self, x, y, rx, ry):
        c = self.c
        if self.skin == "hive":
            pf = ellipse(x, y, rx, ry)
            flat(c, pf, self.glass, lw=2.6 * C.PX)
            if not self.icon:
                fill(c, ellipse(x + rx * 0.1, y, rx * 0.22, ry * 0.75), INK)
        elif self.skin == "prism":
            pf = poly([(x + rx * 1.3, y), (x, y - ry), (x - rx, y), (x, y + ry)])
            self.part(pf, (lighter(self.glass, .4), self.glass), shift=rx * .3)
        elif self.skin == "junker":
            pf = circle(x, y, ry)
            flat(c, pf, self.glass, lw=2.6 * C.PX)
            ink(c, circle(x, y, ry * 1.35), 2 * C.PX)
            if not self.icon:
                fill(c, circle(x - ry * .3, y - ry * .3, ry * .3), WHITE, .8)
        else:
            pf = ellipse(x, y, rx, ry)
            self.part(pf, (lighter(self.glass, .5), self.glass), shift=ry * .4)
            if not self.icon:
                fill(c, ellipse(x + rx * 0.2, y - ry * 0.35, rx * 0.35, ry * 0.2), WHITE, .85)

    def turret(self, x, y, r, barrels=1, ang=0.0):
        self.guns.append((x + math.cos(ang) * r * 2.4, y + math.sin(ang) * r * 2.4))
        for i in range(barrels):
            off = (i - (barrels - 1) / 2) * r * 0.55
            ox, oy = -math.sin(ang) * off, math.cos(ang) * off
            inked_line(self.c, [(x + ox, y + oy), (x + ox + math.cos(ang) * r * 2.3, y + oy + math.sin(ang) * r * 2.3)],
                       r * 0.38, self.metal[1], lw=2.4 * C.PX)
        self.part(self.round_(x, y, r), self.metal, shift=r * 0.35, hl=(x - r * .35, y - r * .35), hl_r=r * .2)
        fill(self.c, circle(x, y, r * 0.3), self.accent if not self.icon else (.7, .7, .7))
        ink(self.c, circle(x, y, r * 0.3), 1.6 * C.PX)

    def tank(self, x, y, r, team=False):
        self.part(self.round_(x, y, r), self.team if team else self.metal, shift=r * 0.38,
                  hl=(x - r * .4, y - r * .4), hl_r=r * .18)

    def container(self, x, y, w, h, team=False):
        pf = self.box_(x, y, w, h)
        self.part(pf, self.team if team else self.metal, shift=min(w, h) * 0.25)
        if not self.icon:
            for k in (-1, 1):
                line(self.c, [(x + k * w * 0.18, y - h * 0.38), (x + k * w * 0.18, y + h * 0.38)], 1.6 * C.PX,
                     darker(self.metal[1], 0.6))

    def dish(self, x, y, r):
        self.part(self.round_(x, y, r), (lighter(self.glass, .55), self.glass), shift=r * 0.4)
        if not self.icon:
            for k in (0.62, 0.3):
                ink(self.c, circle(x, y, r * k), 1.6 * C.PX)
            line(self.c, [(x, y), (x + r * 0.9, y - r * 0.9)], 2 * C.PX)
            fill(self.c, circle(x + r * 0.9, y - r * 0.9, 3 * C.PX), self.accent)
            ink(self.c, circle(x + r * 0.9, y - r * 0.9, 3 * C.PX), 1.5 * C.PX)

    def panel(self, x, y, w, h):
        """Solar panel."""
        pf = rrect(x, y, w, h, 0.01)
        col = (T("#4361ee"), T("#3a0ca3")) if not self.icon else self.metal
        self.part(pf, col, shift=min(w, h) * 0.15)
        if not self.icon:
            n = max(2, int(w / 0.12))
            for i in range(1, n):
                line(self.c, [(x - w / 2 + w * i / n, y - h / 2), (x - w / 2 + w * i / n, y + h / 2)], 1.3 * C.PX)
            line(self.c, [(x - w / 2, y), (x + w / 2, y)], 1.3 * C.PX)


# ---------------------------------------------------------------- ship classes
# Nose points +x. Coordinates in "design units": the 128 px frame spans -1.09..1.09.
def ship_interceptor(d):
    d.engine(-0.78, 0, 0.13)
    d.wings([(0.15, 0.12), (-0.5, 0.86), (-0.74, 0.86), (-0.55, 0.12)])
    d.hull([(1.0, 0), (0.55, 0.12), (-0.2, 0.19), (-0.72, 0.15), (-0.8, 0)])
    d.cockpit(0.35, 0, 0.17, 0.08)
    d.guns += [(0.0, 0.62), (0.0, -0.62)]


def ship_gunship(d):
    d.engine(-0.8, 0.24, 0.14); d.engine(-0.8, -0.24, 0.14)
    d.wings([(0.35, 0.3), (0.12, 0.74), (-0.55, 0.74), (-0.65, 0.3)])
    d.hull([(0.92, 0), (0.62, 0.24), (0.05, 0.38), (-0.72, 0.36), (-0.82, 0)])
    d.cockpit(0.55, 0, 0.14, 0.09)
    d.turret(-0.15, 0.55, 0.15, 2); d.turret(-0.15, -0.55, 0.15, 2); d.turret(0.1, 0, 0.17, 2)


def ship_bomber(d):
    d.engine(-0.8, 0, 0.15); d.engine(-0.72, 0.52, 0.12); d.engine(-0.72, -0.52, 0.12)
    d.wings([(0.45, 0.14), (-0.55, 0.95), (-0.82, 0.95), (-0.68, 0.2)])
    for s in (1, -1):  # torpedo racks
        for k in (0.36, 0.62):
            d.container(-0.18 - k * 0.15, s * k, 0.42, 0.11)
            d.guns.append((0.03 - k * 0.15, s * k))
    d.hull([(0.98, 0), (0.45, 0.17), (-0.55, 0.25), (-0.78, 0)])
    d.cockpit(0.48, 0, 0.18, 0.08)


def ship_scout(d):
    d.engine(-0.62, 0, 0.11)
    d.wings([(-0.05, 0.1), (-0.35, 0.72), (-0.47, 0.72), (-0.35, 0.1)])
    d.hull([(0.78, 0), (0.35, 0.15), (-0.5, 0.15), (-0.62, 0)])
    d.dish(-0.05, 0, 0.26)
    d.cockpit(0.48, 0, 0.12, 0.06)
    d.guns.append((0.78, 0))


def ship_tanker(d):
    d.engine(-0.92, 0.14, 0.12); d.engine(-0.92, -0.14, 0.12)
    d.hull([(1.0, 0), (0.86, 0.11), (-0.84, 0.13), (-0.9, 0)], stripe=False)
    for x in (0.42, -0.02, -0.46):
        d.tank(x, 0, 0.29, team=(x == -0.02))
    d.cockpit(0.86, 0, 0.1, 0.07)


def ship_freighter(d):
    d.engine(-0.92, 0.18, 0.13); d.engine(-0.92, -0.18, 0.13)
    d.hull([(1.0, 0), (0.82, 0.14), (-0.84, 0.12), (-0.9, 0)], stripe=False)
    for x in (0.42, 0.0, -0.42):
        for y in (0.22, -0.22):
            d.container(x, y, 0.38, 0.36, team=(x == 0.0))
    d.cockpit(0.85, 0, 0.1, 0.08)


def ship_tug(d):
    d.engine(-0.6, 0, 0.3)
    for s in (1, -1):
        p = [(0.45, s * 0.28), (0.95, s * 0.42), (1.02, s * 0.22), (0.62, s * 0.12)]
        d.part(d.shape(p) if d.skin == "hive" else poly(p), d.metal, shift=0.05)
    d.hull([(0.62, 0), (0.5, 0.32), (-0.35, 0.38), (-0.55, 0)], stripe=False)
    d.cockpit(0.3, 0, 0.13, 0.1)


# advanced tier (rules 2): decoy drone, torpedo, mass-driver slug
def ship_decoy(d):
    # a small drone with a fake "big ship" emitter: thin hull, one big glowing reflector fan
    d.engine(-0.55, 0, 0.1)
    c = d.c
    if not d.icon:
        for r in (0.55, 0.75):
            c.new_path(); c.arc(-0.05, 0, r, -0.6, 0.6)
            c.set_source_rgb(*d.glass); c.set_line_width(3 * C.PX); c.set_dash([6 * C.PX, 5 * C.PX]); c.stroke()
            c.set_dash([])
    d.hull([(0.55, 0), (0.25, 0.12), (-0.45, 0.12), (-0.55, 0)], stripe=False)
    pf = lambda cc: (cc.move_to(-0.05, 0), cc.arc(-0.05, 0, 0.42, -0.75, 0.75), cc.close_path())
    d.part(pf, (lighter(d.glass, .5), d.glass), shift=0.05)
    d.cockpit(0.3, 0, 0.08, 0.05)


def ship_torpedo(d):
    # long thin warhead with fins: nose cone in team colour, one fat engine
    d.engine(-0.85, 0, 0.13)
    for s in (1, -1):
        d.part(poly([(-0.45, s * 0.1), (-0.75, s * 0.36), (-0.82, s * 0.36), (-0.7, s * 0.1)]), d.metal, shift=0.03)
    d.hull([(0.55, 0), (0.4, 0.12), (-0.78, 0.12), (-0.82, 0)], stripe=False)
    d.part(poly([(0.95, 0), (0.55, 0.13), (0.55, -0.13)]), d.team, shift=0.04)
    if not d.icon:
        for x in (0.1, -0.25):
            line(d.c, [(x, -0.11), (x, 0.11)], 2 * C.PX, darker(d.metal[1], 0.6))
    d.guns.append((0.95, 0))


def ship_slug(d):
    # an unpowered mass-driver slug: a dense tumbling ingot with motion streaks
    c = d.c
    if not d.icon:
        for y in (-0.18, 0.0, 0.18):
            line(c, [(-0.95, y), (-0.45, y)], 3 * C.PX, d.glass)
    pf = d.shape([(0.55, 0.0), (0.35, 0.3), (-0.35, 0.3), (-0.45, 0.0), (-0.35, -0.3), (0.35, -0.3)]) if d.skin == "hive" else poly([(0.55, 0.0), (0.35, 0.3), (-0.35, 0.3), (-0.45, 0.0), (-0.35, -0.3), (0.35, -0.3)])
    d.part(pf, d.metal, shift=0.1)
    d.part(rrect(0.0, 0.0, 0.3, 0.16, 0.03), d.team, shift=0.04)


SHIPS = {
    # name: (fn, relative size, role, beats, weak_to)
    "interceptor": (ship_interceptor, 0.55, "fast light fighter, kinetic cannons", "bomber", "gunship"),
    "gunship": (ship_gunship, 0.8, "turreted brawler, energy beams", "interceptor", "bomber"),
    "bomber": (ship_bomber, 0.9, "heavy torpedo carrier, anti-capital", "gunship", "interceptor"),
    "scout": (ship_scout, 0.45, "sensor picket, nearly invisible", None, None),
    "tanker": (ship_tanker, 1.0, "carries fuel between depots", None, None),
    "freighter": (ship_freighter, 1.0, "carries matter (cargo)", None, None),
    "tug": (ship_tug, 0.7, "asteroid drive / pusher", None, None),
    "decoy": (ship_decoy, 0.45, "decoy drone: looks like a warship on sensors", None, None),
    "torpedo": (ship_torpedo, 0.4, "guided torpedo", None, None),
    "slug": (ship_slug, 0.3, "mass-driver slug (unpowered)", None, None),
}

SHIP_PX = 128
SHIP_UNIT = SHIP_PX * 0.46


ONLY = None  # set by --only: generate just these ship classes / station kinds


def gen_ships():
    for cls, (fn, size, role, beats, weak) in SHIPS.items():
        if ONLY and cls not in ONLY:
            continue
        entry = {"size": size, "role": role, "beats": beats, "weak_to": weak, "px": SHIP_PX,
                 "facing": "+x", "skins": {}, "icon": f"sprites/icons/ship_{cls}.png"}
        for skin in SKINS:
            files = {}
            for team in TEAM_ORDER:
                s, c = canvas(SHIP_PX, SHIP_PX, SHIP_UNIT, 5.0)
                d = Drawer(c, skin, team, seed=hash((cls, skin)) & 0xffff)
                d.rnd = random.Random(f"{cls}{skin}")
                fn(d)
                files[team] = save(s, f"sprites/ships/{skin}/{cls}_{team}.png")
            to_px = lambda p: [round(p[0] * SHIP_UNIT, 1), round(p[1] * SHIP_UNIT, 1)]
            entry["skins"][skin] = files
            entry["engines_px"] = [to_px(p) for p in d.engines]
            entry["guns_px"] = [to_px(p) for p in d.guns]
        s, c = canvas(SHIP_PX, SHIP_PX, SHIP_UNIT, 5.0)
        d = Drawer(c, "vanguard", "neutral", icon=True); d.rnd = random.Random(cls)
        fn(d)
        save(s, f"sprites/icons/ship_{cls}.png")
        MANIFEST["ships"][cls] = entry


# ---------------------------------------------------------------- stations (192 px, top-down)
def st_shipyard(d):
    c = d.c
    for y in (0.6, -0.6):
        d.panel(-0.2, y * 1.38, 0.9, 0.18)
    for x in (-0.6, -0.2, 0.2, 0.6):
        inked_line(c, [(x, -0.55), (x, 0.55)], 0.05, d.metal[1], lw=2.5 * C.PX)
    # ship under construction (blueprint ghost)
    pts = mirror_half([(0.62, 0), (0.25, 0.2), (-0.5, 0.25), (-0.62, 0)])
    fill(c, poly(pts), d.glass, 0.25)
    line(c, pts + [pts[0]], 2.2 * C.PX, d.glass, dash=[6 * C.PX, 5 * C.PX])
    for y in (0.55, -0.55):
        d.container(0.0, y, 1.75, 0.16, team=True)
    for x, y in ((-0.6, 0.55), (0.6, -0.55)):  # cranes
        inked_line(c, [(x, y), (x + 0.25, y * 0.55)], 0.06, d.accent, lw=2.4 * C.PX)
        fill(c, circle(x + 0.25, y * 0.55, 0.05), d.accent); ink(c, circle(x + 0.25, y * 0.55, 0.05), 2 * C.PX)
    d.container(-0.95, 0, 0.28, 0.5, team=True)
    d.cockpit(-0.95, 0, 0.08, 0.1)


def st_fuel(d):
    c = d.c
    for i in range(6):
        a = i * math.pi / 3 + math.pi / 6
        inked_line(c, [(0, 0), (math.cos(a) * 0.62, math.sin(a) * 0.62)], 0.09, d.metal[1], lw=2.5 * C.PX)
    for i in range(6):
        a = i * math.pi / 3 + math.pi / 6
        d.tank(math.cos(a) * 0.65, math.sin(a) * 0.65, 0.25)
        if not d.icon:
            x, y = math.cos(a) * 0.65, math.sin(a) * 0.65
            drop = drop_path(x + 0.02, y + 0.02, 0.1)
            fill(c, drop, d.accent); ink(c, drop, 1.6 * C.PX)
    d.tank(0, 0, 0.3, team=True)
    fill(c, circle(0, 0, 0.1), d.accent if not d.icon else (.7, .7, .7)); ink(c, circle(0, 0, 0.1), 2 * C.PX)


def st_telescope(d):
    c = d.c
    for y in (0.75, -0.75):
        d.panel(-0.35, y, 0.8, 0.22)
    d.container(-0.55, 0, 0.75, 0.45, team=True)
    d.part(d.round_(0.25, 0, 0.62), d.metal, shift=0.1)
    pf = d.round_(0.25, 0, 0.5)
    d.part(pf, (lighter(d.glass, .6), d.glass), shift=0.14)
    if not d.icon:
        for k in (0.38, 0.24):
            ink(c, circle(0.25, 0, k), 1.6 * C.PX)
        fill(c, ellipse(0.08, -0.17, 0.12, 0.06, -0.6), WHITE, .8)
    for i in range(3):
        a = i * 2 * math.pi / 3 + 0.3
        line(c, [(0.25 + math.cos(a) * 0.5, math.sin(a) * 0.5), (0.25, 0)], 3 * C.PX, INK)
    d.part(d.round_(0.25, 0, 0.1), d.team, shift=0.03)


def st_scanner(d):
    c = d.c
    for a in (0.0, math.pi / 2, math.pi, 1.5 * math.pi):  # antenna spikes
        inked_line(c, [(0, 0), (math.cos(a) * 0.95, math.sin(a) * 0.95)], 0.04, d.metal[1], lw=2.4 * C.PX)
        fill(c, circle(math.cos(a) * 0.95, math.sin(a) * 0.95, 0.06), d.accent)
        ink(c, circle(math.cos(a) * 0.95, math.sin(a) * 0.95, 0.06), 2 * C.PX)
    # habitat ring
    c.new_path(); c.arc(0, 0, 0.62, 0, 2 * math.pi)
    c.set_source_rgb(*INK); c.set_line_width(0.2); c.stroke()
    c.new_path(); c.arc(0, 0, 0.62, 0, 2 * math.pi)
    c.set_source_rgb(*d.team[0]); c.set_line_width(0.2 - 2 * C.LW * 0.8); c.stroke()
    if not d.icon:
        c.new_path(); c.arc(0, 0, 0.62, 0.2, 2.6)
        c.set_source_rgb(*d.team[1]); c.set_line_width(0.2 - 2 * C.LW * 0.8); c.stroke()
        for i in range(12):
            a = i * math.pi / 6
            line(c, [(math.cos(a) * 0.55, math.sin(a) * 0.55), (math.cos(a) * 0.69, math.sin(a) * 0.69)], 1.5 * C.PX)
    d.tank(0, 0, 0.26, team=True)
    # radar dish (sector)
    c.save(); c.translate(0.05, -0.05); c.rotate(-0.7)
    pf = lambda cc: (cc.move_to(0, 0), cc.arc(0, 0, 0.42, -0.55, 0.55), cc.close_path())
    d.part(pf, (lighter(d.glass, .5), d.glass), shift=0.06)
    c.restore()


def st_defense(d):
    c = d.c
    pf = ngon(0, 0, 0.66, 6, 0) if d.skin != "hive" else blob(0, 0, 0.62, 0.08, 3)
    d.part(pf, d.team, shift=0.14)
    if not d.icon:
        ink(c, ngon(0, 0, 0.45, 6, 0) if d.skin != "hive" else circle(0, 0, 0.45), 2 * C.PX)
    for i in range(3):
        a = i * 2 * math.pi / 3 - math.pi / 2
        d.turret(math.cos(a) * 0.42, math.sin(a) * 0.42, 0.17, 2, a)
    fill(c, circle(0, 0, 0.13), d.accent if not d.icon else (.7, .7, .7)); ink(c, circle(0, 0, 0.13), 2.5 * C.PX)
    if not d.icon:
        fill(c, circle(-0.04, -0.04, 0.04), WHITE, .8)


def st_relay(d):
    c = d.c
    if not d.icon:
        for r in (0.7, 0.9):
            c.new_path(); c.arc(0, 0, r, -0.5, 0.5)
            c.set_source_rgb(*d.accent); c.set_line_width(4 * C.PX); c.set_dash([8 * C.PX, 6 * C.PX]); c.stroke()
            c.set_dash([])
    for i in range(3):
        a = i * 2 * math.pi / 3 + math.pi
        e = (math.cos(a) * 0.82, math.sin(a) * 0.82)
        inked_line(c, [(0, 0), e], 0.05, d.metal[1], lw=2.4 * C.PX)
        d.panel(math.cos(a) * 0.45 - math.sin(a) * 0.0, math.sin(a) * 0.45, 0.26, 0.26) if i == 0 else None
        d.part(d.round_(e[0], e[1], 0.1), d.metal, shift=0.03)
    d.tank(0, 0, 0.32, team=True)
    d.dish(0.0, 0.0, 0.17)


def drop_path(x, y, r):
    def f(c):
        c.move_to(x, y - r * 1.5)
        c.curve_to(x + r * 0.4, y - r * 0.6, x + r, y - r * 0.2, x + r, y + r * 0.25)
        c.arc(x, y + r * 0.25, r, 0, math.pi)
        c.curve_to(x - r, y - r * 0.2, x - r * 0.4, y - r * 0.6, x, y - r * 1.5)
        c.close_path()
    return f


# advanced tier stations
def st_gravimetric(d):
    # mass sensor: a heavy core ring with four pendulum masses on long booms
    c = d.c
    for i in range(4):
        a = i * math.pi / 2 + math.pi / 4
        e = (math.cos(a) * 0.82, math.sin(a) * 0.82)
        inked_line(c, [(0, 0), e], 0.04, d.metal[1], lw=2.4 * C.PX)
        d.part(d.round_(e[0], e[1], 0.14), d.metal, shift=0.04)
    if not d.icon:
        for r in (0.95, 1.05):
            ink(c, circle(0, 0, r), 1.4 * C.PX, d.glass)
    d.part(d.round_(0, 0, 0.42), d.team, shift=0.12)
    d.part(d.round_(0, 0, 0.22), (lighter(d.glass, .5), d.glass), shift=0.06)
    fill(c, circle(0, 0, 0.07), INK)


def st_interdiction(d):
    # interdiction grid / minefield node: a spiky mine-layer with a dashed net around it
    c = d.c
    if not d.icon:
        c.new_path(); c.arc(0, 0, 0.9, 0, 2 * math.pi)
        c.set_source_rgb(*d.accent); c.set_line_width(3 * C.PX); c.set_dash([5 * C.PX, 7 * C.PX]); c.stroke(); c.set_dash([])
        for i in range(6):  # little mines on the net
            a = i * math.pi / 3
            m = star_burst(math.cos(a) * 0.9, math.sin(a) * 0.9, 0.11, 0.06, 6)
            fill(c, m, d.team[1]); ink(c, m, 1.6 * C.PX)
    spikes = star_burst(0, 0, 0.62, 0.38, 8)
    d.part(spikes, d.team, shift=0.1)
    d.part(d.round_(0, 0, 0.26), d.metal, shift=0.06)
    fill(c, circle(0, 0, 0.09), d.accent if not d.icon else (.7, .7, .7)); ink(c, circle(0, 0, 0.09), 2 * C.PX)


def st_laser_battery(d):
    # laser battery: a capacitor bank (team) with one long emitter barrel and a lens
    c = d.c
    for y in (0.62, -0.62):
        d.panel(-0.45, y, 0.8, 0.2)
    for x in (-0.55, -0.2):
        for y in (0.22, -0.22):
            d.tank(x, y, 0.17, team=True)
    inked_line(c, [(0.0, 0.0), (0.95, 0.0)], 0.14, d.metal[1], lw=2.6 * C.PX)
    d.part(d.round_(0.05, 0, 0.3), d.metal, shift=0.08)
    pf = ellipse(0.95, 0, 0.08, 0.16)
    d.part(pf, (lighter(d.glass, .6), d.glass), shift=0.03)
    if not d.icon:
        line(c, [(1.03, 0), (1.09, 0)], 4 * C.PX, d.accent)
    d.guns.append((1.0, 0))


def st_mass_driver(d):
    # mass driver: a long rail of coil rings with a slug in the breech
    c = d.c
    for y in (0.14, -0.14):
        inked_line(c, [(-0.85, y), (0.95, y)], 0.07, d.metal[1], lw=2.4 * C.PX)
    for x in (-0.6, -0.3, 0.0, 0.3, 0.6, 0.9):
        d.part(rrect(x, 0, 0.1, 0.5, 0.03), d.team if x in (0.0, 0.6) else d.metal, shift=0.03)
    d.container(-0.85, 0, 0.36, 0.6, team=True)
    d.part(rrect(-0.45, 0, 0.18, 0.12, 0.02), d.metal, shift=0.02)
    if not d.icon:
        fill(c, circle(-0.45, 0, 0.03), d.accent)


def st_laser_launch(d):
    # laser launch: a big upward-facing mirror dish on a truss with a bright beam out of it
    c = d.c
    if not d.icon:
        c.new_path(); c.move_to(0.1, -0.08); c.line_to(1.05, -0.3); c.line_to(1.05, 0.3); c.line_to(0.1, 0.08); c.close_path()
        c.set_source_rgba(*d.glass, 0.35); c.fill()
    for y in (0.7, -0.7):
        d.panel(-0.55, y, 0.7, 0.22)
    inked_line(c, [(-0.55, -0.6), (-0.55, 0.6)], 0.06, d.metal[1], lw=2.4 * C.PX)
    pf = lambda cc: (cc.move_to(-0.2, 0), cc.arc(-0.2, 0, 0.55, -1.0, 1.0), cc.close_path())
    d.part(pf, (lighter(d.glass, .55), d.glass), shift=0.08)
    d.part(d.round_(-0.62, 0, 0.18), d.team, shift=0.05)
    fill(c, circle(0.12, 0, 0.06), d.accent if not d.icon else (.7, .7, .7)); ink(c, circle(0.12, 0, 0.06), 1.8 * C.PX)


def st_stargate(d):
    # stargate: a thick segmented ring (team) with anchor pylons and a swirling glass centre
    c = d.c
    if not d.icon:
        fill(c, circle(0, 0, 0.62), d.glass, 0.35)
        for k, r in enumerate((0.48, 0.33, 0.18)):
            c.new_path(); c.arc(0, 0, r, k * 0.9, k * 0.9 + 4.0)
            c.set_source_rgb(*lighter(d.glass, .4)); c.set_line_width(3 * C.PX); c.stroke()
    c.new_path(); c.arc(0, 0, 0.78, 0, 2 * math.pi)
    c.set_source_rgb(*INK); c.set_line_width(0.3); c.stroke()
    c.new_path(); c.arc(0, 0, 0.78, 0, 2 * math.pi)
    c.set_source_rgb(*d.team[0]); c.set_line_width(0.3 - 2 * C.LW * 0.8); c.stroke()
    if not d.icon:
        c.new_path(); c.arc(0, 0, 0.78, 0.3, 2.4)
        c.set_source_rgb(*d.team[1]); c.set_line_width(0.3 - 2 * C.LW * 0.8); c.stroke()
    for i in range(8):  # segments
        a = i * math.pi / 4
        line(c, [(math.cos(a) * 0.64, math.sin(a) * 0.64), (math.cos(a) * 0.92, math.sin(a) * 0.92)], 2 * C.PX)
    for i in range(3):  # pylons
        a = i * 2 * math.pi / 3 + math.pi / 6
        d.part(d.round_(math.cos(a) * 0.98, math.sin(a) * 0.98, 0.12), d.metal, shift=0.03)
        fill(c, circle(math.cos(a) * 0.98, math.sin(a) * 0.98, 0.04), d.accent if not d.icon else (.7, .7, .7))


STATIONS = {
    "shipyard": (st_shipyard, "builds ships in orbit"),
    "fuel_depot": (st_fuel, "stores and refuels; can sit on an asteroid"),
    "telescope": (st_telescope, "long-range optical intel on far bodies"),
    "scanner": (st_scanner, "scanning station: reveals small ships nearby"),
    "defense": (st_defense, "defense platform with three turrets"),
    "relay": (st_relay, "sensor relay: extends intel range"),
    "gravimetric": (st_gravimetric, "gravimetric sensor: detects mass, even cloaked ships"),
    "interdiction": (st_interdiction, "interdiction grid / minefield node"),
    "laser_battery": (st_laser_battery, "laser battery: long-range beam defense"),
    "mass_driver": (st_mass_driver, "mass driver: fires slugs"),
    "laser_launch": (st_laser_launch, "laser launch: boosts departing ships"),
    "stargate": (st_stargate, "stargate: instant hop to a linked gate"),
}
ST_PX = 192
ST_UNIT = ST_PX * 0.46


def gen_stations():
    for name, (fn, role) in STATIONS.items():
        if ONLY and name not in ONLY:
            continue
        entry = {"role": role, "px": ST_PX, "skins": {}, "icon": f"sprites/icons/station_{name}.png"}
        for skin in SKINS:
            files = {}
            for team in TEAM_ORDER:
                s, c = canvas(ST_PX, ST_PX, ST_UNIT, 5.5)
                d = Drawer(c, skin, team); d.rnd = random.Random(f"{name}{skin}")
                fn(d)
                files[team] = save(s, f"sprites/stations/{skin}/{name}_{team}.png")
            entry["skins"][skin] = files
        s, c = canvas(ST_PX, ST_PX, ST_UNIT, 6.5)
        d = Drawer(c, "vanguard", "neutral", icon=True); d.rnd = random.Random(name)
        fn(d)
        save(s, f"sprites/icons/station_{name}.png")
        MANIFEST["stations"][name] = entry
    # asteroid drive module (attached to pushed asteroids), one per team
    files = {}
    for team in TEAM_ORDER:
        s, c = canvas(64, 64, 64 * 0.46, 4)
        d = Drawer(c, "vanguard", team)
        d.engine(-0.45, 0, 0.42)
        d.part(rrect(0.25, 0, 0.9, 0.7, 0.1), d.team, shift=0.12)
        for y in (0.45, -0.45):
            inked_line(c, [(0.3, y * 0.7), (0.85, y * 1.5)], 0.1, d.metal[1], lw=2 * C.PX)
        files[team] = save(s, f"sprites/stations/asteroid_drive_{team}.png")
    MANIFEST["stations"]["asteroid_drive"] = {"role": "drive clamped to an asteroid; thrust +x", "px": 64,
                                              "skins": {"any": files}}


# ---------------------------------------------------------------- celestial bodies
SHADE = (0.07, 0.03, 0.2)


def body_sprite(px, R, painter, lit, seed, ring=None, lw=5.0):
    """Disk of radius R px at canvas centre. lit=True bakes cel shading (light from upper-left)."""
    s, c = canvas(px, px, 1.0, lw)
    c.translate(px / 2, px / 2)
    rnd = random.Random(seed)
    if ring:
        draw_ring(c, R, ring, back=True)
    disk = circle(0, 0, R)
    c.save(); disk(c); c.clip()
    painter(c, R, rnd)
    if lit:
        shade_crescent(c, R, (-1, -0.9))
    c.restore()
    if lit:
        fill(c, circle(-R * 0.42, -R * 0.42, R * 0.13), WHITE, 0.55)
    ink(c, disk)
    if ring:
        draw_ring(c, R, ring, back=False)
    return s


def shade_crescent(c, R, ldir, k=0.42, alpha=0.5):
    c.save()
    c.new_path(); c.rectangle(-1e4, -1e4, 2e4, 2e4)
    circle(ldir[0] * R * k, ldir[1] * R * k, R * 1.04)(c)
    c.set_fill_rule(cairo.FILL_RULE_EVEN_ODD); c.clip(); c.set_fill_rule(cairo.FILL_RULE_WINDING)
    c.set_source_rgba(*SHADE, alpha); c.paint()
    dots(c, SHADE, sp=max(4.6, R / 9), r=max(1.25, R / 34), alpha=0.55)
    c.restore()


def draw_ring(c, R, cols, back):
    rx, ry, w = R * 1.9, R * 0.55, R * 0.42
    c.save()
    c.new_path()
    if back:
        c.rectangle(-1e4, -1e4, 2e4, 1e4)  # y<0 half
    else:
        c.rectangle(-1e4, 0, 2e4, 1e4)
    c.clip()
    c.rotate(-0.25)
    outer, inner = ellipse(0, 0, rx, ry), ellipse(0, 0, rx - w, ry - w * 0.29)
    c.new_path(); outer(c); inner(c)
    c.set_fill_rule(cairo.FILL_RULE_EVEN_ODD)
    c.set_source_rgb(*hexc(cols[0])); c.fill(); c.set_fill_rule(cairo.FILL_RULE_WINDING)
    mid = ellipse(0, 0, rx - w * 0.45, ry - w * 0.13)
    ink(c, mid, 6, hexc(cols[1]))
    ink(c, outer); ink(c, inner)
    c.restore()


def craters(c, R, rnd, n, base, rmin=0.06, rmax=0.18):
    for _ in range(n):
        a = rnd.uniform(0, 6.283); d = math.sqrt(rnd.random()) * R * 0.85
        x, y, r = math.cos(a) * d, math.sin(a) * d, R * rnd.uniform(rmin, rmax)
        fill(c, circle(x, y, r), darker(base, 0.78))
        fill(c, circle(x - r * 0.18, y - r * 0.18, r * 0.75), darker(base, 0.9))
        ink(c, circle(x, y, r), max(2, R / 40))


def bands(c, R, rnd, cols, n=7, wav=0.06, tilt=-0.12):
    c.save(); c.rotate(tilt)
    c.set_source_rgb(*hexc(cols[0])); c.paint()
    edges = sorted(rnd.uniform(-1, 1) for _ in range(n))
    for i, e in enumerate(edges):
        col = hexc(cols[(i + 1) % len(cols)])
        ph, fr = rnd.uniform(0, 6), rnd.uniform(3, 6)
        c.new_path(); c.move_to(-R * 1.2, R * 1.2)
        for k in range(41):
            x = -R * 1.2 + k * R * 2.4 / 40
            c.line_to(x, e * R + math.sin(x / R * fr + ph) * R * wav)
        c.line_to(R * 1.2, R * 1.2); c.close_path()
        c.set_source_rgb(*col); c.fill_preserve()
        c.set_source_rgb(*INK); c.set_line_width(max(2, R / 60)); c.new_path()
        c.move_to(-R * 1.2, e * R + math.sin(-1.2 * fr + ph) * R * wav)
        for k in range(41):
            x = -R * 1.2 + k * R * 2.4 / 40
            c.line_to(x, e * R + math.sin(x / R * fr + ph) * R * wav)
        c.stroke()
    c.restore()


def continents(c, R, rnd, col, n, rmin=0.18, rmax=0.42, wob=0.35):
    for i in range(n):
        a = rnd.uniform(0, 6.283); d = rnd.uniform(0, R * 0.8)
        pf = blob(math.cos(a) * d, math.sin(a) * d, R * rnd.uniform(rmin, rmax), wob, rnd.randint(0, 1 << 30), 11)
        fill(c, pf, hexc(col[0]))
        c.save(); pf(c); c.clip(); c.translate(R * 0.04, R * 0.04)
        fill(c, pf, hexc(col[1])); c.restore()
        c.save(); pf(c); c.clip()
        fill(c, union(*[circle(math.cos(a) * d + rnd.uniform(-.2, .2) * R, math.sin(a) * d + rnd.uniform(-.2, .2) * R,
                               R * .08) for _ in range(2)]), hexc(col[1]))
        c.restore()
        ink(c, pf, max(2.5, R / 45))


def clouds(c, R, rnd, n, col="#ffffff", alpha=0.95):
    for _ in range(n):
        a = rnd.uniform(0, 6.283); d = rnd.uniform(0, R * 0.85)
        x, y = math.cos(a) * d, math.sin(a) * d
        w = R * rnd.uniform(0.3, 0.5)
        pf = union(*[circle(x + (k - 1) * w * 0.42, y - (0.12 * w if k == 1 else 0), w * (0.3 if k == 1 else 0.22))
                     for k in range(3)], rrect(x, y + w * 0.08, w * 1.1, w * 0.3, w * 0.15))
        ink(c, pf, max(3, R / 30) * 2)   # outline of the union only: thick stroke, then fill over it
        fill(c, pf, hexc(col), 1.0)


def cracks(c, R, rnd, n, col, w=None):
    for _ in range(n):
        x, y = rnd.uniform(-R, R), rnd.uniform(-R, R)
        pts = [(x, y)]
        a = rnd.uniform(0, 6.283)
        for k in range(rnd.randint(3, 6)):
            a += rnd.uniform(-0.8, 0.8)
            x += math.cos(a) * R * 0.18; y += math.sin(a) * R * 0.18
            pts.append((x, y))
        line(c, pts, (w or R / 22) + 3, INK)
        line(c, pts, w or R / 22, hexc(col))


def p_terran(c, R, rnd):
    c.set_source_rgb(*hexc("#3a86ff")); c.paint()
    dots(c, hexc("#4cc9f0"), sp=R / 7, r=R / 40, alpha=0.6)
    continents(c, R, rnd, ("#52b788", "#2d6a4f"), 5)
    clouds(c, R, rnd, 3)
    for y in (-R, R):  # polar caps
        fill(c, ellipse(0, y * 0.97, R * 0.5, R * 0.2), WHITE); ink(c, ellipse(0, y * 0.97, R * 0.5, R * 0.2), R / 50)


def p_ocean(c, R, rnd):
    c.set_source_rgb(*hexc("#1d4e89")); c.paint()
    for _ in range(6):
        x, y = rnd.uniform(-R, R), rnd.uniform(-R, R); r = R * rnd.uniform(0.2, 0.5)
        c.new_path(); c.arc(x, y, r, rnd.uniform(0, 6), rnd.uniform(0, 6) + 2.5)
        c.set_source_rgb(*hexc("#4cc9f0")); c.set_line_width(R / 22); c.stroke()
    continents(c, R, rnd, ("#e9c46a", "#bc8a3c"), 4, 0.05, 0.1, 0.25)
    clouds(c, R, rnd, 3)


def p_desert(c, R, rnd):
    bands(c, R, rnd, ["#e9c46a", "#f4d58d", "#d4a54a"], 6, 0.1, 0.3)
    craters(c, R, rnd, 7, hexc("#e9c46a"))


def p_ice(c, R, rnd):
    c.set_source_rgb(*hexc("#e0fbfc")); c.paint()
    continents(c, R, rnd, ("#bde0fe", "#98c1d9"), 4, .2, .45, .3)
    cracks(c, R, rnd, 9, "#7fb7d8", R / 40)


def p_lava(c, R, rnd):
    c.set_source_rgb(*hexc("#3d0c11")); c.paint()
    continents(c, R, rnd, ("#6a1e1e", "#4a1414"), 5, .2, .4, .3)
    cracks(c, R, rnd, 10, "#ff6b35", R / 18)
    cracks(c, R, rnd, 6, "#ffd23f", R / 30)


def p_barren(c, R, rnd):
    c.set_source_rgb(*hexc("#9a8c98")); c.paint()
    continents(c, R, rnd, ("#a89ca6", "#857682"), 3, .25, .5, .3)
    craters(c, R, rnd, 14, hexc("#9a8c98"))


def p_toxic(c, R, rnd):
    bands(c, R, rnd, ["#b5e48c", "#99d98c", "#d9ed92", "#76c893"], 8, 0.08)
    clouds(c, R, rnd, 2, "#fff3b0")


def p_gas_orange(c, R, rnd):
    bands(c, R, rnd, ["#f4a261", "#e76f51", "#e9c46a", "#f6bd60"], 9, 0.05)
    pf = ellipse(R * 0.25, R * 0.3, R * 0.26, R * 0.15, -0.12)
    cel(c, pf, hexc("#e63946"), hexc("#9d0208"), shift=R * 0.05, ht=False, lw=R / 40)
    ink(c, ellipse(R * 0.25, R * 0.3, R * 0.14, R * 0.07, -0.12), R / 60)


def p_gas_blue(c, R, rnd):
    bands(c, R, rnd, ["#a2d2ff", "#bde0fe", "#cdb4db", "#8ecae6"], 8, 0.04)


def p_jungle(c, R, rnd):
    c.set_source_rgb(*hexc("#2d6a4f")); c.paint()
    continents(c, R, rnd, ("#40916c", "#1b4332"), 6, .15, .35)
    for _ in range(4):  # rivers/lakes
        a = rnd.uniform(0, 6.3); d = rnd.uniform(0, R * .7)
        fill(c, blob(math.cos(a) * d, math.sin(a) * d, R * .08, .3, rnd.randint(0, 999)), hexc("#4cc9f0"))
    clouds(c, R, rnd, 3)


PLANETS = {
    "terran": (p_terran, None), "ocean": (p_ocean, None), "desert": (p_desert, None), "ice": (p_ice, None),
    "lava": (p_lava, None), "barren": (p_barren, None), "toxic": (p_toxic, None), "jungle": (p_jungle, None),
    "gas_orange": (p_gas_orange, None), "gas_blue": (p_gas_blue, ("#e5d9f2", "#a594c7")),
    "gas_ringed": (p_gas_orange, ("#ffe8b6", "#c99a3d")),
}


def m_grey(c, R, rnd):
    c.set_source_rgb(*hexc("#ced4da")); c.paint(); craters(c, R, rnd, 9, hexc("#ced4da"), .08, .22)


def m_icy(c, R, rnd):
    c.set_source_rgb(*hexc("#caf0f8")); c.paint(); cracks(c, R, rnd, 8, "#90e0ef", R / 25)


def m_red(c, R, rnd):
    c.set_source_rgb(*hexc("#c8743c")); c.paint(); craters(c, R, rnd, 7, hexc("#c8743c"), .08, .2)


def m_sulfur(c, R, rnd):
    c.set_source_rgb(*hexc("#ffd166")); c.paint()
    for _ in range(8):
        a = rnd.uniform(0, 6.3); d = rnd.uniform(0, R * .85)
        pf = blob(math.cos(a) * d, math.sin(a) * d, R * rnd.uniform(.07, .16), .3, rnd.randint(0, 999))
        fill(c, pf, hexc("#e76f51")); ink(c, pf, R / 30)


def m_carbon(c, R, rnd):
    c.set_source_rgb(*hexc("#5c5f66")); c.paint(); craters(c, R, rnd, 10, hexc("#5c5f66"), .06, .2)


MOONS = {"grey": m_grey, "icy": m_icy, "red": m_red, "sulfur": m_sulfur, "carbon": m_carbon}


def gen_bodies():
    B = MANIFEST["bodies"]
    B["shadow_overlay"] = {"file": "sprites/bodies/shadow_overlay.png", "px": 256, "radius_px": 112,
                           "note": "draw over *_flat bodies scaled so radius_px = body radius, rotated so +x points away from the star"}
    for name, (fn, ring) in PLANETS.items():
        px, R = (384, 112) if ring else (256, 112)
        e = {"px": px, "radius_px": R, "kind": "planet"}
        for lit in (False, True):
            s = body_sprite(px, R, fn, lit, f"p{name}", ring, 6)
            e["lit" if lit else "flat"] = save(s, f"sprites/bodies/planet_{name}_{'lit' if lit else 'flat'}.png")
        B[f"planet_{name}"] = e
    for name, fn in MOONS.items():
        e = {"px": 128, "radius_px": 54, "kind": "moon"}
        for lit in (False, True):
            s = body_sprite(128, 54, fn, lit, f"m{name}", None, 4.5)
            e["lit" if lit else "flat"] = save(s, f"sprites/bodies/moon_{name}_{'lit' if lit else 'flat'}.png")
        B[f"moon_{name}"] = e
    # shadow overlay
    s, c = canvas(256, 256, 1.0)
    c.translate(128, 128)
    c.save(); circle(0, 0, 112)(c); c.clip(); shade_crescent(c, 112, (-1, 0), 0.5, 0.55); c.restore()
    save(s, "sprites/bodies/shadow_overlay.png")
    # asteroids (pre-lit; irregular)
    ast = {"rock": ("#9c8b7a", "#6b5d50"), "rock2": ("#a39e93", "#6f6a60"), "rock3": ("#8a7c6d", "#5a4e42"),
           "rock4": ("#b08968", "#7f5539"), "metal": ("#b8c4d0", "#6c7a89"), "metal2": ("#c9b79c", "#8a7a63"),
           "ice": ("#d7f0ff", "#90c2e7"), "ice2": ("#e9f5f9", "#a3cfe0")}
    for i, (name, (a, b)) in enumerate(ast.items()):
        s, c = canvas(96, 96, 1.0, 4)
        c.translate(48, 48)
        rnd = random.Random(f"a{name}")
        pf = blob(0, 0, 32, 0.32 if "rock" in name else 0.22, rnd.randint(0, 99999), 8 if "metal" in name else 10)
        if "metal" in name:
            pts = [(math.cos(k / 7 * 6.283 + .3) * 34 * rnd.uniform(.7, 1.1), math.sin(k / 7 * 6.283 + .3) * 34 * rnd.uniform(.7, 1.1)) for k in range(7)]
            pf = poly(pts)
        cel(c, pf, hexc(a), hexc(b), shift=9)
        c.save(); pf(c); c.clip()
        if "metal" in name:
            for p in pts:
                line(c, [(0, 0), p], 1.6, darker(hexc(b), .8))
            fill(c, ngon(-6, -8, 7, 4, .4), WHITE, .7)
        elif "ice" in name:
            cracks(c, 30, rnd, 3, "#ffffff", 2.5)
        else:
            craters(c, 30, rnd, 3, hexc(a), .12, .22)
        c.restore(); ink(c, pf)
        B[f"asteroid_{name}"] = {"px": 96, "radius_px": 32, "kind": "asteroid",
                                 "lit": save(s, f"sprites/bodies/asteroid_{name}.png")}
    # stars
    stars = {"yellow": ("#ffd23f", "#ff9f1c", "#fff3b0"), "orange": ("#ffb347", "#f3722c", "#ffe0b2"),
             "red": ("#ff6b6b", "#c1121f", "#ffc2c2"), "blue": ("#bde0fe", "#4cc9f0", "#ffffff"),
             "white": ("#f8f9fa", "#ced4da", "#ffffff")}
    for name, (a, b, hi) in stars.items():
        s, c = canvas(512, 512, 1.0, 7)
        c.translate(256, 256)
        R = 140
        cor = star_burst(0, 0, R * 1.72, R * 1.22, 14, 0.1, 0.12, len(name) * 37)
        flat(c, cor, hexc(b))
        cor2 = star_burst(0, 0, R * 1.42, R * 1.12, 14, 0.32, 0.1, 7)
        flat(c, cor2, hexc(a))
        cel(c, circle(0, 0, R), hexc(a), hexc(b), shift=R * 0.28)
        c.save(); circle(0, 0, R)(c); c.clip()
        rnd = random.Random(name)
        for _ in range(4):
            aa = rnd.uniform(0, 6.3); d = rnd.uniform(.2, .7) * R
            fill(c, blob(math.cos(aa) * d, math.sin(aa) * d, R * rnd.uniform(.06, .12), .3, rnd.randint(0, 99)), hexc(b))
        c.restore()
        fill(c, circle(-R * 0.4, -R * 0.4, R * 0.18), hexc(hi), .9)
        ink(c, circle(0, 0, R))
        B[f"star_{name}"] = {"px": 512, "radius_px": R, "kind": "star",
                             "lit": save(s, f"sprites/bodies/star_{name}.png"),
                             "note": "corona extends to 1.72x radius; slowly rotate for life"}
    s, c = canvas(256, 256, 1.0)
    g = cairo.RadialGradient(128, 128, 0, 128, 128, 128)
    g.add_color_stop_rgba(0, 1, 1, 1, .9); g.add_color_stop_rgba(.35, 1, 1, 1, .35); g.add_color_stop_rgba(1, 1, 1, 1, 0)
    c.set_source(g); c.paint()
    B["star_glow"] = {"file": save(s, "sprites/bodies/star_glow.png"), "px": 256, "tint": True,
                      "note": "additive soft glow, tint with the star's colour"}


# ---------------------------------------------------------------- effects
def strip(frames, w, h, rel, painter, **kw):
    s = cairo.ImageSurface(cairo.FORMAT_ARGB32, w * frames, h)
    cc = cairo.Context(s)
    for i in range(frames):
        fs, fc = canvas(w, h, 1.0, kw.get("lw", 4.0))
        fc.translate(w / 2, h / 2)
        painter(fc, i / max(1, frames - 1), i, w, h)
        cc.set_source_surface(fs, i * w, 0); cc.paint()
    save(s, rel)
    return {"file": rel, "frames": frames, "frame_w": w, "frame_h": h}


def ease_out(t):
    return 1 - (1 - t) ** 3


def explosion(R, seed, big=False):
    def paint(c, t, i, w, h):
        rnd = random.Random(seed + i)
        e = ease_out(min(1, t * 1.6))
        fire_a = max(0, 1 - max(0, t - 0.35) / 0.45)
        # smoke puffs
        if t > 0.25:
            rr = random.Random(seed)
            for k in range(9 if big else 6):
                a = rr.uniform(0, 6.283); d = R * (0.35 + 0.55 * ease_out(t)) * rr.uniform(.6, 1)
                r = R * (0.18 + 0.2 * t) * rr.uniform(.8, 1.2)
                al = max(0, 1 - (t - 0.25) / 0.75)
                c.push_group()
                cel(c, circle(math.cos(a) * d, math.sin(a) * d, r), hexc("#c9c6d8"), hexc("#8a86a3"), shift=r * .3)
                c.pop_group_to_source(); c.paint_with_alpha(al)
        if fire_a > 0:
            c.push_group()
            r1 = R * (0.35 + 0.65 * e)
            flat(c, star_burst(0, 0, r1, r1 * 0.62, 11 if big else 9, i * 0.4, 0.18, seed + i), hexc("#ff9f1c"))
            flat(c, star_burst(0, 0, r1 * .72, r1 * .45, 9, i * 0.4 + .3, 0.2, seed + 2 * i), hexc("#ffd23f"))
            core = max(0, 1 - t * 2.2)
            if core > 0:
                flat(c, circle(0, 0, r1 * 0.35 * core + 1), WHITE, lw=2.5)
            c.pop_group_to_source(); c.paint_with_alpha(fire_a)
        # debris
        rr = random.Random(seed * 3)
        for k in range(10 if big else 6):
            a = rr.uniform(0, 6.283); d = R * (0.2 + 0.85 * ease_out(t))
            if t > 0.1 and t < 0.95:
                x, y = math.cos(a) * d, math.sin(a) * d
                line(c, [(x, y), (x - math.cos(a) * R * .12, y - math.sin(a) * R * .12)], 3.5, INK)
                line(c, [(x, y), (x - math.cos(a) * R * .12, y - math.sin(a) * R * .12)], 1.6, hexc("#ffd23f"))
    return paint


def gen_fx():
    F = MANIFEST["fx"]
    F["explosion_small"] = strip(8, 96, 96, "sprites/fx/explosion_small.png", explosion(40, 11), lw=4) | {"fps": 16}
    F["explosion_large"] = strip(10, 192, 192, "sprites/fx/explosion_large.png", explosion(84, 23, True), lw=6) | {"fps": 14}

    def asteroid_impact(c, t, i, w, h):
        R = 120
        e = ease_out(t)
        al = max(0, 1 - t)
        c.push_group()
        c.new_path(); c.arc(0, 0, R * (0.2 + 0.8 * e), 0, 6.283)
        c.set_source_rgb(*INK); c.set_line_width(14 * (1 - t) + 4); c.stroke()
        c.new_path(); c.arc(0, 0, R * (0.2 + 0.8 * e), 0, 6.283)
        c.set_source_rgb(*WHITE); c.set_line_width(8 * (1 - t) + 1); c.stroke()
        c.pop_group_to_source(); c.paint_with_alpha(al)
        explosion(70, 41, True)(c, min(1, t * 1.15), i, w, h)
        rr = random.Random(5)
        for k in range(14):  # ejecta rocks
            a = rr.uniform(0, 6.283); d = R * 0.95 * ease_out(t) * rr.uniform(.4, 1)
            if 0.05 < t < 0.97:
                r = rr.uniform(4, 9) * (1 - t * .5)
                cel(c, blob(math.cos(a) * d, math.sin(a) * d, r, .3, k), hexc("#9c8b7a"), hexc("#6b5d50"), shift=r * .4,
                    ht=False, lw=2.5)
    F["asteroid_impact"] = strip(12, 256, 256, "sprites/fx/asteroid_impact.png", asteroid_impact, lw=5) | {"fps": 12}

    def kin_impact(c, t, i, w, h):
        rnd = random.Random(3)
        al = 1 - t
        c.push_group()
        r = 6 + 18 * ease_out(t)
        if t < 0.6:
            flat(c, star_burst(0, 0, r, r * .5, 6, i, .2, i), hexc("#ffd23f"), lw=3)
        for k in range(7):
            a = rnd.uniform(0, 6.283); d0, d1 = r * 0.8, r * 0.8 + 10 * (1 - t) + 4
            pts = [(math.cos(a) * d0, math.sin(a) * d0), (math.cos(a) * d1, math.sin(a) * d1)]
            line(c, pts, 4, INK); line(c, pts, 2, WHITE)
        c.pop_group_to_source(); c.paint_with_alpha(al)
    F["impact_kinetic"] = strip(6, 64, 64, "sprites/fx/impact_kinetic.png", kin_impact) | {"fps": 20}

    def en_impact(c, t, i, w, h):  # white+ink → tint with team colour
        al = 1 - t * .9
        c.push_group()
        r = 6 + 20 * ease_out(t)
        c.new_path(); c.arc(0, 0, r, 0, 6.283)
        c.set_source_rgb(*INK); c.set_line_width(7); c.stroke()
        c.new_path(); c.arc(0, 0, r, 0, 6.283)
        c.set_source_rgb(*WHITE); c.set_line_width(3.5); c.stroke()
        rnd = random.Random(i)
        for k in range(4):
            a = rnd.uniform(0, 6.283)
            pts = [(math.cos(a) * r * .3, math.sin(a) * r * .3)]
            for j in range(3):
                pts.append((math.cos(a) * r * (0.5 + j * .3) + rnd.uniform(-4, 4), math.sin(a) * r * (0.5 + j * .3) + rnd.uniform(-4, 4)))
            line(c, pts, 4.5, INK); line(c, pts, 2, WHITE)
        if t < .5:
            flat(c, circle(0, 0, 7 * (1 - 2 * t) + 1), WHITE, lw=3)
        c.pop_group_to_source(); c.paint_with_alpha(al)
    F["impact_energy"] = strip(6, 64, 64, "sprites/fx/impact_energy.png", en_impact) | {"fps": 20, "tint": True}

    def shield(c, t, i, w, h):
        al = 1 - t
        c.push_group()
        R = 40
        c.save(); c.new_path(); c.arc(0, 0, R, -1.0 - t * .4, 1.0 + t * .4); c.arc_negative(0, 0, R - 12, 1.0 + t * .4, -1.0 - t * .4)
        c.close_path(); c.clip()
        c.set_source_rgba(1, 1, 1, 0.45); c.paint()
        for x in range(-6, 7):
            for y in range(-6, 7):
                hx, hy = x * 9 + (y % 2) * 4.5, y * 7.8
                ink(c, ngon(hx, hy, 5, 6, math.pi / 6), 2.2, WHITE)
        c.restore()
        c.new_path(); c.arc(0, 0, R, -1.0 - t * .4, 1.0 + t * .4)
        c.set_source_rgb(*INK); c.set_line_width(6); c.stroke()
        c.new_path(); c.arc(0, 0, R, -1.0 - t * .4, 1.0 + t * .4)
        c.set_source_rgb(*WHITE); c.set_line_width(3); c.stroke()
        c.pop_group_to_source(); c.paint_with_alpha(al)
    F["shield_hit"] = strip(6, 96, 96, "sprites/fx/shield_hit.png", shield) | {
        "fps": 18, "tint": True, "note": "arc faces +x: rotate toward the incoming shot"}

    def muzzle(c, t, i, w, h):
        r = [10, 20, 16, 8][i]
        flat(c, star_burst(r * 0.6 - 16, 0, r, r * 0.45, 5, 0, .25, i), hexc("#ffd23f") if i < 3 else WHITE, lw=3)
        if i < 2:
            flat(c, circle(r * .6 - 16, 0, r * .3), WHITE, lw=2)
    F["muzzle_flash"] = strip(4, 48, 48, "sprites/fx/muzzle_flash.png", muzzle) | {
        "fps": 24, "anchor": [0.17, 0.5], "note": "flash points +x from the gun"}

    def flame(c, t, i, w, h):
        L = [52, 44, 56, 48][i]; W = [11, 10, 12, 10][i]
        def f(L, W):
            def p(cc):
                cc.move_to(w / 2 - 2, -W); cc.curve_to(w / 2 - L * .4, -W * 1.1, w / 2 - L * .8, -W * .4, w / 2 - L, 0)
                cc.curve_to(w / 2 - L * .8, W * .4, w / 2 - L * .4, W * 1.1, w / 2 - 2, W); cc.close_path()
            return p
        flat(c, f(L, W), hexc("#ff9f1c"), lw=3)
        flat(c, f(L * .7, W * .62), hexc("#ffd23f"), outline=False)
        flat(c, f(L * .38, W * .3), WHITE, outline=False)
    F["engine_flame"] = strip(4, 64, 32, "sprites/fx/engine_flame.png", flame) | {
        "fps": 14, "anchor": [1.0, 0.5], "note": "nozzle at right edge; flame points -x. Draw with the ship's rotation at each engines_px point"}

    def smoke(c, t, i, w, h):
        r = 10 + 14 * t
        c.push_group()
        cel(c, union(circle(-r * .4, 0, r * .7), circle(r * .4, -r * .2, r * .6), circle(0, r * .3, r * .55)),
            hexc("#c9c6d8"), hexc("#8a86a3"), shift=r * .3, lw=3)
        c.pop_group_to_source(); c.paint_with_alpha(1 - t * .85)
    F["smoke_puff"] = strip(5, 64, 64, "sprites/fx/smoke_puff.png", smoke) | {"fps": 10}

    def scan(c, t, i, w, h):
        r = 8 + 52 * ease_out(t)
        c.push_group()
        c.new_path(); c.arc(0, 0, r, 0, 6.283)
        c.set_source_rgb(*INK); c.set_line_width(7); c.stroke()
        c.new_path(); c.arc(0, 0, r, 0, 6.283)
        c.set_source_rgb(*WHITE); c.set_line_width(3.5); c.set_dash([10, 6]); c.stroke(); c.set_dash([])
        c.pop_group_to_source(); c.paint_with_alpha(1 - t)
    F["scan_ping"] = strip(8, 128, 128, "sprites/fx/scan_ping.png", scan) | {"fps": 10, "tint": True}

    # single sprites
    s, c = canvas(24, 12, 1.0, 2.5); c.translate(12, 6)
    flat(c, union(rrect(-2, 0, 14, 7, 3.5)), hexc("#ffd23f"), lw=2.5)
    fill(c, rrect(-4, -1, 6, 2, 1), WHITE)
    F["kinetic_shell"] = {"file": save(s, "sprites/fx/kinetic_shell.png"), "facing": "+x"}
    s, c = canvas(64, 12, 1.0, 2.5); c.translate(32, 6)
    g = cairo.LinearGradient(-30, 0, 28, 0); g.add_color_stop_rgba(0, 1, .82, .25, 0); g.add_color_stop_rgba(1, 1, .82, .25, 1)
    c.new_path(); c.move_to(-30, 0); c.line_to(22, -3.5); c.arc(22, 0, 3.5, -math.pi / 2, math.pi / 2); c.close_path()
    c.set_source(g); c.fill()
    flat(c, circle(24, 0, 4), WHITE, lw=2.5)
    F["kinetic_tracer"] = {"file": save(s, "sprites/fx/kinetic_tracer.png"), "facing": "+x",
                           "note": "head at right; stretch along x with speed"}
    s, c = canvas(48, 24, 1.0, 3); c.translate(24, 12)
    pf = lambda cc: (cc.move_to(20, 0), cc.curve_to(14, -9, -6, -7, -20, 0), cc.curve_to(-6, 7, 14, 9, 20, 0), cc.close_path())
    flat(c, pf, WHITE, lw=3)
    fill(c, ellipse(8, 0, 7, 3), WHITE)
    F["energy_bolt"] = {"file": save(s, "sprites/fx/energy_bolt.png"), "facing": "+x", "tint": True}
    # beams: body (ink edges, white inside → tint), core (untinted white)
    s, c = canvas(64, 32, 1.0)
    c.rectangle(0, 8, 64, 16); c.set_source_rgb(*WHITE); c.fill()
    for y in (8, 24):
        line(c, [(0, y), (64, y)], 4, INK)
    F["energy_beam_body"] = {"file": save(s, "sprites/fx/energy_beam_body.png"), "tint": True, "tile_x": True,
                             "note": "stretch/tile along x from gun to target; tint with team colour, then draw beam_core on top"}
    s, c = canvas(64, 32, 1.0)
    g = cairo.LinearGradient(0, 8, 0, 24)
    for st, a in ((0, 0), (.35, .9), (.5, 1), (.65, .9), (1, 0)):
        g.add_color_stop_rgba(st, 1, 1, 1, a)
    c.rectangle(0, 10, 64, 12); c.set_source(g); c.fill()
    F["energy_beam_core"] = {"file": save(s, "sprites/fx/energy_beam_core.png"), "tile_x": True}
    # debris chunks
    for k in range(4):
        s, c = canvas(24, 24, 1.0, 2.5); c.translate(12, 12)
        rnd = random.Random(k)
        pf = poly([(math.cos(a) * rnd.uniform(4, 9), math.sin(a) * rnd.uniform(4, 9)) for a in [j * 6.283 / 5 for j in range(5)]])
        cel(c, pf, hexc("#adb5bd"), hexc("#6c757d"), shift=2.5, ht=False, lw=2.5)
        F[f"debris_{k}"] = {"file": save(s, f"sprites/fx/debris_{k}.png")}
    # comic words
    words = {"boom": ("BOOM!", "#ff9f1c"), "kaboom": ("KA-BOOM!", "#e63946"), "pow": ("POW!", "#ffd23f"),
             "zap": ("ZAP!", "#4cc9f0"), "bzzt": ("BZZT!", "#c7f464"), "krak": ("KRAK!", "#ffd23f"),
             "wham": ("WHAM!", "#ff4fd8"), "ping": ("PING!", "#ffffff")}
    for k, (wd, col) in words.items():
        W = 256
        s, c = canvas(W, 128, 1.0, 5); c.translate(W / 2, 64)
        ro, ri = (120, 86) if len(wd) > 6 else (104, 72)
        rr = random.Random(k)
        pts = [(math.cos(0.1 + i * math.pi / 13) * (ro if i % 2 == 0 else ri) * (1 + rr.uniform(-.1, .1)),
                math.sin(0.1 + i * math.pi / 13) * (ro if i % 2 == 0 else ri) * 0.5 * (1 + rr.uniform(-.1, .1)))
               for i in range(26)]
        flat(c, poly(pts), WHITE, lw=5)
        text(c, wd, 0, 2, 54 if len(wd) <= 6 else 44, hexc(col), 9, rot=-0.08)
        F[f"word_{k}"] = {"file": save(s, f"sprites/fx/word_{k}.png"), "note": "comic onomatopoeia sticker; pop-in scale + fade"}


# ---------------------------------------------------------------- icons (96 px)
def icon(name, painter, desc):
    s, c = canvas(96, 96, 1.0, 5.5)
    c.translate(48, 48)
    painter(c)
    MANIFEST["icons"][name] = {"file": save(s, f"sprites/icons/{name}.png"), "px": 96, "desc": desc}


def gen_icons():
    Y, O, Bl, G, R, P = hexc("#ffd23f"), hexc("#ff9f1c"), hexc("#4cc9f0"), hexc("#80ed99"), hexc("#e63946"), hexc("#ff4fd8")
    SL = (hexc("#b8c4d0"), hexc("#6c7a89"))

    def matter(c):  # chunky ore crystal cluster on a cube
        cube = [(0, -6), (26, 6), (0, 18), (-26, 6)]
        cel(c, poly([(-26, 6), (0, 18), (0, 40), (-26, 28)]), *SL, ht=False)
        cel(c, poly([(26, 6), (0, 18), (0, 40), (26, 28)]), SL[1], darker(SL[1]), ht=True)
        cel(c, poly(cube), lighter(SL[0], .4), SL[0], ht=False)
        for x, h, w in ((-10, 40, 9), (6, 48, 10), (18, 30, 7)):
            p = poly([(x - w, 6), (x - w * .7, 6 - h * .7), (x, 6 - h), (x + w * .7, 6 - h * .7), (x + w, 6)])
            cel(c, p, lighter(Bl, .3), hexc("#3a86ff"), shift=4)
    icon("matter", matter, "Matter resource")

    def energy(c):
        p = poly([(8, -42), (-24, 6), (-2, 6), (-10, 42), (24, -8), (2, -8), (14, -42)])
        cel(c, p, Y, O, shift=6, hl=(2, -26), hl_r=4)
    icon("energy", energy, "Energy resource")

    def fuel(c):
        cel(c, drop_path(0, 6, 28), O, hexc("#e85d04"), shift=8, hl=(-10, -2), hl_r=5)
        text(c, "F", 0, 14, 30, WHITE, 6)
    icon("fuel", fuel, "Fuel resource")

    def research(c):  # flask
        p = lambda cc: (cc.move_to(-8, -40), cc.line_to(8, -40), cc.line_to(8, -14), cc.line_to(32, 32),
                        cc.curve_to(34, 40, 30, 42, 24, 42), cc.line_to(-24, 42), cc.curve_to(-30, 42, -34, 40, -32, 32),
                        cc.line_to(-8, -14), cc.close_path())
        cel(c, p, hexc("#e9ecef"), hexc("#adb5bd"), ht=False, outline=False)
        c.save(); p(c); c.clip()
        cel(c, rrect(0, 30, 80, 36, 0), G, hexc("#38a169"), shift=5, outline=False)
        c.restore()
        ink(c, p)
        for x, y, r in ((-6, 22, 5), (8, 30, 4), (2, 10, 3)):
            flat(c, circle(x, y, r), WHITE, lw=2.5)
        line(c, [(-13, -40), (13, -40)], 7)
    icon("research", research, "Research")

    def upgrades(c):
        for k, y in enumerate((14, -8)):
            p = poly([(-30, y + 18), (0, y - 12), (30, y + 18), (30, y + 30), (0, y), (-30, y + 30)])
            cel(c, p, Bl if k else lighter(Bl, .2), hexc("#3a86ff"), shift=4)
        flat(c, star_burst(0, -34, 11, 5, 5, -math.pi / 2), Y, lw=3.5)
    icon("upgrades", upgrades, "Upgrades")

    def power(c):  # sword
        cel(c, poly([(-6, -44), (0, -50), (6, -44), (6, 14), (-6, 14)]), hexc("#e9ecef"), hexc("#adb5bd"), shift=4, ht=False)
        cel(c, rrect(0, 18, 40, 10, 4), Y, O, shift=3, ht=False)
        cel(c, rrect(0, 32, 10, 20, 3), hexc("#9d0208"), darker(hexc("#9d0208")), shift=3, ht=False)
        flat(c, circle(0, 44, 6), Y, lw=3.5)
    icon("power", power, "Military power in orbit")

    def clock(c):
        cel(c, circle(0, 0, 38), WHITE, hexc("#ced4da"), shift=8)
        for k in range(12):
            a = k * math.pi / 6
            line(c, [(math.cos(a) * 28, math.sin(a) * 28), (math.cos(a) * 33, math.sin(a) * 33)], 3)
        line(c, [(0, 0), (0, -24)], 5); line(c, [(0, 0), (16, 8)], 5)
        flat(c, circle(0, 0, 4), R, lw=2)
    icon("time", clock, "Time / ETA")

    def telescope(c):
        c.save(); c.rotate(-0.6)
        cel(c, rrect(4, 0, 64, 20, 4), Bl, hexc("#3a86ff"), shift=4)
        cel(c, rrect(-34, 0, 14, 14, 3), *SL, shift=3, ht=False)
        flat(c, ellipse(36, 0, 5, 11), hexc("#caf0f8"), lw=4)
        c.restore()
        line(c, [(-4, 6), (-22, 40)], 6); line(c, [(-4, 6), (14, 40)], 6)
    icon("telescope", telescope, "Telescope / intel")

    def chat(c):
        p = lambda cc: (rrect(0, -6, 76, 52, 18)(cc))
        tail = poly([(-14, 16), (-26, 38), (2, 18)])
        flat(c, union(p, tail), WHITE)
        fill(c, rrect(0, -6, 70, 46, 15), WHITE); fill(c, poly([(-12, 14), (-22, 32), (0, 15)]), WHITE)
        for x in (-18, 0, 18):
            fill(c, circle(x, -6, 5), INK)
    icon("chat", chat, "Chat")

    def alert(c):
        cel(c, star_burst(0, 0, 44, 32, 10, 0), Y, O, shift=6, ht=False)
        text(c, "!", 0, 2, 52, R, 9)
    icon("alert", alert, "Alert")

    def warning(c):
        cel(c, poly([(0, -40), (42, 36), (-42, 36)]), Y, O, shift=5)
        text(c, "!", 0, 10, 40, INK, 0.1)
    icon("warning", warning, "Warning")

    def gear(c):
        pts = []
        for k in range(32):
            a = k * 2 * math.pi / 32
            r = 40 if (k // 2) % 2 == 0 else 30
            pts.append((math.cos(a) * r, math.sin(a) * r))
        g = lambda cc: (poly(pts)(cc), circle(0, 0, 12)(cc))
        c.set_fill_rule(cairo.FILL_RULE_EVEN_ODD)
        cel(c, g, *SL, shift=6)
        c.set_fill_rule(cairo.FILL_RULE_WINDING)
    icon("settings", gear, "Settings")

    def build(c):  # hammer + wrench crossed
        c.save(); c.rotate(math.pi / 4)
        cel(c, rrect(0, 10, 10, 64, 4), hexc("#bc6c25"), hexc("#7f4f24"), shift=3, ht=False)
        cel(c, rrect(0, -26, 44, 18, 4), *SL, shift=4)
        c.restore()
    icon("build", build, "Build")

    def fleet(c):
        for x, y, s in ((-18, -18, .8), (-18, 18, .8), (16, 0, 1.0)):
            p = poly([(x + 22 * s, y), (x - 14 * s, y - 14 * s), (x - 6 * s, y), (x - 14 * s, y + 14 * s)])
            cel(c, p, Bl, hexc("#3a86ff"), shift=3, ht=False)
    icon("fleet", fleet, "Fleet / ships")

    def planet(c):
        R0 = 30
        draw_ring(c, R0, ("#ffe8b6", "#c99a3d"), True)
        cel(c, circle(0, 0, R0), hexc("#f4a261"), hexc("#e76f51"), shift=8)
        draw_ring(c, R0, ("#ffe8b6", "#c99a3d"), False)
    icon("planet", planet, "Planet")

    def trophy(c):
        cup = lambda cc: (cc.move_to(-26, -36), cc.line_to(26, -36), cc.curve_to(26, 0, 14, 10, 0, 12),
                          cc.curve_to(-14, 10, -26, 0, -26, -36), cc.close_path())
        for s in (1, -1):
            c.new_path(); c.arc(s * 26, -22, 12, -math.pi / 2, math.pi / 2) if s > 0 else c.arc_negative(-26, -22, 12, -math.pi / 2, math.pi / 2)
            c.set_source_rgb(*INK); c.set_line_width(10); c.stroke()
            c.new_path(); c.arc(s * 26, -22, 12, -math.pi / 2, math.pi / 2) if s > 0 else c.arc_negative(-26, -22, 12, -math.pi / 2, math.pi / 2)
            c.set_source_rgb(*Y); c.set_line_width(4); c.stroke()
        cel(c, rrect(0, 24, 10, 26, 2), Y, O, shift=3, ht=False)
        cel(c, rrect(0, 38, 40, 10, 3), O, darker(O), shift=3, ht=False)
        cel(c, cup, Y, O, shift=7, hl=(-12, -26), hl_r=4)
    icon("trophy", trophy, "Victory / territory")

    def lock(c):
        c.new_path(); c.arc(0, -8, 18, math.pi, 0); c.set_source_rgb(*INK); c.set_line_width(16); c.stroke()
        c.new_path(); c.arc(0, -8, 18, math.pi, 0); c.set_source_rgb(*SL[0]); c.set_line_width(7); c.stroke()
        cel(c, rrect(0, 16, 56, 44, 8), Y, O, shift=6)
        flat(c, circle(0, 12, 6), INK, outline=False); line(c, [(0, 14), (0, 26)], 5)
    icon("lock", lock, "Locked tech")

    def check(c):
        cel(c, circle(0, 0, 40), G, hexc("#38a169"), shift=7)
        line(c, [(-20, 2), (-6, 18), (22, -14)], 16); line(c, [(-20, 2), (-6, 18), (22, -14)], 8, WHITE)
    icon("check", check, "Confirm / done")

    def close(c):
        cel(c, circle(0, 0, 40), R, hexc("#9d0208"), shift=7)
        for p in ([(-16, -16), (16, 16)], [(16, -16), (-16, 16)]):
            line(c, p, 16); line(c, p, 8, WHITE)
    icon("close", close, "Cancel / close")

    def plus(c):
        cel(c, circle(0, 0, 40), Bl, hexc("#3a86ff"), shift=7)
        for p in ([(-20, 0), (20, 0)], [(0, -20), (0, 20)]):
            line(c, p, 16); line(c, p, 8, WHITE)
    icon("plus", plus, "Add")

    def minus(c):
        cel(c, circle(0, 0, 40), Bl, hexc("#3a86ff"), shift=7)
        line(c, [(-20, 0), (20, 0)], 16); line(c, [(-20, 0), (20, 0)], 8, WHITE)
    icon("minus", minus, "Remove")

    def play(c):
        cel(c, poly([(-24, -34), (34, 0), (-24, 34)]), G, hexc("#38a169"), shift=6)
    icon("play", play, "Play / resume time")

    def pause(c):
        for x in (-14, 14):
            cel(c, rrect(x, 0, 18, 64, 4), Y, O, shift=4, ht=False)
    icon("pause", pause, "Pause")

    def ff(c):
        for x in (-20, 12):
            cel(c, poly([(x - 18, -28), (x + 22, 0), (x - 18, 28)]), Y, O, shift=4, ht=False)
    icon("fast_forward", ff, "Scrub forward / time warp")

    def eye(c):
        p = lambda cc: (cc.move_to(-42, 0), cc.curve_to(-20, -32, 20, -32, 42, 0), cc.curve_to(20, 32, -20, 32, -42, 0), cc.close_path())
        flat(c, p, WHITE)
        c.save(); p(c); c.clip()
        cel(c, circle(0, 0, 18), Bl, hexc("#3a86ff"), shift=5, ht=False)
        fill(c, circle(0, 0, 8), INK); fill(c, circle(-5, -5, 4), WHITE)
        c.restore(); ink(c, p)
    icon("eye", eye, "Visible / scanned")

    def eye_off(c):
        eye(c)
        line(c, [(-34, 34), (34, -34)], 14); line(c, [(-34, 34), (34, -34)], 6, R)
    icon("eye_off", eye_off, "Unknown / hidden")

    def players(c):
        for x, col, s in ((-16, R, .85), (16, Bl, .85), (0, Y, 1.0)):
            cel(c, lambda cc, x=x, s=s: (cc.new_sub_path(), cc.arc(x, 34, 22 * s, math.pi, 0), cc.close_path()), col,
                darker(col), shift=4, ht=False)
            cel(c, circle(x, 2 - 6 * (s == 1), 13 * s), col, darker(col), shift=3, ht=False, hl=(x - 4, -6), hl_r=3)
    icon("players", players, "Players")

    def station(c):
        c.new_path(); c.arc(0, 0, 30, 0, 6.283); c.set_source_rgb(*INK); c.set_line_width(16); c.stroke()
        c.new_path(); c.arc(0, 0, 30, 0, 6.283); c.set_source_rgb(*SL[0]); c.set_line_width(7); c.stroke()
        for a in (0, math.pi / 2, math.pi, 1.5 * math.pi):
            inked_line(c, [(0, 0), (math.cos(a) * 30, math.sin(a) * 30)], 5, SL[1], lw=3)
        cel(c, circle(0, 0, 14), Bl, hexc("#3a86ff"), shift=4, ht=False)
    icon("station", station, "Space station")

    def asteroid(c):
        cel(c, blob(0, 0, 34, .3, 9, 9), hexc("#9c8b7a"), hexc("#6b5d50"), shift=8)
        craters(c, 30, random.Random(4), 3, hexc("#9c8b7a"), .15, .25)
    icon("asteroid", asteroid, "Asteroid")

    def target(c):
        for r, col in ((36, R), (20, WHITE)):
            c.new_path(); c.arc(0, 0, r, 0, 6.283); c.set_source_rgb(*INK); c.set_line_width(12); c.stroke()
            c.new_path(); c.arc(0, 0, r, 0, 6.283); c.set_source_rgb(*col); c.set_line_width(5); c.stroke()
        for a in range(4):
            aa = a * math.pi / 2
            p = [(math.cos(aa) * 26, math.sin(aa) * 26), (math.cos(aa) * 46, math.sin(aa) * 46)]
            line(c, p, 10); line(c, p, 4, WHITE)
        flat(c, circle(0, 0, 5), R, lw=3)
    icon("target", target, "Attack / target")

    def maneuver(c):
        cel(c, poly([(0, -38), (38, 0), (0, 38), (-38, 0)]), Bl, hexc("#3a86ff"), shift=6)
        for a, col in ((-math.pi / 2, G), (math.pi / 2, Y)):
            p = [(0, 0), (math.cos(a) * 28, math.sin(a) * 28)]
            line(c, p, 10); line(c, p, 4, col)
        flat(c, circle(0, 0, 8), WHITE, lw=3)
    icon("maneuver_node", maneuver, "Maneuver node")

    def notify(c):  # bell
        p = lambda cc: (cc.move_to(-30, 22), cc.curve_to(-22, 10, -26, -30, 0, -32), cc.curve_to(26, -30, 22, 10, 30, 22), cc.close_path())
        cel(c, p, Y, O, shift=6, hl=(-10, -16), hl_r=4)
        cel(c, circle(0, 30, 7), O, darker(O), shift=2, ht=False)
        line(c, [(-34, 22), (34, 22)], 7)
    icon("notification", notify, "Notification")

    def science_atom(c):
        for k in range(3):
            c.save(); c.rotate(k * math.pi / 3)
            ink(c, ellipse(0, 0, 40, 14), 9); ink(c, ellipse(0, 0, 40, 14), 4, P)
            c.restore()
        cel(c, circle(0, 0, 11), P, darker(P), shift=3, ht=False)
    icon("tech", science_atom, "Tech tree")

    def solar(c):
        cel(c, rrect(0, 0, 76, 44, 3), hexc("#4361ee"), hexc("#3a0ca3"), shift=5)
        for x in (-19, 0, 19):
            line(c, [(x, -22), (x, 22)], 2.5)
        line(c, [(-38, 0), (38, 0)], 2.5)
        flat(c, star_burst(30, -28, 14, 8, 8, 0), Y, lw=3)
    icon("solar", solar, "Solar panels (energy upgrade)")

    def reactor(c):
        cel(c, circle(0, 0, 38), Y, O, shift=7)
        for k in range(3):
            a = k * 2 * math.pi / 3 - math.pi / 2
            p = lambda cc, a=a: (cc.move_to(0, 0), cc.arc(0, 0, 30, a - .5, a + .5), cc.close_path())
            flat(c, p, INK, outline=False)
        flat(c, circle(0, 0, 8), INK, outline=False)
        ink(c, circle(0, 0, 8), 3, Y)
    icon("reactor", reactor, "Reactor (energy upgrade)")

    def refinery(c):
        cel(c, rrect(-10, 8, 36, 56, 6), *SL, shift=5)
        cel(c, rrect(22, 22, 22, 28, 4), *SL, shift=4, ht=False)
        cel(c, drop_path(-10, 8, 11), O, hexc("#e85d04"), shift=3, ht=False)
        for i, (x, y, r) in enumerate(((20, -14, 7), (28, -28, 9), (18, -38, 6))):
            cel(c, circle(x, y, r), hexc("#e9ecef"), hexc("#adb5bd"), shift=2, ht=False)
    icon("refinery", refinery, "Fuel refinery")

    def mine(c):  # pickaxe over ore
        cel(c, ngon(4, 18, 20, 6, .2), lighter(Bl, .3), hexc("#3a86ff"), shift=5)
        c.save(); c.rotate(-0.7)
        cel(c, rrect(0, -6, 8, 64, 3), hexc("#bc6c25"), hexc("#7f4f24"), shift=2, ht=False)
        p = lambda cc: (cc.move_to(-32, -30), cc.curve_to(-10, -46, 10, -46, 32, -30), cc.line_to(10, -36), cc.line_to(-10, -36), cc.close_path())
        cel(c, p, *SL, shift=3, ht=False)
        c.restore()
    icon("extraction", mine, "Matter extraction upgrade")

    def shield_i(c):
        p = lambda cc: (cc.move_to(0, -42), cc.curve_to(18, -32, 30, -32, 36, -32), cc.curve_to(36, 6, 24, 30, 0, 44),
                        cc.curve_to(-24, 30, -36, 6, -36, -32), cc.curve_to(-30, -32, -18, -32, 0, -42), cc.close_path())
        cel(c, p, Bl, hexc("#3a86ff"), shift=7, hl=(-14, -18), hl_r=4)
        flat(c, star_burst(0, 0, 14, 7, 5, -math.pi / 2), Y, lw=3)
    icon("shield", shield_i, "Shields / defense upgrade")

    def kinetic(c):
        for y in (-14, 14):
            cel(c, lambda cc, y=y: (cc.move_to(30, y), cc.curve_to(30, y - 10, 20, y - 10, 10, y - 10), cc.line_to(-30, y - 10),
                                    cc.line_to(-30, y + 10), cc.line_to(10, y + 10), cc.curve_to(20, y + 10, 30, y + 10, 30, y), cc.close_path()),
                Y, O, shift=4, ht=False)
            line(c, [(-20, y - 10), (-20, y + 10)], 3)
    icon("weapon_kinetic", kinetic, "Kinetic weapons")

    def energy_w(c):
        p = [(-40, 0), (-22, -14), (-6, 12), (10, -14), (26, 12), (40, 0)]
        line(c, p, 16); line(c, p, 8, Bl); line(c, p, 3, WHITE)
        flat(c, circle(-40, 0, 7), Bl, lw=3); flat(c, star_burst(40, 0, 13, 7, 6, 0), WHITE, lw=3)
    icon("weapon_energy", energy_w, "Energy weapons")

    def menu(c):
        for y in (-22, 0, 22):
            line(c, [(-30, y), (30, y)], 16); line(c, [(-30, y), (30, y)], 8, WHITE)
    icon("menu", menu, "Menu")

    def back(c):
        cel(c, poly([(-36, 0), (0, -34), (0, -14), (36, -14), (36, 14), (0, 14), (0, 34)]), Y, O, shift=5)
    icon("back", back, "Back")


# ---------------------------------------------------------------- UI bits, emblems, launcher icon
def gen_ui():
    U = MANIFEST["ui"]
    s, c = canvas(256, 256, 1.0)
    c.set_source_rgb(*hexc("#1b1340")); c.paint()
    rnd = random.Random(2)
    for _ in range(70):
        x, y = rnd.uniform(0, 256), rnd.uniform(0, 256)
        for ox in (-256, 0, 256):
            for oy in (-256, 0, 256):
                fill(c, circle(x + ox, y + oy, 3), WHITE, 0.06)
    for row in range(0, 16):
        for col in range(0, 16):
            fill(c, circle(col * 16 + (8 if row % 2 else 0), row * 16, 1.8), WHITE, 0.04)
    for _ in range(14):  # a few real stars
        x, y = rnd.uniform(8, 248), rnd.uniform(8, 248)
        flat(c, star_burst(x, y, rnd.uniform(2.5, 4.5), 1.2, 4, 0), WHITE, outline=False)
    U["bg_tile"] = {"file": save(s, "sprites/ui/bg_tile.png"), "px": 256, "tile": True,
                    "note": "tileable halftone + stars on #1b1340 (matches theme BG)"}
    s, c = canvas(256, 256, 1.0)
    for row in range(0, 16):
        for col in range(0, 16):
            fill(c, circle(col * 16 + (8 if row % 2 else 0), row * 16, 2.2), WHITE, 1)
    U["halftone_tile"] = {"file": save(s, "sprites/ui/halftone_tile.png"), "px": 256, "tile": True,
                          "note": "white dots on transparent; draw with low alpha / tint"}
    # selection ring (white+ink, tint)
    s, c = canvas(128, 128, 1.0); c.translate(64, 64)
    for k in range(8):
        a0 = k * math.pi / 4 + 0.12
        for w, col in ((10, INK), (5, WHITE)):
            c.new_path(); c.arc(0, 0, 54, a0, a0 + math.pi / 4 - 0.24); c.set_source_rgb(*col); c.set_line_width(w); c.stroke()
    U["select_ring"] = {"file": save(s, "sprites/ui/select_ring.png"), "px": 128, "tint": True, "note": "rotate slowly"}
    s, c = canvas(96, 96, 1.0); c.translate(48, 48)
    for a in range(4):
        aa = a * math.pi / 2 + math.pi / 4
        p = [(math.cos(aa - .35) * 40, math.sin(aa - .35) * 40), (math.cos(aa) * 30, math.sin(aa) * 30),
             (math.cos(aa + .35) * 40, math.sin(aa + .35) * 40)]
        line(c, p, 11); line(c, p, 5, WHITE)
    U["target_bracket"] = {"file": save(s, "sprites/ui/target_bracket.png"), "px": 96, "tint": True}
    s, c = canvas(48, 48, 1.0, 4); c.translate(24, 24)
    flat(c, poly([(0, -18), (18, 0), (0, 18), (-18, 0)]), WHITE)
    fill(c, circle(0, 0, 5), INK)
    U["node_marker"] = {"file": save(s, "sprites/ui/node_marker.png"), "px": 48, "tint": True,
                        "note": "maneuver node on a trajectory"}
    s, c = canvas(48, 48, 1.0, 4); c.translate(24, 24)
    flat(c, lambda cc: (cc.move_to(0, 20), cc.curve_to(-22, -4, -14, -20, 0, -20), cc.curve_to(14, -20, 22, -4, 0, 20), cc.close_path()), WHITE)
    fill(c, circle(0, -6, 6), INK)
    U["waypoint"] = {"file": save(s, "sprites/ui/waypoint.png"), "px": 48, "tint": True, "anchor": [0.5, 0.92]}
    s, c = canvas(64, 64, 1.0, 4); c.translate(32, 32)
    for r, w in ((26, 5), (16, 4)):
        c.new_path(); c.arc(0, 0, r, 0, 6.283); c.set_source_rgb(*INK); c.set_line_width(w + 5); c.stroke()
        c.new_path(); c.arc(0, 0, r, 0, 6.283); c.set_source_rgb(*WHITE); c.set_line_width(w); c.set_dash([6, 4]); c.stroke(); c.set_dash([])
    U["transfer_window"] = {"file": save(s, "sprites/ui/transfer_window.png"), "px": 64, "tint": True,
                            "note": "marks an optimal transfer window / intercept point"}


def gen_emblems():
    for skin, sk in SKINS.items():
        s, c = canvas(128, 128, 1.0, 5.5); c.translate(64, 64)
        m = tuple(hexc(x) for x in sk["metal"]); acc = hexc(sk["accent"])
        if skin == "vanguard":
            cel(c, poly([(0, -50), (44, -22), (34, 40), (0, 54), (-34, 40), (-44, -22)]), *m, shift=8)
            for s_ in (1, -1):
                cel(c, poly([(s_ * 8, 6), (s_ * 56, -14), (s_ * 50, 0), (s_ * 10, 24)]), hexc("#e9ecef"), hexc("#8d99ae"), shift=3, ht=False)
            flat(c, star_burst(0, 0, 24, 10, 5, -math.pi / 2), acc)
        elif skin == "hive":
            cel(c, blob(0, 0, 50, .1, 4, 10), *m, shift=8)
            for k in range(6):
                a = k * math.pi / 3
                flat(c, ngon(math.cos(a) * 26, math.sin(a) * 26, 11, 6, math.pi / 6), darker(m[1], .8), lw=3)
            flat(c, ellipse(0, 0, 18, 14), acc); fill(c, ellipse(2, 0, 4, 11), INK)
        elif skin == "prism":
            p = [(0, -54), (40, -10), (0, 54), (-40, -10)]
            cel(c, poly(p), *m, shift=8)
            for q in p:
                line(c, [(0, -6), q], 2.5, m[1])
            line(c, [(-40, -10), (40, -10)], 2.5, m[1])
            flat(c, ngon(0, -6, 13, 4), acc)
        else:
            cel(c, ngon(0, 0, 48, 8, math.pi / 8), *m, shift=8)
            for k in range(8):
                a = k * math.pi / 4 + math.pi / 8
                fill(c, circle(math.cos(a) * 40, math.sin(a) * 40, 3), INK)
            c.save(); c.rotate(math.pi / 4)
            cel(c, rrect(0, 0, 12, 60, 3), hexc("#ced4da"), hexc("#868e96"), shift=3, ht=False)
            cel(c, circle(0, -28, 12), hexc("#ced4da"), hexc("#868e96"), shift=3, ht=False)
            fill(c, rrect(0, -36, 8, 12, 1), m[1])
            c.restore()
            flat(c, circle(0, 0, 9), acc)
        MANIFEST["emblems"][skin] = save(s, f"sprites/emblems/{skin}.png")


def gen_launcher():
    def paint(c, S):
        u = S / 512
        c.scale(u, u)
        rrect(256, 256, 512, 512, 96)(c); c.clip()
        c.set_source_rgb(*hexc("#1b1340")); c.paint()
        for row in range(0, 33):
            for col in range(0, 33):
                fill(c, circle(col * 16 + (8 if row % 2 else 0), row * 16, 2.4), WHITE, 0.07)
        C.LW = 14
        c.save(); c.translate(140, 150)
        flat(c, star_burst(0, 0, 120, 88, 12, 0.2, .1, 3), hexc("#ff9f1c"))
        cel(c, circle(0, 0, 80), hexc("#ffd23f"), hexc("#ff9f1c"), shift=22)
        c.restore()
        c.new_path(); c.arc(256, 256, 250, 0.2, 1.6); c.set_source_rgb(1, 1, 1); c.set_line_width(8)
        c.set_dash([20, 16]); c.stroke(); c.set_dash([])
        c.save(); c.translate(330, 350)
        cel(c, circle(0, 0, 96), hexc("#4cc9f0"), hexc("#3a86ff"), shift=28, hl=(-38, -38), hl_r=16)
        c.restore()
        c.save(); c.translate(250, 300); c.rotate(-0.7); c.scale(110, 110)
        C.PX = 1 / 110; C.LW = 9 / 110
        d = Drawer(c, "vanguard", "red"); d.rnd = random.Random(1)
        ship_interceptor(d)
        c.restore()
        C.LW = 14
    out = {}
    for name, S in (("mdpi", 48), ("hdpi", 72), ("xhdpi", 96), ("xxhdpi", 144), ("xxxhdpi", 192), ("store", 512)):
        s, c = canvas(S, S)
        paint(c, S)
        out[name] = save(s, f"icon/ic_launcher_{name}.png" if name != "store" else "icon/ic_launcher_512.png")
    MANIFEST["ui"]["launcher_icon"] = out


def main():
    gen_ships(); gen_stations(); gen_bodies(); gen_fx(); gen_icons(); gen_ui(); gen_emblems(); gen_launcher()
    MANIFEST["conventions"] = {
        "paths": "relative to assets/; load with macroquad load_texture(\"assets/<path>\")",
        "format": "PNG RGBA, straight alpha, sRGB",
        "facing": "ships, shots and flames face +x (angle 0); rotate by atan2(vy, vx)",
        "team_variants": "ships/stations are pre-baked per team colour: <class>_<team>.png, team in blue|red|green|yellow|neutral",
        "tint": "sprites marked tint=true are white+black ink: multiply by a colour (draw_texture_ex color) to recolour",
        "flipbooks": "horizontal strips; frame i at x = i*frame_w",
        "bodies": "*_flat + shadow_overlay for dynamic lighting from the star, or *_lit pre-shaded (light from upper-left)",
        "px_points": "engines_px / guns_px are pixel offsets from the sprite centre, +x = nose, at native px size",
    }
    old = {}
    mp = os.path.join(C.ROOT, "manifest.json")
    if os.path.exists(mp):
        old = json.load(open(mp))
    MANIFEST["audio"] = old.get("audio", {})
    json.dump(MANIFEST, open(mp, "w"), indent=1)
    print("ok")


def main_only(names):
    """Regenerate just these ships/stations and merge them into the existing manifest."""
    global ONLY
    ONLY = set(names)
    gen_ships(); gen_stations()
    mp = os.path.join(C.ROOT, "manifest.json")
    m = json.load(open(mp))
    for k in ("ships", "stations"):
        m.setdefault(k, {}).update(MANIFEST[k])
    json.dump(m, open(mp, "w"), indent=1)
    print("ok", sorted(ONLY))


if __name__ == "__main__":
    if len(sys.argv) > 2 and sys.argv[1] == "--only":
        main_only(sys.argv[2].split(","))
    else:
        main()
