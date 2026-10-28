// mq_js_bundle.js assigns this global from "use strict" code; declare it so the bundle loads fully
var register_plugin;

// Phones report devicePixelRatio ~3: rendering that many pixels costs more than it shows. Draw at
// most 2 device pixels per CSS pixel (miniquad reads this for its high_dpi canvas size).
(function () {
    const real = window.devicePixelRatio || 1;
    if (real > 2) {
        try {
            Object.defineProperty(window, "devicePixelRatio", { get: () => 2, configurable: true });
        } catch (e) {}
    }
    // WebGL defaults to 4x multisampling and a transparent canvas; the Android app uses neither
    // (and at 2x the pixels are small enough without MSAA). Both cost a lot of fill on phones.
    const getContext = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function (type, attrs) {
        if (this.id === "glcanvas" && /webgl/.test(type)) {
            attrs = Object.assign({ antialias: false, alpha: false, powerPreference: "high-performance" }, attrs || {});
        }
        return getContext.call(this, type, attrs);
    };
})();
