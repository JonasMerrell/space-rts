#!/usr/bin/env python3
"""Synthesise music + SFX for KEPLER CLASH! into assets/audio/ (16-bit mono WAV, 22050 Hz).
Moods: "ambient" (default, Jonas's pick: warm detuned-saw pads, bells, reverb) or "chiptune".
Deterministic. Music loops are seamless (tails wrap to the start).
Usage: python3 assets/tools/gen_audio.py [--mood ambient|chiptune]"""
import numpy as np, wave, os, json, sys
from scipy.signal import lfilter, butter

SR = 22050
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "audio")
rng = np.random.default_rng(7)


# ---------------------------------------------------------------- dsp helpers
def t_(d): return np.arange(int(d * SR)) / SR
def midi(n): return 440.0 * 2 ** ((n - 69) / 12)
def phase(f):  # f: scalar or per-sample array → cumulative phase in cycles
    f = np.asarray(f, float)
    return np.cumsum(f) / SR if f.ndim else None
def pulse(freq, n, duty=0.5):
    ph = np.cumsum(np.broadcast_to(freq, (n,)).astype(float)) / SR
    return np.where(ph % 1 < duty, 1.0, -1.0)
def tri(freq, n):
    ph = np.cumsum(np.broadcast_to(freq, (n,)).astype(float)) / SR
    return 4 * np.abs((ph % 1) - 0.5) - 1
def saw(freq, n):
    ph = np.cumsum(np.broadcast_to(freq, (n,)).astype(float)) / SR
    return 2 * (ph % 1) - 1
def sine(freq, n):
    ph = np.cumsum(np.broadcast_to(freq, (n,)).astype(float)) / SR
    return np.sin(2 * np.pi * ph)
def noise(n): return rng.uniform(-1, 1, n)
def lfsr_noise(n, rate):  # NES-style stepped noise: sample & hold at `rate` Hz
    step = max(1, int(SR / rate))
    v = rng.choice([-1.0, 1.0], n // step + 2)
    return np.repeat(v, step)[:n]
def lp(x, cut, order=2):
    b, a = butter(order, min(cut, SR / 2 * 0.95) / (SR / 2))
    return lfilter(b, a, x)
def hp(x, cut, order=2):
    b, a = butter(order, cut / (SR / 2), btype="high")
    return lfilter(b, a, x)
def adsr(n, a=0.005, d=0.05, s=0.6, r=0.05):
    e = np.full(n, float(s))
    ai, di, ri = int(a * SR), int(d * SR), int(r * SR)
    ai = min(ai, n); e[:ai] = np.linspace(0, 1, ai) if ai else e[:ai]
    di = min(di, n - ai); e[ai:ai + di] = np.linspace(1, s, di) if di else e[ai:ai + di]
    ri = min(ri, n); e[n - ri:] *= np.linspace(1, 0, ri) if ri else 1
    return e
def expd(n, k): return np.exp(-np.arange(n) / SR * k)
def place(buf, x, at):
    i = int(at * SR)
    if i >= len(buf): return
    n = min(len(x), len(buf) - i); buf[i:i + n] += x[:n]
def echo(x, delay, fb, mix):
    k = int(delay * SR); y = x.copy()
    for rep in range(1, 6):
        if k * rep >= len(x): break
        y[k * rep:] += x[:-k * rep] * (fb ** rep) * mix
    return y
def norm(x, peak=0.89):
    return x / (np.max(np.abs(x)) + 1e-9) * peak
def write(rel, x, peak=0.89, rms=None):
    """Peak-normalise, then cap loudness so square-wave SFX are not harsher than the rest
    (music passes rms= to be levelled up with a soft clip instead)."""
    x = norm(x, peak)
    r = np.sqrt(np.mean(x ** 2)) + 1e-9
    if rms:
        x = np.tanh(x * rms / r * 1.1) / np.tanh(1.1)
    elif r > 0.28:
        x *= 0.28 / r
    path = os.path.join(OUT, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with wave.open(path, "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(SR)
        w.writeframes((np.clip(x, -1, 1) * 32767).astype("<i2").tobytes())
    return "audio/" + rel


# ---------------------------------------------------------------- chip instruments
def i_lead(f, dur, vel=1.0, duty=0.25, vib=True):
    n = int(dur * SR)
    tt = np.arange(n) / SR
    fr = f * (1 + (0.006 * np.sin(2 * np.pi * 5.5 * tt) * np.clip((tt - 0.12) * 6, 0, 1) if vib else 0))
    return pulse(fr, n, duty) * adsr(n, 0.004, 0.08, 0.55, min(0.04, dur / 3)) * vel * 0.32
def i_bass(f, dur, vel=1.0):
    n = int(dur * SR)
    return tri(f, n) * adsr(n, 0.002, 0.05, 0.8, 0.02) * vel * 0.55
def i_arp(chord, dur, vel=1.0, rate=0.045, duty=0.125):
    n = int(dur * SR); out = np.zeros(n); step = int(rate * SR)
    for k, i in enumerate(range(0, n, step)):
        m = min(step, n - i)
        out[i:i + m] = pulse(midi(chord[k % len(chord)]), m, duty)
    return out * adsr(n, 0.002, 0.1, 0.5, 0.03) * vel * 0.15
def i_pad(chord, dur, vel=1.0):
    n = int(dur * SR)
    x = sum(pulse(midi(c) * (1 + d), n, 0.5) for c in chord for d in (-0.003, 0.003))
    return lp(x, 1400) * adsr(n, 0.15, 0.3, 0.7, 0.3) * vel * 0.05
def d_kick(vel=1.0):
    n = int(0.16 * SR); tt = np.arange(n) / SR
    f = 50 + 140 * np.exp(-tt * 40)
    return np.sign(sine(f, n)) * 0.6 * expd(n, 22) * vel + sine(f, n) * expd(n, 18) * 0.6 * vel
def d_snare(vel=1.0):
    n = int(0.14 * SR)
    return (lfsr_noise(n, 9000) * expd(n, 24) + tri(190 * np.exp(-np.arange(n) / SR * 20), n) * expd(n, 30) * .5) * vel * 0.5
def d_hat(vel=1.0, open_=False):
    n = int((0.12 if open_ else 0.035) * SR)
    return hp(lfsr_noise(n, 16000), 6000) * expd(n, 18 if open_ else 80) * vel * 0.32


# ---------------------------------------------------------------- composition helpers
MAJ, MIN = [0, 2, 4, 5, 7, 9, 11], [0, 2, 3, 5, 7, 8, 10]
def chord_of(root, scale, degree):
    return [root + scale[(degree + k) % 7] + 12 * ((degree + k) // 7) for k in (0, 2, 4)]


class Song:
    def __init__(self, bpm, bars, beats_per_bar=4):
        self.bpm, self.bars, self.bpb = bpm, bars, beats_per_bar
        self.beat = 60 / bpm
        self.length = bars * beats_per_bar * self.beat
        self.buf = np.zeros(int((self.length + 8) * SR))
    def at(self, bar, beat): return (bar * self.bpb + beat) * self.beat
    def add(self, x, bar, beat): place(self.buf, x, self.at(bar, beat))
    def render(self, fx=None):
        n = int(self.length * SR)
        buf = fx(self.buf) if fx else self.buf
        out = buf[:n].copy()
        tail = buf[n:]
        out[:len(tail)] += tail  # seamless loop: wrap the release tails to the start
        return out


def melody_bar(rnd, chord, scale_notes, pattern, prev):
    """Pick pitches for a rhythm pattern [(beat, dur)], favouring chord tones near prev."""
    out = []
    for i, (b, d) in enumerate(pattern):
        strong = b == int(b)
        pool = [n for n in scale_notes if (n % 12) in [c % 12 for c in chord]] if strong else scale_notes
        pool = [n for n in pool if abs(n - prev) <= 7] or pool
        w = np.array([1.0 / (1 + abs(n - prev)) for n in pool])
        n = pool[rnd.choice(len(pool), p=w / w.sum())]
        out.append((b, d, n)); prev = n
    return out, prev


PATTERNS = [
    [(0, 1), (1, .5), (1.5, .5), (2, 1.5), (3.5, .5)],
    [(0, .5), (.5, .5), (1, 1), (2, .5), (2.5, .5), (3, 1)],
    [(0, 1.5), (1.5, .5), (2, 2)],
    [(0, .75), (.75, .75), (1.5, .5), (2, 1), (3, .5), (3.5, .5)],
    [(0, 2), (2, 1), (3, 1)],
]


def compose(seed, root, scale, prog, bpm, sections, lead_oct=12, duty=0.25, drums="full", arp=True, pad=False,
            bass_style="octaves", swing=0.0):
    """sections: list of (name, n_bars, intensity 0..1). Prog cycles per bar."""
    rnd = np.random.default_rng(seed)
    bars = sum(s[1] for s in sections)
    S = Song(bpm, bars)
    scale_notes = [root + lead_oct + o * 12 + s for o in (-1, 0, 1) for s in scale]
    motifs = {}
    bar = 0
    prev = root + lead_oct + 7
    for name, nb, inten in sections:
        for k in range(nb):
            deg = prog[(bar) % len(prog)]
            ch = chord_of(root, scale, deg)
            # bass
            if bass_style == "octaves":
                for b in range(8):
                    S.add(i_bass(midi(ch[0] - 24 + (12 if b % 2 else 0)), S.beat * 0.45, 0.9), bar, b * 0.5)
            elif bass_style == "drive":
                for b in range(8):
                    S.add(i_bass(midi(ch[0] - 24), S.beat * 0.4, 1.0 if b % 2 == 0 else .7), bar, b * 0.5)
            else:  # whole notes + fifth
                S.add(i_bass(midi(ch[0] - 24), S.beat * 1.9), bar, 0)
                S.add(i_bass(midi(ch[0] - 17), S.beat * 1.9, .8), bar, 2)
            if arp and inten >= 0.3:
                S.add(i_arp(ch + [ch[0] + 12], S.beat * 4, 0.8 + 0.4 * inten), bar, 0)
            if pad:
                S.add(i_pad(ch, S.beat * 4, 0.6 + 0.5 * inten), bar, 0)
            # drums
            if drums and inten > 0.15:
                for b in range(4):
                    if drums == "full" or b in (0, 2):
                        S.add(d_kick(), bar, b if drums == "full" and inten > .7 else (b if b % 2 == 0 else 99))
                    if b % 2 == 1 and inten > 0.35:
                        S.add(d_snare(), bar, b)
                for h in range(8):
                    if inten > 0.5 or h % 2 == 0:
                        S.add(d_hat(0.8 if h % 2 else 1.0, open_=(h == 7 and k % 2 == 1)), bar, h * 0.5 + (swing if h % 2 else 0))
                if k == nb - 1 and inten > 0.5:  # fill
                    for f in range(4):
                        S.add(d_snare(0.6 + f * .1), bar, 3 + f * 0.25)
            # lead melody: 2-bar motifs, repeated within a section (A A' style)
            if inten >= 0.25:
                key = (name, k % 2)
                if key not in motifs:
                    pat = PATTERNS[rnd.integers(len(PATTERNS))]
                    motifs[key], prev = melody_bar(rnd, ch, scale_notes, pat, prev)
                notes = motifs[key]
                if k % 4 == 3:  # vary the last bar of each phrase: land on the chord root
                    notes = notes[:-1] + [(notes[-1][0], notes[-1][1], min(scale_notes, key=lambda n: abs(n - (ch[0] + lead_oct)) if n % 12 == ch[0] % 12 else 99))]
                for b, d, n in notes:
                    # keep melody consonant with the current chord on strong beats
                    if b == int(b) and (n % 12) not in [c % 12 for c in ch]:
                        n = min([c + lead_oct + o for c in ch for o in (-12, 0, 12)], key=lambda m: abs(m - n))
                    S.add(i_lead(midi(n), d * S.beat * 0.95, 0.8 + 0.3 * inten, duty), bar, b)
            bar += 1
    return S


# ---------------------------------------------------------------- music
def music():
    M = {}
    # Title theme: heroic major, I–V–vi–IV, 144 bpm
    s = compose(11, 60, MAJ, [0, 4, 5, 3], 144,
                [("intro", 4, 0.3), ("a", 8, 0.8), ("b", 8, 1.0), ("a", 8, 0.8), ("out", 4, 0.5)], duty=0.25)
    x = echo(s.render(), s.beat * 0.75, 0.35, 0.5)
    M["title"] = {"file": write("music/title.wav", x, rms=0.16), "loop": True, "seconds": round(s.length, 1),
                  "use": "main menu / lobby"}
    # Orbit (in-game calm): dorian-ish minor, slow arps, sparse lead
    s = compose(23, 57, MIN, [0, 5, 2, 6], 96,
                [("calm", 8, 0.3), ("drift", 8, 0.45), ("calm", 8, 0.3), ("lift", 8, 0.55)],
                duty=0.125, drums="half", pad=True, bass_style="long")
    x = echo(s.render(), s.beat * 0.5, 0.45, 0.6)
    M["orbit"] = {"file": write("music/orbit.wav", x, rms=0.14), "loop": True, "seconds": round(s.length, 1),
                  "use": "in-game map, default bed (keep volume ~0.5)"}
    # Battle: driving minor, i–VI–VII–i, 168 bpm
    s = compose(37, 52, MIN, [0, 5, 6, 0, 0, 5, 3, 4], 168,
                [("a", 8, 0.9), ("b", 8, 1.0), ("a", 8, 0.9), ("break", 4, 0.6)], duty=0.5,
                bass_style="drive")
    x = echo(s.render(), s.beat * 0.5, 0.3, 0.4)
    M["battle"] = {"file": write("music/battle.wav", x, rms=0.17), "loop": True, "seconds": round(s.length, 1),
                   "use": "crossfade in when combat is happening on screen"}
    # Jingles
    def jingle(notes, bpm, bass, name, use):
        beat = 60 / bpm
        L = sum(d for _, d in notes) * beat + 1.2
        buf = np.zeros(int(L * SR)); t = 0
        for n, d in notes:
            if n is not None:
                place(buf, i_lead(midi(n), d * beat * 0.95, 1.0, 0.25), t)
                place(buf, i_lead(midi(n - 12), d * beat * 0.95, 0.5, 0.5, False), t)
            t += d * beat
        for n, at, d in bass:
            place(buf, i_bass(midi(n), d * beat), at * beat)
        M[name] = {"file": write(f"music/{name}.wav", echo(buf, beat * .5, .3, .4)), "loop": False,
                   "seconds": round(L, 1), "use": use}
    jingle([(72, .5), (76, .5), (79, .5), (84, 1), (None, .5), (79, .5), (84, 2)], 140,
           [(48, 0, 2), (43, 2, 1), (48, 3, 2)], "victory", "game won")
    jingle([(69, 1), (68, 1), (67, 1), (66, 3)], 100, [(45, 0, 2), (44, 2, 1), (38, 3, 3)], "defeat", "game lost")
    return M


# ---------------------------------------------------------------- sfx
def chip_blips(notes, d=0.09, gap=0.02, duty=0.25, tail=0.25):
    L = len(notes) * (d + gap) + tail; buf = np.zeros(int(L * SR))
    for i, nn in enumerate(notes):
        m = int(d * SR); place(buf, pulse(midi(nn), m, duty) * adsr(m, .002, .02, .7, .02), i * (d + gap))
    return echo(buf, .11, .35, .5)


def sfx():
    S = {}
    # engine burn: seamless 2 s loop of filtered stepped noise + low pulse rumble
    n = int(2.0 * SR)
    base = lp(lfsr_noise(n + SR, 3000), 900) * 0.8 + pulse(55, n + SR, 0.3) * 0.15
    base = lp(base, 1200)
    loop = base[:n].copy(); xf = int(0.3 * SR)
    w = np.linspace(0, 1, xf)
    loop[:xf] = loop[:xf] * w + base[n:n + xf] * (1 - w)  # crossfade tail into head
    S["engine_burn_loop"] = {"file": write("sfx/engine_burn_loop.wav", loop, 0.7), "loop": True}
    n = int(0.9 * SR); tt = np.arange(n) / SR
    raw = lfsr_noise(n, 3000); k = np.clip(tt * 3, 0, 1)
    ign = lp(raw, 400) * (1 - k) + lp(raw, 1600) * k  # opening filter sweep
    ign = ign * np.clip(tt * 5, 0, 1) + pulse(40 + 60 * tt, n, .3) * .2 * expd(n, 2)
    S["engine_burn_start"] = {"file": write("sfx/engine_burn_start.wav", ign * adsr(n, .01, .1, 1, .2)), "loop": False}
    # energy weapon: chip "pew" — fast falling pulse sweep + sparkle
    n = int(0.32 * SR); tt = np.arange(n) / SR
    f = 2400 * np.exp(-tt * 14) + 300
    x = pulse(f, n, 0.25) * expd(n, 9) + hp(lfsr_noise(n, 12000), 4000) * expd(n, 40) * .3
    S["energy_weapon"] = {"file": write("sfx/energy_weapon.wav", x), "loop": False}
    n = int(0.6 * SR); tt = np.arange(n) / SR  # beam hum
    x = pulse(220 + 30 * np.sin(2 * np.pi * 18 * tt), n, .5) * .5 + pulse(442, n, .125) * .3
    S["energy_beam"] = {"file": write("sfx/energy_beam.wav", lp(x, 2500) * adsr(n, .01, .05, .8, .15)), "loop": False}
    # kinetic cannon: thump + noise crack
    n = int(0.35 * SR); tt = np.arange(n) / SR
    x = d_kick(1.0)[:n] if len(d_kick()) >= n else np.pad(d_kick(), (0, n - len(d_kick())))
    x = x * 1.2 + lp(lfsr_noise(n, 6000), 3000) * expd(n, 16) * .9
    S["kinetic_cannon"] = {"file": write("sfx/kinetic_cannon.wav", x), "loop": False}
    n = int(0.6 * SR); tt = np.arange(n) / SR
    x = np.sign(sine(35 + 100 * np.exp(-tt * 25), n)) * expd(n, 9) + lp(lfsr_noise(n, 3000), 1500) * expd(n, 8)
    S["kinetic_cannon_heavy"] = {"file": write("sfx/kinetic_cannon_heavy.wav", x), "loop": False}
    # explosions
    def boom(d, rate, cut, k, sub):
        n = int(d * SR); tt = np.arange(n) / SR
        r = lfsr_noise(n, rate)
        r = lp(r, cut) * expd(n, k)
        r += np.sign(sine(sub * np.exp(-tt * 3) + 20, n)) * expd(n, k * 1.5) * .5
        return r
    S["explosion_small"] = {"file": write("sfx/explosion_small.wav", boom(0.8, 4000, 2500, 6, 90)), "loop": False}
    S["explosion_large"] = {"file": write("sfx/explosion_large.wav",
                                          echo(boom(2.0, 2200, 1400, 2.6, 60), .09, .4, .5)), "loop": False}
    n = int(3.2 * SR)
    x = echo(boom(3.2, 1200, 700, 1.5, 45), .13, .5, .6)
    x[:int(.05 * SR)] += np.linspace(1, 0, int(.05 * SR)) * 2
    S["asteroid_impact"] = {"file": write("sfx/asteroid_impact.wav", x), "loop": False}
    # hits
    n = int(0.12 * SR)
    S["hit_kinetic"] = {"file": write("sfx/hit_kinetic.wav", (lfsr_noise(n, 7000) * .7 + pulse(160, n, .5) * .4) * expd(n, 35)),
                        "loop": False}
    n = int(0.3 * SR); tt = np.arange(n) / SR
    x = pulse(900 + 600 * np.sin(2 * np.pi * 30 * tt), n, .5) * expd(n, 12) * .6 + hp(lfsr_noise(n, 14000), 5000) * expd(n, 20) * .4
    S["shield_hit"] = {"file": write("sfx/shield_hit.wav", x), "loop": False}
    # alerts
    blips = chip_blips
    S["alert_ping"] = {"file": write("sfx/alert_ping.wav", blips([88, 93], 0.1)), "loop": False, "use": "generic attention"}
    n = int(1.2 * SR); tt = np.arange(n) / SR
    x = pulse(np.where((tt * 3) % 1 < .5, 660, 520), n, .5) * adsr(n, .01, .05, .9, .1)
    S["alert_enemy"] = {"file": write("sfx/alert_enemy.wav", x), "loop": False, "use": "hostile fleet detected / under attack"}
    S["ui_click"] = {"file": write("sfx/ui_click.wav", pulse(1800, int(.025 * SR), .5) * expd(int(.025 * SR), 120), .6), "loop": False}
    S["ui_tap"] = {"file": write("sfx/ui_tap.wav", tri(900, int(.04 * SR)) * expd(int(.04 * SR), 90), .6), "loop": False}
    S["ui_confirm"] = {"file": write("sfx/ui_confirm.wav", blips([76, 83], 0.06, 0.0, tail=.1)), "loop": False}
    S["ui_back"] = {"file": write("sfx/ui_back.wav", blips([79, 72], 0.06, 0.0, .5, tail=.1)), "loop": False}
    S["ui_error"] = {"file": write("sfx/ui_error.wav", blips([50, 50], 0.1, 0.04, .5, tail=.1)), "loop": False}
    S["notification"] = {"file": write("sfx/notification.wav", blips([84, 88, 91], 0.07)), "loop": False}
    S["chat_message"] = {"file": write("sfx/chat_message.wav", blips([81, 86], 0.05, 0.03, .125)), "loop": False}
    S["build_complete"] = {"file": write("sfx/build_complete.wav", blips([72, 76, 79, 84], 0.08)), "loop": False}
    S["research_complete"] = {"file": write("sfx/research_complete.wav", blips([72, 79, 76, 84, 88], 0.07, .01, .125)), "loop": False}
    S["transfer_scheduled"] = {"file": write("sfx/transfer_scheduled.wav", blips([67, 74, 79], 0.05, 0, .5, .15)), "loop": False}
    n = int(1.0 * SR)
    x = echo(sine(1320, n) * expd(n, 7) * .8 + pulse(1320, n, .5) * expd(n, 30) * .3, .18, .5, .7)
    S["scan_ping"] = {"file": write("sfx/scan_ping.wav", x), "loop": False, "use": "sonar-style scanner sweep"}
    n = int(1.4 * SR); tt = np.arange(n) / SR
    x = pulse(110 * 2 ** (tt * 2.5), n, .25) * adsr(n, .02, .1, .8, .3) * .7 + lp(lfsr_noise(n, 2000), 800) * .3 * tt
    S["capture"] = {"file": write("sfx/capture.wav", x), "loop": False, "use": "planet ownership changed to you"}
    return S


# ================================================================ mood: ambient synth
def _comb(x, D, g):
    y = x.copy()
    for i in range(D, len(x), D):
        j = min(i + D, len(x))
        y[i:j] += g * y[i - D:j - D]
    return y
def _allpass(x, D, g):
    y = -g * x
    y[D:] += x[:-D]
    for i in range(D, len(x), D):
        j = min(i + D, len(x))
        y[i:j] += g * y[i - D:j - D]
    return y
def reverb(x, mix=0.35, size=1.0, damp=3500):
    """Schroeder reverb: 4 parallel combs → 2 allpasses, damped, mixed with dry."""
    wet = sum(_comb(x, int(d * size * SR), g) for d, g in ((.0297, .77), (.0371, .75), (.0411, .73), (.0437, .71)))
    wet = _allpass(_allpass(wet, int(.005 * SR), .7), int(.0017 * SR), .7)
    wet = lp(wet, damp) * 0.25
    return x * (1 - mix) + wet * mix * 2


def a_pad(chord, dur, vel=1.0, cut=900, wobble=0.6):
    n = int(dur * SR); tt = np.arange(n) / SR
    x = sum(saw(midi(c) * (1 + d), n) for c in chord for d in (-.004, 0, .005))
    lo, hi = lp(x, cut * 0.5), lp(x, cut * 1.6)
    k = 0.5 + 0.5 * np.sin(2 * np.pi * tt * wobble / max(dur, 1) + 1)
    return (lo * (1 - k) + hi * k) * adsr(n, dur * .3, .2, 1, dur * .35) * vel * 0.045
def a_sub(f, dur, vel=1.0):
    n = int(dur * SR)
    return (sine(f, n) + 0.15 * sine(2 * f, n)) * adsr(n, .08, .2, .85, .3) * vel * 0.4
def a_bell(f, dur, vel=1.0):
    n = int(max(dur, 1.2) * SR)
    x = sine(f, n) + 0.35 * sine(f * 2.76, n) * expd(n, 6) + 0.2 * sine(f * 5.4, n) * expd(n, 12)
    return x * expd(n, 2.6) * np.clip(np.arange(n) / (0.004 * SR), 0, 1) * vel * 0.16
def a_pluck(f, dur, vel=1.0, cut=2200):
    n = int(dur * SR)
    return lp(saw(f, n) + saw(f * 1.006, n), cut) * expd(n, 7) * vel * 0.12
def a_kick(vel=1.0):
    n = int(.35 * SR); tt = np.arange(n) / SR
    return sine(45 + 70 * np.exp(-tt * 30), n) * expd(n, 9) * vel * 0.7
def a_shaker(vel=1.0):
    n = int(.06 * SR)
    return hp(noise(n), 5000) * expd(n, 60) * vel * 0.12
def a_tom(f, vel=1.0):
    n = int(.4 * SR); tt = np.arange(n) / SR
    return (sine(f * (1 + .5 * np.exp(-tt * 25)), n) * .8 + lp(noise(n), 900) * .3) * expd(n, 10) * vel * .5


def compose_ambient(seed, root, scale, prog, bpm, sections, cut=900, kick=False, shaker=False, toms=False,
                    pulse_bass=False, bells=True, sparkles=True, plucks=False):
    rnd = np.random.default_rng(seed)
    S = Song(bpm, sum(x[1] for x in sections))
    scale_notes = [root + 12 + o * 12 + q for o in (0, 1) for q in scale]
    motifs, prev, bar = {}, root + 19, 0
    for name, nb, inten in sections:
        for k in range(nb):
            ch = chord_of(root, scale, prog[bar % len(prog)])
            S.add(a_pad(ch, S.beat * 4.6, 0.7 + 0.5 * inten, cut * (0.7 + 0.6 * inten)), bar, 0)
            if pulse_bass and inten > .3:
                for b in range(8):
                    S.add(a_pluck(midi(ch[0] - 24), S.beat * .45, .9 if b % 2 == 0 else .6, 500 + 900 * inten) * 2.2, bar, b * .5)
            S.add(a_sub(midi(ch[0] - 24), S.beat * 4.2, 0.6 + 0.4 * inten), bar, 0)
            if sparkles and inten > .2:
                for j in range(rnd.integers(2, 5)):
                    nn = ch[rnd.integers(3)] + 24 + 12 * rnd.integers(0, 2)
                    S.add(a_bell(midi(nn), 1.0, 0.25 + .2 * rnd.random()), bar, rnd.integers(0, 8) * .5)
            if plucks and inten > .3:
                for b in range(16):
                    S.add(a_pluck(midi((ch + [ch[0] + 12])[b % 4] + 12), S.beat * .3, .5 + .3 * inten), bar, b * .25)
            if kick and inten > .4:
                for b in (0, 2) if inten < .8 else (0, 1, 2, 3):
                    S.add(a_kick(), bar, b)
            if shaker and inten > .3:
                for h in range(8):
                    S.add(a_shaker(1.0 if h % 2 else .6), bar, h * .5)
            if toms and inten > .6 and k % 2 == 1:
                for f_, b in ((140, 3), (110, 3.25), (85, 3.5)):
                    S.add(a_tom(f_), bar, b)
            if bells and inten >= .35:
                key = (name, k % 2)
                if key not in motifs:
                    pat = [PATTERNS[2], PATTERNS[4], PATTERNS[0]][rnd.integers(3)]
                    motifs[key], prev = melody_bar(rnd, ch, scale_notes, pat, prev)
                for b, d, nn in motifs[key]:
                    if b == int(b) and (nn % 12) not in [c % 12 for c in ch]:
                        nn = min([c + 12 + o for c in ch for o in (0, 12)], key=lambda m: abs(m - nn))
                    S.add(a_bell(midi(nn), d * S.beat, 0.8 + .3 * inten), bar, b)
            bar += 1
    return S


def music_ambient():
    M = {}
    s = compose_ambient(5, 57, MIN, [0, 5, 2, 6], 80,
                        [("intro", 4, .25), ("a", 8, .55), ("b", 8, .75), ("a", 8, .55), ("out", 4, .3)],
                        kick=True, shaker=True)
    M["title"] = {"file": write("music/title.wav", s.render(lambda b: reverb(b, .45, 1.3)), rms=0.15), "loop": True,
                  "seconds": round(s.length, 1), "use": "main menu / lobby"}
    s = compose_ambient(9, 50, MIN, [0, 5, 3, 4, 0, 6, 5, 4], 64,
                        [("drift", 8, .2), ("glow", 8, .4), ("drift", 8, .25), ("glow", 8, .45)], cut=700)
    M["orbit"] = {"file": write("music/orbit.wav", s.render(lambda b: reverb(b, .55, 1.5, 2500)), rms=0.13), "loop": True,
                  "seconds": round(s.length, 1), "use": "in-game map, default bed (keep volume ~0.5)"}
    s = compose_ambient(13, 52, MIN, [0, 5, 6, 0, 0, 3, 6, 4], 112,
                        [("a", 8, .7), ("b", 8, .95), ("a", 8, .75), ("break", 4, .5)], cut=1300,
                        kick=True, shaker=True, toms=True, pulse_bass=True, plucks=True, bells=False)
    M["battle"] = {"file": write("music/battle.wav", s.render(lambda b: reverb(b, .3, 1.0)), rms=0.16), "loop": True,
                   "seconds": round(s.length, 1), "use": "crossfade in when combat is happening on screen"}

    def swell(chords, melody, name, use, bpm=72):
        beat = 60 / bpm; L = len(chords) * 2 * beat + 3.5
        buf = np.zeros(int(L * SR))
        for i, ch in enumerate(chords):
            place(buf, a_pad(ch, 2.4 * beat if i < len(chords) - 1 else 5, 1.2, 1400), i * 2 * beat)
            place(buf, a_sub(midi(ch[0] - 24), 2.2 * beat if i < len(chords) - 1 else 4), i * 2 * beat)
        for nn, at in melody:
            place(buf, a_bell(midi(nn), 1.5, 1.0), at * beat)
        M[name] = {"file": write(f"music/{name}.wav", reverb(buf, .5, 1.4)), "loop": False,
                   "seconds": round(L, 1), "use": use}
    swell([[60, 64, 67], [65, 69, 72], [67, 71, 74], [72, 76, 79]], [(72, 0), (76, 1), (79, 2), (84, 4), (88, 6)],
          "victory", "game won")
    swell([[57, 60, 64], [53, 57, 60], [52, 55, 59], [45, 48, 52]], [(76, 0), (74, 2), (72, 4), (69, 6)],
          "defeat", "game lost")
    return M


def a_blips(notes, d=0.08, gap=0.0, tail=0.35):
    L = len(notes) * (d + gap) + tail + 0.5; buf = np.zeros(int(L * SR))
    for i, nn in enumerate(notes):
        m = int((d + 0.25) * SR)
        x = sine(midi(nn), m) + 0.25 * sine(midi(nn) * 2, m) * expd(m, 20)
        place(buf, x * expd(m, 14) * np.clip(np.arange(m) / (0.003 * SR), 0, 1), i * (d + gap))
    out = reverb(buf, .35, .8)[:int((len(notes) * (d + gap) + tail) * SR)]
    f = min(len(out), int(.1 * SR)); out[-f:] *= np.linspace(1, 0, f)  # fade the cut reverb tail
    return out


def sfx_ambient():
    S = {}
    n = int(2.0 * SR); ext = n + SR
    base = lp(noise(ext), 350) * 1.4 + lp(noise(ext), 1200) * .25 + sine(42, ext) * .25
    loop = base[:n].copy(); xf = int(.4 * SR); w = np.linspace(0, 1, xf)
    loop[:xf] = loop[:xf] * w + base[n:n + xf] * (1 - w)
    S["engine_burn_loop"] = {"file": write("sfx/engine_burn_loop.wav", loop, .7), "loop": True}
    n = int(1.2 * SR); tt = np.arange(n) / SR
    raw = noise(n); k = np.clip(tt * 2, 0, 1)
    x = lp(raw, 200) * (1 - k) * 2 + lp(raw, 900) * k + sine(30 + 25 * k, n) * .4
    S["engine_burn_start"] = {"file": write("sfx/engine_burn_start.wav", x * adsr(n, .25, .1, 1, .3)), "loop": False}
    n = int(.6 * SR); tt = np.arange(n) / SR
    x = sine(1500 * np.exp(-tt * 9) + 260, n) * expd(n, 7) + saw(1500 * np.exp(-tt * 9) + 262, n) * expd(n, 14) * .2
    S["energy_weapon"] = {"file": write("sfx/energy_weapon.wav", reverb(lp(x, 4000), .3, .7)), "loop": False}
    n = int(.8 * SR); tt = np.arange(n) / SR
    x = sum(sine(f * (1 + .004 * np.sin(2 * np.pi * 6 * tt)), n) for f in (330, 331.5, 495)) + lp(noise(n), 3000) * .15
    S["energy_beam"] = {"file": write("sfx/energy_beam.wav", reverb(x * adsr(n, .03, .1, .8, .25), .3, .7)), "loop": False}
    n = int(.5 * SR); tt = np.arange(n) / SR
    x = sine(40 + 110 * np.exp(-tt * 28), n) * expd(n, 10) * 1.2 + lp(noise(n), 2200) * expd(n, 22)
    S["kinetic_cannon"] = {"file": write("sfx/kinetic_cannon.wav", reverb(x, .25, .6)), "loop": False}
    n = int(.9 * SR); tt = np.arange(n) / SR
    x = sine(30 + 80 * np.exp(-tt * 18), n) * expd(n, 6) * 1.3 + lp(noise(n), 1200) * expd(n, 9)
    S["kinetic_cannon_heavy"] = {"file": write("sfx/kinetic_cannon_heavy.wav", reverb(x, .3, .9)), "loop": False}
    def boom(d, cut, k, sub):
        n = int(d * SR); tt = np.arange(n) / SR
        raw = noise(n); kk = np.exp(-tt * 3)
        x = lp(raw, cut) * kk + lp(raw, cut / 4) * (1 - kk)
        return x * expd(n, k) + sine(sub * np.exp(-tt * 2) + 18, n) * expd(n, k * 1.2) * .8
    S["explosion_small"] = {"file": write("sfx/explosion_small.wav", reverb(boom(1.0, 1800, 5, 80), .35, 1.0)), "loop": False}
    S["explosion_large"] = {"file": write("sfx/explosion_large.wav", reverb(boom(2.6, 1100, 2.2, 55), .45, 1.4)), "loop": False}
    S["asteroid_impact"] = {"file": write("sfx/asteroid_impact.wav", reverb(boom(3.6, 700, 1.4, 40), .5, 1.6)), "loop": False}
    n = int(.25 * SR)
    S["hit_kinetic"] = {"file": write("sfx/hit_kinetic.wav", reverb((lp(noise(n), 2500) + sine(180, n) * .5) * expd(n, 30), .25, .6)), "loop": False}
    n = int(.6 * SR); tt = np.arange(n) / SR
    x = sum(sine(f * (1 + .02 * np.sin(2 * np.pi * 25 * tt)), n) for f in (880, 1320)) * expd(n, 8)
    S["shield_hit"] = {"file": write("sfx/shield_hit.wav", reverb(x, .4, .8)), "loop": False}
    S["alert_ping"] = {"file": write("sfx/alert_ping.wav", a_blips([84, 91], .14, .02, .9)), "loop": False, "use": "generic attention"}
    n = int(1.6 * SR); tt = np.arange(n) / SR
    f = np.where((tt * 2.5) % 1 < .5, 587, 466)
    x = lp(saw(f, n) + saw(f * 1.005, n), 1600) * adsr(n, .02, .1, .9, .2) * .6
    S["alert_enemy"] = {"file": write("sfx/alert_enemy.wav", reverb(x, .35, 1.0)), "loop": False, "use": "hostile fleet detected / under attack"}
    S["ui_click"] = {"file": write("sfx/ui_click.wav", a_blips([96], .02, 0, .08), .5), "loop": False}
    S["ui_tap"] = {"file": write("sfx/ui_tap.wav", a_blips([88], .02, 0, .1), .5), "loop": False}
    S["ui_confirm"] = {"file": write("sfx/ui_confirm.wav", a_blips([79, 86], .07, 0, .3)), "loop": False}
    S["ui_back"] = {"file": write("sfx/ui_back.wav", a_blips([86, 79], .07, 0, .3)), "loop": False}
    S["ui_error"] = {"file": write("sfx/ui_error.wav", a_blips([62, 61], .12, .03, .3)), "loop": False}
    S["notification"] = {"file": write("sfx/notification.wav", a_blips([84, 88, 91], .1, 0, .8)), "loop": False}
    S["chat_message"] = {"file": write("sfx/chat_message.wav", a_blips([86, 91], .07, .02, .5)), "loop": False}
    S["build_complete"] = {"file": write("sfx/build_complete.wav", a_blips([72, 76, 79, 84], .1, 0, 1.0)), "loop": False}
    S["research_complete"] = {"file": write("sfx/research_complete.wav", a_blips([72, 79, 76, 84, 88], .09, 0, 1.0)), "loop": False}
    S["transfer_scheduled"] = {"file": write("sfx/transfer_scheduled.wav", a_blips([67, 74, 79], .07, 0, .5)), "loop": False}
    n = int(1.6 * SR)
    S["scan_ping"] = {"file": write("sfx/scan_ping.wav", reverb(sine(1175, n) * expd(n, 5), .55, 1.6)), "loop": False,
                      "use": "sonar-style scanner sweep"}
    n = int(2.0 * SR)
    x = a_pad([57, 64, 69], 2.0, 3, 2200); place(x, a_bell(midi(81), 1.6, .8), 0.1)
    S["capture"] = {"file": write("sfx/capture.wav", reverb(x, .45, 1.2)),
                    "loop": False, "use": "planet ownership changed to you"}
    return S


def menu_sfx(blips):
    """Menu/navigation sounds, built from the mood's blip voice."""
    M = {}
    def add(name, x, use, peak=.8):
        M[name] = {"file": write(f"sfx/{name}.wav", x, peak), "loop": False, "use": use}
    add("menu_open", blips([72, 79, 84], .05, 0, .25), "a menu / panel / sheet slides open")
    add("menu_close", blips([84, 79, 72], .05, 0, .2), "a menu / panel closes")
    add("menu_select", blips([91], .03, 0, .1), "focus / highlight a list item", .55)
    add("menu_tab", blips([86, 86], .03, .03, .1), "switch tab", .6)
    add("toggle_on", blips([79, 91], .04, 0, .12), "switch turned on", .7)
    add("toggle_off", blips([91, 79], .04, 0, .12), "switch turned off", .7)
    add("slider_tick", blips([100], .012, 0, .04), "slider / time-scrubber detent (rate-limit ~20/s)", .4)
    add("start_game", blips([60, 67, 72, 76, 79, 84], .09, 0, 1.2), "join / start a match")
    add("lobby_join", blips([76, 81], .08, .04, .5), "another player joined the lobby")
    add("purchase", blips([84, 88, 96], .05, 0, .4), "spend resources (queue build / upgrade)")
    return M


def main():
    mood = "ambient"
    if "--mood" in sys.argv:
        mood = sys.argv[sys.argv.index("--mood") + 1]
    if mood not in ("ambient", "chiptune"):
        sys.exit(f"unknown mood {mood!r}")
    import shutil
    shutil.rmtree(OUT, ignore_errors=True)
    if mood == "ambient":
        M, S = music_ambient(), sfx_ambient()
        S.update(menu_sfx(a_blips))
    else:
        M, S = music(), sfx()
        S.update(menu_sfx(lambda notes, d=0.06, gap=0.0, tail=0.12: chip_blips(notes, d, gap, 0.25, tail)))
    A = {"mood": mood, "format": "WAV PCM 16-bit mono 22050 Hz", "music": M, "sfx": S}
    mp = os.path.join(ROOT, "manifest.json")
    man = json.load(open(mp)) if os.path.exists(mp) else {}
    man["audio"] = A
    json.dump(man, open(mp, "w"), indent=1)
    reel(A)
    print("ok")


def reel(A):
    """preview/audio_reel.wav: 8 s of each music loop, then every SFX (review only, not shipped)."""
    def load(rel):
        with wave.open(os.path.join(ROOT, rel)) as w:
            return np.frombuffer(w.readframes(w.getnframes()), "<i2") / 32768
    parts = []
    for k in ("title", "orbit", "battle"):
        x = load(A["music"][k]["file"])[:8 * SR].copy(); x[-SR:] *= np.linspace(1, 0, SR); parts.append(x)
    for k in ("victory", "defeat"):
        parts.append(load(A["music"][k]["file"]))
    for k, v in A["sfx"].items():
        x = load(v["file"])
        parts.append(np.tile(x, 2) if v.get("loop") else x); parts.append(np.zeros(int(.25 * SR)))
    os.makedirs(os.path.join(ROOT, "preview"), exist_ok=True)
    with wave.open(os.path.join(ROOT, "preview", "audio_reel.wav"), "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(SR)
        w.writeframes((np.concatenate(parts) * 32767).astype("<i2").tobytes())


if __name__ == "__main__":
    main()
