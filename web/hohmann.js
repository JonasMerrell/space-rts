// Browser glue for the Hohmann's Reach wasm client (miniquad plugin "hohmann"):
// a WebSocket to the game server, localStorage, and notifications while the tab is hidden.
// Loaded after mq_js_bundle.js. All strings cross the boundary as UTF-8 (ptr, len) in wasm memory.
"use strict";
(function () {
    const enc = new TextEncoder();
    const dec = new TextDecoder();
    const mem = () => new Uint8Array(wasm_memory.buffer);
    const str = (ptr, len) => dec.decode(mem().subarray(ptr, ptr + len));
    // a value handed to Rust in two calls: *_len() then *_take(dst)
    let staged = null;
    function stage(s) {
        staged = s == null ? null : enc.encode(s);
        return staged == null ? -1 : staged.length;
    }
    function take(dst) {
        if (staged) mem().set(staged, dst);
        staged = null;
    }

    // ---- WebSocket: one live socket at a time; a handle number tells stale ones apart
    let ws = null, wsId = 0, wsState = 2, inbox = [];
    // notifications while hidden: we keep the server connection alive and poll for events ourselves
    let notifyOn = false, since = 0, pollTimer = null;

    function defaultUrl() {
        return (location.protocol === "https:" ? "wss://" : "ws://") + location.host + "/ws";
    }

    function open(url) {
        close();
        wsId += 1;
        const id = wsId;
        wsState = 0;
        inbox = [];
        try {
            ws = new WebSocket(url || defaultUrl());
        } catch (e) {
            wsState = 2;
            return id;
        }
        ws.onopen = () => { if (id === wsId) wsState = 1; };
        ws.onclose = () => { if (id === wsId) wsState = 2; };
        ws.onerror = () => { if (id === wsId) wsState = 2; };
        ws.onmessage = (ev) => {
            if (id !== wsId || typeof ev.data !== "string") return;
            const m = ev.data;
            if (m.startsWith('{"Events"')) {
                // answers to our own background polls; the game ignores them anyway
                if (document.hidden) showEvents(m);
                return;
            }
            if (document.hidden && m.startsWith('{"State"')) {
                // the game isn't drawing; keep only the newest full snapshot
                inbox = inbox.filter((x) => !x.startsWith('{"State"'));
            }
            inbox.push(m);
        };
        return id;
    }

    function close() {
        if (ws) {
            try { ws.close(); } catch (e) {}
        }
        ws = null;
        wsState = 2;
        inbox = [];
    }

    function sendRaw(s) {
        if (ws && wsState === 1) ws.send(s);
    }

    // ---- notifications (the Android app's NotifyJob, for a hidden browser tab)
    function worth(e) {
        if (!e) return false;
        if (e.kind === "Info") return e.to == null;
        if (e.kind !== "Good" && e.kind !== "Bad" && e.kind !== "Battle") return false;
        return e.title !== "Retreating";
    }

    function showEvents(line) {
        let rows;
        try { rows = JSON.parse(line).Events; } catch (e) { return; }
        if (!Array.isArray(rows)) return;
        for (const [code, when, e] of rows) {
            if (typeof when === "number" && when > since) since = when;
            if (!worth(e) || !("Notification" in window) || Notification.permission !== "granted") continue;
            try {
                const n = new Notification(e.title || "Hohmann's Reach", {
                    body: e.body || "",
                    tag: code + "#" + e.seq,
                    icon: "icon.png",
                });
                n.onclick = () => { window.focus(); n.close(); };
            } catch (err) {}
        }
    }

    function poll() {
        if (!notifyOn || !document.hidden) return;
        // keeps the connection from idling out too (the game's own pings stop while hidden)
        sendRaw('"Ping"\n');
        sendRaw(JSON.stringify({ Events: { since: since } }) + "\n");
    }

    document.addEventListener("visibilitychange", () => {
        if (document.hidden) {
            since = Date.now() / 1000;
            if (notifyOn && !pollTimer) pollTimer = setInterval(poll, 30000);
        } else if (pollTimer) {
            clearInterval(pollTimer);
            pollTimer = null;
        }
    });

    function askPermission() {
        if (!("Notification" in window) || Notification.permission !== "default") return;
        // browsers only allow the prompt from a user gesture
        const once = () => {
            window.removeEventListener("pointerdown", once, true);
            window.removeEventListener("keydown", once, true);
            try { Notification.requestPermission(); } catch (e) {}
        };
        window.addEventListener("pointerdown", once, true);
        window.addEventListener("keydown", once, true);
    }

    // ---- on-screen keyboard. Phones only open their keyboard for a focused HTML input, and only
    // from inside a user gesture, so: the game reports its text fields every frame; a tap that
    // ends on one focuses a hidden input right away (in the touchend handler); what's typed there
    // is replayed into the game as key events. A sentinel character lets us see backspaces.
    let fields = [], fieldFocused = -1, focusedAt = 0;
    const SENTINEL = "\u200b";
    const kbd = document.createElement("input");
    kbd.id = "kbd";
    kbd.type = "text";
    kbd.autocomplete = "off";
    kbd.autocapitalize = "off";
    kbd.spellcheck = false;
    kbd.setAttribute("autocorrect", "off");
    kbd.setAttribute("aria-hidden", "true");
    kbd.value = SENTINEL;
    document.body.appendChild(kbd);

    function inField(x, y) {
        for (let i = 0; i + 3 < fields.length; i += 4) {
            if (x >= fields[i] && x <= fields[i] + fields[i + 2] && y >= fields[i + 1] && y <= fields[i + 1] + fields[i + 3]) return true;
        }
        return false;
    }

    function pressKey(code) {
        wasm_exports.key_down(code, 0, false);
        wasm_exports.key_up(code, 0);
    }

    function flushInput() {
        const v = kbd.value;
        if (v.startsWith(SENTINEL)) {
            for (const ch of v.slice(SENTINEL.length)) {
                if (ch === "\n") pressKey(257);
                else wasm_exports.key_press(ch.codePointAt(0));
            }
        } else {
            // the sentinel was deleted: one backspace per missing character
            const lost = Math.max(1, SENTINEL.length - v.length);
            for (let i = 0; i < lost; i++) pressKey(259);
            for (const ch of v.replace(SENTINEL, "")) wasm_exports.key_press(ch.codePointAt(0));
        }
        kbd.value = SENTINEL;
        kbd.setSelectionRange(SENTINEL.length, SENTINEL.length);
    }

    let composing = false;
    kbd.addEventListener("compositionstart", () => { composing = true; });
    kbd.addEventListener("compositionend", () => { composing = false; flushInput(); });
    kbd.addEventListener("input", () => { if (!composing) flushInput(); });
    kbd.addEventListener("keydown", (e) => {
        // keys that don't produce text; backspace arrives through the input event
        const codes = { Enter: 257, Tab: 258, Escape: 256 };
        if (codes[e.key]) {
            e.preventDefault();
            const shift = e.shiftKey ? SAPP_MODIFIER_SHIFT : 0;
            wasm_exports.key_down(codes[e.key], shift, false);
            wasm_exports.key_up(codes[e.key], shift);
        }
    });

    function onTapEnd(e) {
        const t = e.changedTouches && e.changedTouches[0];
        if (!t) return;
        const r = canvas.getBoundingClientRect();
        if (inField(t.clientX - r.left, t.clientY - r.top)) {
            kbd.value = SENTINEL;
            kbd.focus({ preventScroll: true });
            focusedAt = performance.now();
        } else if (document.activeElement === kbd) {
            kbd.blur();
        }
    }
    canvas.addEventListener("touchend", onTapEnd, { passive: true });

    // ---- storage
    function lsGet(k) {
        try { return localStorage.getItem(k); } catch (e) { return null; }
    }
    function lsSet(k, v) {
        try { localStorage.setItem(k, v); } catch (e) {}
    }
    function lsRemove(k) {
        try { localStorage.removeItem(k); } catch (e) {}
    }

    // ---- loading screen: count the game's files as they arrive
    let loadObserver = null;
    (function loadingProgress() {
        const box = document.getElementById("loading");
        const text = document.getElementById("loading-text");
        if (!box || !text || !("PerformanceObserver" in window)) return;
        let seen = 0;
        try {
            loadObserver = new PerformanceObserver((list) => {
                for (const e of list.getEntries()) if (e.name.includes("/assets/")) seen++;
                text.textContent = "Loading game art… " + seen + " files";
            });
            loadObserver.observe({ type: "resource", buffered: true });
        } catch (e) {}
    })();

    function register_plugin(importObject) {
        const env = importObject.env;
        env.hr_ws_open = (ptr, len) => open(str(ptr, len));
        env.hr_ws_state = (id) => (id === wsId ? wsState : 2);
        env.hr_ws_send = (id, ptr, len) => { if (id === wsId) sendRaw(str(ptr, len)); };
        env.hr_ws_recv_len = (id) => (id === wsId && inbox.length ? stage(inbox.shift()) : -1);
        env.hr_ws_close = (id) => { if (id === wsId) close(); };
        env.hr_take = (dst) => take(dst);
        env.hr_store_get_len = (ptr, len) => stage(lsGet(str(ptr, len)));
        env.hr_store_set = (kp, kl, vp, vl) => lsSet(str(kp, kl), str(vp, vl));
        env.hr_store_remove = (ptr, len) => lsRemove(str(ptr, len));
        env.hr_page_host_len = () => stage(location.host);
        env.hr_loaded = () => {
            const l = document.getElementById("loading");
            if (!l) return;
            if (loadObserver) loadObserver.disconnect();
            l.classList.add("done");
            setTimeout(() => l.remove(), 400);
        };
        env.hr_text_fields = (ptr, n, focused) => {
            fields = Array.from(new Float32Array(wasm_memory.buffer, ptr, n * 4));
            fieldFocused = focused;
            // the game dropped focus (submitted, or the field went away): put the keyboard away.
            // Not right after our tap: the game may only see it a frame or two later.
            if (focused < 0 && document.activeElement === kbd && performance.now() - focusedAt > 600) kbd.blur();
        };
        env.hr_notify = (on) => {
            notifyOn = on !== 0;
            if (notifyOn) askPermission();
        };
    }

    miniquad_add_plugin({ register_plugin, version: 1, name: "hohmann" });
})();
