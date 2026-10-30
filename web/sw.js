// Service worker: shows the server's push notifications (a battle starting, enemies on their way)
// while the game isn't open (crates/server/src/push.rs). Registered by hohmann.js.
"use strict";
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (ev) => ev.waitUntil(self.clients.claim()));

self.addEventListener("push", (ev) => {
    let n = {};
    try { n = ev.data ? ev.data.json() : {}; } catch (e) {}
    ev.waitUntil(self.clients.matchAll({ type: "window", includeUncontrolled: true }).then((wins) => {
        // the game is on screen and shows the news itself
        if (wins.some((w) => w.visibilityState === "visible" && w.focused)) return;
        return self.registration.showNotification(n.title || "Hohmann's Reach", {
            body: n.body || "",
            tag: n.tag || "battle",
            icon: "icon.png",
            badge: "icon.png",
            renotify: true,
        });
    }));
});

self.addEventListener("notificationclick", (ev) => {
    ev.notification.close();
    ev.waitUntil(self.clients.matchAll({ type: "window", includeUncontrolled: true }).then((wins) => {
        for (const w of wins) {
            if ("focus" in w) return w.focus();
        }
        return self.clients.openWindow("./");
    }));
});
