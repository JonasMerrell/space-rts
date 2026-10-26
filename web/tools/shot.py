#!/usr/bin/env python3
"""Screenshot the browser build in headless Chromium (system chromium + playwright).
usage: shot.py URL OUT.png [WxH] [--mobile] [--wait S] [--tap X,Y ...] [--type TEXT] [--key KEY]
Steps run in the order given, each followed by a short wait; --shot NAME saves an extra screenshot."""
import sys, os
from playwright.sync_api import sync_playwright

args = sys.argv[1:]
url, out = args[0], args[1]
rest = args[2:]
w, h = 412, 880
if rest and "x" in rest[0] and not rest[0].startswith("--"):
    w, h = map(int, rest.pop(0).split("x"))
mobile = "--mobile" in rest
exe = os.environ.get("CHROMIUM", "/usr/bin/chromium")
with sync_playwright() as pw:
    b = pw.chromium.launch(executable_path=exe, args=["--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist", "--ignore-certificate-errors"] if os.environ.get("INSECURE") else ["--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"])
    ctx = b.new_context(viewport={"width": w, "height": h}, is_mobile=mobile, has_touch=mobile, device_scale_factor=1)
    p = ctx.new_page()
    p.on("console", lambda m: print("console:", m.type, m.text[:300]) if "Missed function" not in m.text and "function in gl.js" not in m.text else None)
    p.on("pageerror", lambda e: print("pageerror:", e))
    if os.environ.get("REQLOG"):
        p.on("requestfinished", lambda r: print("req:", r.url[-60:]))
        p.on("requestfailed", lambda r: print("failed:", r.url[-60:], r.failure))
    p.goto(url)
    i = 0
    wait = 8.0
    while i < len(rest):
        a = rest[i]
        if a == "--wait":
            wait = float(rest[i + 1]); i += 1
            p.wait_for_timeout(wait * 1000)
        elif a == "--tap":
            x, y = map(float, rest[i + 1].split(",")); i += 1
            if mobile:
                p.touchscreen.tap(x, y)
            else:
                p.mouse.move(x, y); p.mouse.down(); p.wait_for_timeout(120); p.mouse.up()
            p.wait_for_timeout(600)
        elif a == "--type":
            p.keyboard.type(rest[i + 1], delay=40); i += 1
            p.wait_for_timeout(300)
        elif a == "--key":
            p.keyboard.press(rest[i + 1]); i += 1
            p.wait_for_timeout(300)
        elif a == "--shot":
            p.screenshot(path=rest[i + 1]); i += 1
        i += 1
    if "--wait" not in rest:
        p.wait_for_timeout(wait * 1000)
    if os.environ.get("GLINFO"):
        print("gl:", p.evaluate("() => [typeof gl, gl && gl.getParameter(gl.VERSION), gl && gl.getParameter(gl.RENDERER), canvas.width, canvas.height]"))
    p.screenshot(path=out)
    b.close()
