#!/usr/bin/env python3
"""CPU-profile the browser build: open URL (persistent profile in USERDATA), optionally tap X,Y
first, then record the JS/wasm profile for SECONDS and print the top self-time functions plus the
frame rate. usage: profile.py URL [SECONDS] [--tap X,Y ...] [--size WxH] [--dpr N]"""
import sys, os, json, collections
from playwright.sync_api import sync_playwright

args = sys.argv[1:]
url = args.pop(0)
secs = float(args.pop(0)) if args and not args[0].startswith("--") else 8.0
taps, w, h, dpr = [], 412, 880, 1.0
while args:
    a = args.pop(0)
    if a == "--tap":
        taps.append(tuple(map(float, args.pop(0).split(","))))
    elif a == "--size":
        w, h = map(int, args.pop(0).split("x"))
    elif a == "--dpr":
        dpr = float(args.pop(0))
flags = ["--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"]
with sync_playwright() as pw:
    ctx = pw.chromium.launch_persistent_context(os.environ["USERDATA"], executable_path="/usr/bin/chromium", args=flags,
                                                viewport={"width": w, "height": h}, device_scale_factor=dpr)
    p = ctx.new_page()
    p.goto(url)
    p.wait_for_selector("#loading", state="detached", timeout=120000)
    p.wait_for_timeout(3000)
    for x, y in taps:
        p.mouse.move(x, y); p.mouse.down(); p.wait_for_timeout(150); p.mouse.up(); p.wait_for_timeout(2500)
    p.wait_for_timeout(3000)
    cdp = ctx.new_cdp_session(p)
    cdp.send("Profiler.enable")
    cdp.send("Profiler.setSamplingInterval", {"interval": 200})
    cdp.send("Profiler.start")
    fps = p.evaluate(f"() => new Promise(r => {{ let n = 0, worst = 0, last = performance.now(); const t0 = last; function f() {{ const t = performance.now(); worst = Math.max(worst, t - last); last = t; n++; if (t - t0 < {secs * 1000}) requestAnimationFrame(f); else r([n / {secs}, worst]); }} requestAnimationFrame(f); }})")
    prof = cdp.send("Profiler.stop")["profile"]
    if os.environ.get("SHOT"):
        p.screenshot(path=os.environ["SHOT"])
    ctx.close()
nodes = {n["id"]: n for n in prof["nodes"]}
dt = collections.Counter()
for sid, d in zip(prof["samples"], prof["timeDeltas"]):
    dt[sid] += d
self_t = collections.Counter()
for nid, t in dt.items():
    cf = nodes[nid]["callFrame"]
    self_t[cf["functionName"] or "(anon) " + cf["url"][-30:]] += t
total = sum(self_t.values())
# inclusive time of functions whose name contains any INCL substring (comma separated)
parent = {}
for n in prof["nodes"]:
    for c in n.get("children", []):
        parent[c] = n["id"]
incl = collections.Counter()
keys = [k for k in os.environ.get("INCL", "").split(",") if k]
for nid, t in dt.items():
    seen, cur = set(), nid
    while cur is not None:
        name = nodes[cur]["callFrame"]["functionName"]
        for k in keys:
            if k in name and k not in seen:
                seen.add(k); incl[k] += t
        cur = parent.get(cur)
print(f"fps {fps[0]:.1f}, worst frame {fps[1]:.0f} ms, profile {total/1e6:.1f} s")
for k in keys:
    print(f"incl {100*incl[k]/total:5.1f}%  {k}")
for name, t in self_t.most_common(int(os.environ.get("TOP", "25"))):
    print(f"{100*t/total:5.1f}%  {name[:110]}")
