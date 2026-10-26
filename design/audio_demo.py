#!/usr/bin/env python3
"""Synthesise four audio-mood samples: music bed, then the same five SFX
(engine burn, energy weapon, kinetic cannon, explosion, alert) in that mood."""
import numpy as np, wave, os

SR = 22050
rng = np.random.default_rng(1)
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "audio")
os.makedirs(OUT, exist_ok=True)

def t_(d): return np.arange(int(d * SR)) / SR
def midi(n): return 440 * 2 ** ((n - 69) / 12)
def env(n, a=.01, r=.2):
    e = np.ones(n); ai, ri = max(1, int(a * SR)), max(1, int(r * SR))
    e[:ai] = np.linspace(0, 1, ai); e[-ri:] *= np.linspace(1, 0, ri); return e
def lp(x, cut):  # one-pole lowpass, cut may be array
    a = np.exp(-2 * np.pi * np.broadcast_to(cut, x.shape) / SR); y = np.empty_like(x); s = 0.0
    for i in range(len(x)): s = (1 - a[i]) * x[i] + a[i] * s; y[i] = s
    return y
def saw(f, t): return 2 * ((f * t) % 1) - 1
def sq(f, t, d=.5): return np.where((f * t) % 1 < d, 1., -1.)
def tri(f, t): return 2 * np.abs(saw(f, t)) - 1
def noise(n): return rng.uniform(-1, 1, n)
def reverb(x, mix=.35):
    y = x.copy()
    for d, g in [(.037, .5), (.071, .45), (.113, .4), (.173, .35), (.241, .3)]:
        k = int(d * SR); z = np.zeros_like(x)
        for rep in range(1, 8):
            if k * rep >= len(x): break
            z[k * rep:] += x[:-k * rep] * g ** rep
        y += z * mix / 2
    return y
def place(buf, x, at):
    i = int(at * SR); n = min(len(x), len(buf) - i); buf[i:i + n] += x[:n]
def norm(x, p=.9): return x / (np.max(np.abs(x)) + 1e-9) * p

PROG = [[57, 60, 64], [53, 57, 60], [48, 52, 55], [55, 59, 62]]  # Am F C G
MUS, TOTAL = 18.0, 30.0
SFX_AT = [19.0, 21.5, 23.5, 25.5, 28.0]

# ---------- moods ----------
def ambient():
    m = np.zeros(int(TOTAL * SR)); bar = 4.5
    for k in range(int(TOTAL / bar) + 1):
        tt = t_(bar + 1.5)
        for n in PROG[k % 4]:
            v = sum(saw(midi(n) * (1 + d), tt) for d in (-.004, 0, .005))
            v = lp(v, 400 + 300 * np.sin(tt * .6)) * env(len(tt), 1.5, 1.5) * .12
            place(m, v, k * bar)
        place(m, np.sin(2 * np.pi * midi(PROG[k % 4][0] - 24) * tt) * env(len(tt), 1, 1) * .2, k * bar)
    for k in range(40):  # sparkles
        tt = t_(.8); n = PROG[(k // 5) % 4][k % 3] + 24
        place(m, np.sin(2 * np.pi * midi(n) * tt) * np.exp(-tt * 5) * .05, k * .45 + rng.uniform(0, .2))
    m = reverb(m, .6)
    sfx = dict(
        burn=lambda: lp(noise(int(1.8 * SR)), 300) * env(int(1.8 * SR), .3, .6) * 2.5,
        energy=lambda: (lambda tt: np.sin(2 * np.pi * (1800 * np.exp(-tt * 6) + 300) * tt) * np.exp(-tt * 6))(t_(.6)),
        kinetic=lambda: (lambda tt: (lp(noise(len(tt)), 900) * 2 + np.sin(2 * np.pi * 70 * tt)) * np.exp(-tt * 10))(t_(.5)),
        boom=lambda: (lambda tt: lp(noise(len(tt)), 200 + 800 * np.exp(-tt * 3)) * np.exp(-tt * 1.6) * 3)(t_(2.2)),
        alert=lambda: sum(np.sin(2 * np.pi * midi(n) * t_(1.2)) * np.exp(-t_(1.2) * 4) * (t_(1.2) >= 0)
                          for n in (76,)) + np.pad(np.sin(2 * np.pi * midi(83) * t_(1.0)) * np.exp(-t_(1.0) * 4), (int(.2 * SR), 0))[:int(1.2 * SR)])
    return m, sfx, .35

def chiptune():
    m = np.zeros(int(TOTAL * SR)); step = .125
    for i in range(int(TOTAL / step)):
        ch = PROG[(i // 32) % 4]; tt = t_(step)
        n = ch[i % 3] + 12 + (12 if i % 6 == 5 else 0)
        place(m, sq(midi(n), tt, .25) * env(len(tt), .002, .03) * .12, i * step)
        if i % 2 == 0: place(m, tri(midi(ch[0] - 12), t_(step * 2)) * env(int(step * 2 * SR), .002, .05) * .3, i * step)
        if i % 8 == 0: place(m, (lambda u: np.sin(2 * np.pi * (150 * np.exp(-u * 30) + 50) * u) * np.exp(-u * 12))(t_(.25)) * .6, i * step)
        if i % 8 == 4: place(m, noise(int(.1 * SR)) * np.exp(-t_(.1) * 30) * .3, i * step)
        if i % 2 == 1: place(m, noise(int(.03 * SR)) * .08, i * step)
    def burn():
        tt = t_(1.8); return sq(55 + 20 * np.sin(tt * 40), tt, .5) * .3 * env(len(tt), .05, .4) + noise(len(tt)) * .2 * env(len(tt), .05, .4)
    def energy():
        tt = t_(.4); return sq(2000 * np.exp(-tt * 8) + 200, tt, .5) * np.exp(-tt * 5) * .6
    def kinetic():
        tt = t_(.35); return (noise(len(tt)) * .7 + sq(90, tt)) * np.exp(-tt * 12)
    def boom():
        tt = t_(1.6); hold = np.repeat(noise(len(tt) // 40 + 1), 40)[:len(tt)]; return hold * np.exp(-tt * 2.2)
    def alert():
        out = np.zeros(int(1.2 * SR))
        for j, n in enumerate((84, 88, 84, 88)): place(out, sq(midi(n), t_(.12), .5) * .4, j * .14)
        return out
    return m, dict(burn=burn, energy=energy, kinetic=kinetic, boom=boom, alert=alert), .4

def cinematic():
    m = np.zeros(int(TOTAL * SR)); bar = 4.5
    drone = t_(TOTAL)
    m += lp(sum(saw(midi(33) * (1 + d), drone) for d in (-.003, 0, .003)), 120) * .25 * env(len(drone), 2, 2)
    for k in range(int(TOTAL / bar) + 1):
        tt = t_(bar + 2)
        swell = np.clip(tt / (bar * .8), 0, 1) ** 2 * env(len(tt), .01, 2)
        for n in PROG[k % 4] + [PROG[k % 4][0] + 12]:
            v = sum(saw(midi(n) * (1 + d), tt + rng.uniform(0, 1)) for d in (-.006, -.002, .002, .006))
            place(m, lp(v, 900) * swell * .07, k * bar)
        for h in (0, 1.5, 2.25):  # timpani hits
            u = t_(1.5); place(m, np.sin(2 * np.pi * (midi(33) * (1 + .3 * np.exp(-u * 20))) * u) * np.exp(-u * 3) * .7, k * bar + h)
    m = reverb(m, .5)
    def burn():
        tt = t_(2.0); return lp(noise(len(tt)), 150 + 100 * tt) * env(len(tt), .5, .5) * 4 + np.sin(2 * np.pi * 40 * tt) * env(len(tt), .5, .5) * .4
    def energy():
        tt = t_(.9); return (np.sin(2 * np.pi * (900 - 500 * tt) * tt) + .5 * saw(450 - 200 * tt, tt)) * np.exp(-tt * 4) * .6
    def kinetic():
        tt = t_(.8); return (lp(noise(len(tt)), 600) * 3 + np.sin(2 * np.pi * 45 * tt) * 1.2) * np.exp(-tt * 6)
    def boom():
        tt = t_(3.0); return (lp(noise(len(tt)), 100 + 600 * np.exp(-tt * 2)) * 4 + np.sin(2 * np.pi * 30 * tt)) * np.exp(-tt * 1.1)
    def alert():
        tt = t_(1.3); return (sq(midi(62), tt) * .3 + saw(midi(62.3), tt) * .3) * env(len(tt), .02, .3) * (np.sin(2 * np.pi * 3 * tt) > 0)
    return m, dict(burn=burn, energy=energy, kinetic=kinetic, boom=boom, alert=alert), .3

def sonar():
    """Dark minimal: pulsing sub, sonar pings, ticking clock — fits star-chart/CRT looks."""
    m = np.zeros(int(TOTAL * SR)); beat = .5
    for i in range(int(TOTAL / beat)):
        u = t_(beat); ch = PROG[(i // 16) % 4]
        place(m, np.sin(2 * np.pi * midi(ch[0] - 24) * u) * np.exp(-u * 4) * .45, i * beat)
        place(m, noise(int(.01 * SR)) * .12, i * beat + beat / 2)
        if i % 8 == 0:
            p = t_(3.0); place(m, np.sin(2 * np.pi * midi(ch[1] + 24) * p) * np.exp(-p * 1.8) * .2, i * beat)
        if i % 16 == 6:
            p = t_(2.0); place(m, lp(saw(midi(ch[2]), p), 500) * env(len(p), .4, 1) * .12, i * beat)
    m = reverb(m, .7)
    def burn():
        tt = t_(1.8); return lp(noise(len(tt)), 250) * (1 + .5 * np.sin(2 * np.pi * 12 * tt)) * env(len(tt), .2, .5) * 2.5
    def energy():
        tt = t_(.5); return np.sin(2 * np.pi * 2400 * tt) * np.exp(-tt * 14) * .5 + np.sin(2 * np.pi * 1200 * tt) * np.exp(-tt * 9) * .3
    def kinetic():
        tt = t_(.4); return (np.sin(2 * np.pi * 60 * tt) * 1.2 + lp(noise(len(tt)), 2500) * .8 * np.exp(-tt * 40)) * np.exp(-tt * 9)
    def boom():
        tt = t_(2.5); return (lp(noise(len(tt)), 300 * np.exp(-tt) + 60) * 3) * np.exp(-tt * 1.3)
    def alert():
        out = np.zeros(int(1.4 * SR))
        for j in range(2): u = t_(.6); place(out, np.sin(2 * np.pi * 1000 * u) * np.exp(-u * 6) * .7, j * .5)
        return out
    return m, dict(burn=burn, energy=energy, kinetic=kinetic, boom=boom, alert=alert), .35

def render(name, fn):
    m, sfx, sfx_music = fn()
    m = norm(m, .55)
    duck = np.ones(len(m)); i = int(MUS * SR); duck[i:] = sfx_music; duck[i - SR:i] = np.linspace(1, sfx_music, SR)
    m *= duck
    m[-SR:] *= np.linspace(1, 0, SR)
    for at, key in zip(SFX_AT, ("burn", "energy", "kinetic", "boom", "alert")):
        place(m, norm(sfx[key](), .7), at)
    m = np.clip(m, -1, 1)
    with wave.open(os.path.join(OUT, name + ".wav"), "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(SR)
        w.writeframes((m * 32000).astype("<i2").tobytes())
    print("wrote", name)

for name, fn in [("1-ambient-synth", ambient), ("2-retro-chiptune", chiptune),
                 ("3-cinematic", cinematic), ("4-dark-sonar", sonar)]:
    render(name, fn)
