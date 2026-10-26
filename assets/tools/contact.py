#!/usr/bin/env python3
"""Contact sheets of the generated sprites → assets/preview/*.png (for review, not shipped)."""
import os, glob, sys
from PIL import Image, ImageDraw, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BG = (27, 19, 64)
FONT = ImageFont.truetype(os.path.expanduser("~/.local/share/fonts/Bangers-Regular.ttf"), 22)
SMALL = ImageFont.truetype(os.path.expanduser("~/.local/share/fonts/ComicNeue-Bold.ttf"), 13)


def sheet(name, rows, cell=140, cols=None, label=True):
    """rows: list of (title, [paths])."""
    cols = cols or max(len(r[1]) for r in rows)
    H = sum(cell + (34 if label else 0) + 30 for _ in rows) + 10
    W = cols * cell + 20
    im = Image.new("RGBA", (W, H), BG)
    d = ImageDraw.Draw(im)
    y = 10
    for title, paths in rows:
        d.text((12, y), title, font=FONT, fill="white")
        y += 30
        for i, p in enumerate(paths):
            sp = Image.open(os.path.join(ROOT, p)).convert("RGBA")
            sp.thumbnail((cell - 8, cell - 8))
            x = 10 + i * cell
            im.alpha_composite(sp, (x + (cell - sp.width) // 2, y + (cell - sp.height) // 2))
            if label:
                n = os.path.splitext(os.path.basename(p))[0]
                d.text((x + 4, y + cell + 2), n[:20], font=SMALL, fill=(255, 255, 255, 170))
        y += cell + (34 if label else 0)
    os.makedirs(os.path.join(ROOT, "preview"), exist_ok=True)
    im.convert("RGB").save(os.path.join(ROOT, "preview", name + ".png"))


def g(pat):
    return sorted(os.path.relpath(p, ROOT) for p in glob.glob(os.path.join(ROOT, pat)))


if __name__ == "__main__":
    classes = ["interceptor", "gunship", "bomber", "scout", "tanker", "freighter", "tug"]
    skins = ["vanguard", "hive", "prism", "junker"]
    teams = ["blue", "red", "green", "yellow"]
    sheet("ships", [(f"{s} · team {teams[i]}", [f"sprites/ships/{s}/{c}_{teams[i]}.png" for c in classes])
                    for i, s in enumerate(skins)] +
          [("vanguard · all teams (gunship)", [f"sprites/ships/vanguard/gunship_{t}.png" for t in teams + ['neutral']])],
          cell=150)
    st = ["shipyard", "fuel_depot", "telescope", "scanner", "defense", "relay"]
    sheet("stations", [(f"{s} · {teams[i]}", [f"sprites/stations/{s}/{n}_{teams[i]}.png" for n in st])
                       for i, s in enumerate(skins)], cell=190)
    sheet("bodies", [("stars", g("sprites/bodies/star_*.png")),
                     ("planets (lit)", g("sprites/bodies/planet_*_lit.png")[:6]),
                     ("", g("sprites/bodies/planet_*_lit.png")[6:]),
                     ("planets (flat + shadow overlay)", g("sprites/bodies/planet_*_flat.png")[:5] + ["sprites/bodies/shadow_overlay.png"]),
                     ("moons", g("sprites/bodies/moon_*_lit.png")),
                     ("asteroids", g("sprites/bodies/asteroid_*.png"))], cell=170)
    sheet("fx", [(os.path.basename(p)[:-4], [p]) for p in g("sprites/fx/explosion_*.png") + g("sprites/fx/asteroid_impact.png")]
          , cell=1300, label=False)
    sheet("fx_small", [("strips", g("sprites/fx/impact_*.png") + g("sprites/fx/shield_hit.png") + g("sprites/fx/muzzle_flash.png")
                        + g("sprites/fx/engine_flame.png") + g("sprites/fx/smoke_puff.png") + g("sprites/fx/scan_ping.png")),
                       ("shots + debris", g("sprites/fx/kinetic_*.png") + g("sprites/fx/energy_*.png") + g("sprites/fx/debris_*.png")),
                       ("words", g("sprites/fx/word_*.png"))], cell=170)
    ic = g("sprites/icons/*.png")
    res = [f"sprites/icons/{n}.png" for n in ["matter", "energy", "fuel", "research", "upgrades", "power"]]
    rest = [p for p in ic if p not in res and "ship_" not in p and "station_" not in p]
    sheet("icons", [("resources", res)] + [("ui", rest[i:i + 10]) for i in range(0, len(rest), 10)] +
          [("ship + station silhouettes (tintable)", g("sprites/icons/ship_*.png") + g("sprites/icons/station_*.png"))],
          cell=110, cols=13)
    sheet("misc", [("emblems", g("sprites/emblems/*.png")), ("ui", g("sprites/ui/*.png")),
                   ("launcher", ["icon/ic_launcher_512.png", "icon/ic_launcher_xxxhdpi.png", "icon/ic_launcher_mdpi.png"])
                   ], cell=160)
    print("ok")
