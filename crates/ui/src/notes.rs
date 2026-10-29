//! Patch notes: CHANGELOG.md (repo root) baked into the app, a scrollable full-screen sheet opened
//! from the main menu / game menu, and a one-time "What's new" after the app version changes.

use crate::cel;
use crate::input::Input;
use crate::theme::*;
use crate::widgets::{self, Style};
use macroquad::prelude::*;
use std::cell::Cell;
use std::sync::OnceLock;

/// The changelog as shipped with this build (Android, web and desktop alike).
pub const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub date: String,
    pub items: Vec<String>,
}

/// `## 0.8.8 — 2026-10-29` headers (an ASCII ` - ` works too) followed by `- ` bullets; indented
/// lines continue the previous bullet. Everything before the first header is ignored.
pub fn parse(text: &str) -> Vec<Release> {
    let mut out: Vec<Release> = vec![];
    for line in text.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            let (version, date) = h.split_once(" — ").or_else(|| h.split_once(" - ")).unwrap_or((h, ""));
            out.push(Release { version: version.trim().trim_start_matches('v').to_string(), date: date.trim().to_string(), items: vec![] });
        } else if let Some(rel) = out.last_mut() {
            let t = line.trim();
            if let Some(b) = t.strip_prefix("- ") {
                rel.items.push(b.trim().to_string());
            } else if !t.is_empty() && line.starts_with(char::is_whitespace) {
                if let Some(last) = rel.items.last_mut() {
                    last.push(' ');
                    last.push_str(t);
                }
            }
        }
    }
    out
}

pub fn releases() -> &'static [Release] {
    static R: OnceLock<Vec<Release>> = OnceLock::new();
    R.get_or_init(|| parse(CHANGELOG))
}

/// "0.8.10" → [0, 8, 10]; None if any part isn't a number.
pub fn version_key(v: &str) -> Option<Vec<u32>> {
    v.trim().trim_start_matches('v').split('.').map(|p| p.parse().ok()).collect()
}

/// `a` is a later version than `b`.
pub fn newer(a: &str, b: &str) -> bool {
    matches!((version_key(a), version_key(b)), (Some(a), Some(b)) if a > b)
}

thread_local! {
    static OPEN_REQUEST: Cell<bool> = const { Cell::new(false) };
}

/// Ask for the patch notes sheet from anywhere in the UI (e.g. a game menu row); `Ui` opens it.
pub fn request_open() {
    OPEN_REQUEST.with(|r| r.set(true));
}

pub(crate) fn take_request() -> bool {
    OPEN_REQUEST.with(|r| r.replace(false))
}

/// The open sheet.
#[derive(Default)]
pub struct PatchNotes {
    scroll: f32,
    /// Releases newer than this are highlighted ("What's new" after an update).
    pub seen: Option<String>,
    /// Shown as "WHAT'S NEW IN <version>" instead of "PATCH NOTES".
    pub whats_new: Option<String>,
}

impl PatchNotes {
    pub fn new() -> Self {
        Self::default()
    }

    /// After updating from `seen` (None: unknown, highlight just the newest entry) to `version`.
    pub fn whats_new(version: &str, seen: Option<&str>) -> Self {
        let seen = seen.map(str::to_string).or_else(|| releases().get(1).map(|r| r.version.clone()));
        PatchNotes { scroll: 0.0, seen, whats_new: Some(version.to_string()) }
    }

    fn fresh(&self, r: &Release) -> bool {
        self.seen.as_deref().is_some_and(|s| newer(&r.version, s))
    }

    /// Draws the sheet over everything; false once closed.
    pub fn draw(&mut self, inp: &mut Input) -> bool {
        let (sw, sh) = (screen_width(), screen_height());
        inp.block(Rect::new(0.0, 0.0, sw, sh));
        draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.0, 0.0, 0.0, 0.8));
        let top = crate::screens::safe_top() + 16.0 * s();
        let w = (sw - 24.0 * s()).min(520.0 * s());
        let r = Rect::new((sw - w) / 2.0, top, w, sh - top - crate::screens::safe_bottom() - 16.0 * s());
        cel::panel(r, PANEL);
        cel::round_rect_lines(r, 8.0 * s(), 1.5 * s(), if self.whats_new.is_some() { ACCENT } else { BORDER });
        let pad = 18.0 * s();
        let title = match &self.whats_new {
            Some(v) => format!("WHAT'S NEW IN {v}"),
            None => "PATCH NOTES".to_string(),
        };
        text(&title, r.x + pad, r.y + pad + 12.0 * s(), 22.0, ACCENT, FontKind::Title, Align::Left);
        draw_rectangle(r.x + pad, r.y + pad + 30.0 * s(), r.w - 2.0 * pad, 1.0 * s(), BORDER);

        let bh = 50.0 * s();
        let br = Rect::new(r.x + pad, r.y + r.h - bh - 14.0 * s(), r.w - 2.0 * pad, bh);
        let mut open = !widgets::button(inp, br, if self.whats_new.is_some() { "GOT IT" } else { "CLOSE" }, Style::Ghost);

        // list, scrolled by dragging (or the mouse wheel) inside it
        let list = Rect::new(r.x + 10.0 * s(), r.y + pad + 40.0 * s(), r.w - 20.0 * s(), br.y - r.y - pad - 52.0 * s());
        if let (Some(d), Some(p)) = (inp.drag, inp.drag_pos) {
            if list.contains(p) {
                self.scroll -= d.y;
            }
        }
        if let Some((c, k)) = inp.zoom {
            if list.contains(c) {
                self.scroll -= k.ln() / 1.15f32.ln() * 60.0 * s();
            }
        }
        // lay out first so the scroll can be clamped
        let text_w = list.w - 44.0 * s();
        let line_h = 19.0 * s();
        let mut blocks: Vec<(f32, f32, &Release, Vec<Vec<String>>)> = vec![];
        let mut h = 0.0;
        for rel in releases() {
            let items: Vec<Vec<String>> = rel.items.iter().map(|i| wrap(i, FontKind::Body, 14.0, text_w)).collect();
            let bh = 40.0 * s() + items.iter().map(|l| l.len() as f32 * line_h + 6.0 * s()).sum::<f32>() + 8.0 * s();
            blocks.push((h, bh, rel, items));
            h += bh + 8.0 * s();
        }
        self.scroll = self.scroll.clamp(0.0, (h - list.h).max(0.0));
        let y0 = list.y - self.scroll;
        cel::clip(Some(list));
        for (y, bh, rel, items) in &blocks {
            let y = y0 + y;
            if y + bh < list.y || y > list.y + list.h {
                continue;
            }
            let fresh = self.fresh(rel);
            let row = Rect::new(list.x, y, list.w, *bh);
            cel::panel(row, alpha(PANEL_HI, if fresh { 0.85 } else { 0.45 }));
            if fresh {
                draw_rectangle(row.x, row.y + 6.0 * s(), 3.0 * s(), row.h - 12.0 * s(), ACCENT);
            }
            let tx = row.x + 16.0 * s();
            text(&format!("VERSION {}", rel.version), tx, row.y + 20.0 * s(), 17.0, if fresh { ACCENT } else { PAPER }, FontKind::Title, Align::Left);
            let tag = if fresh { "NEW".to_string() } else { rel.date.clone() };
            text(&tag, row.x + row.w - 14.0 * s(), row.y + 20.0 * s(), 13.0, if fresh { ACCENT } else { MUTED }, FontKind::Title, Align::Right);
            let mut ly = row.y + 44.0 * s();
            for lines in items {
                draw_circle(tx + 3.0 * s(), ly - 1.0 * s(), 2.5 * s(), if fresh { ACCENT } else { MUTED });
                for l in lines {
                    text(l, tx + 14.0 * s(), ly, 14.0, PAPER, FontKind::Body, Align::Left);
                    ly += line_h;
                }
                ly += 6.0 * s();
            }
        }
        cel::clip(None);
        // scroll thumb
        if h > list.h {
            let th = (list.h * list.h / h).max(24.0 * s());
            let ty = list.y + (list.h - th) * self.scroll / (h - list.h);
            cel::round_rect(Rect::new(list.x + list.w + 3.0 * s(), ty, 3.0 * s(), th), 1.5 * s(), alpha(PAPER, 0.35));
        }
        if inp.take_back() {
            open = false;
        }
        open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changelog_parses() {
        let rs = releases();
        assert!(rs.len() > 10, "CHANGELOG.md has too few releases: {}", rs.len());
        for r in rs {
            assert!(version_key(&r.version).is_some(), "bad version header {:?}", r.version);
            assert_eq!(r.date.len(), 10, "{}: date should be YYYY-MM-DD, got {:?}", r.version, r.date);
            assert!(!r.items.is_empty(), "{} has no bullets", r.version);
        }
        for pair in rs.windows(2) {
            assert!(newer(&pair[0].version, &pair[1].version), "not newest first: {} then {}", pair[0].version, pair[1].version);
        }
    }

    /// Release builds (scripts/release-*.sh set HOHMANN_VERSION) must ship their own entry on top.
    #[test]
    fn newest_entry_is_this_release() {
        let top = &releases()[0].version;
        if let Some(v) = option_env!("HOHMANN_VERSION") {
            assert_eq!(top, v.trim_start_matches('v'), "add a CHANGELOG.md entry for {v}");
        }
        // and never behind the workspace version
        assert!(!newer(env!("CARGO_PKG_VERSION"), top), "CHANGELOG.md top {top} is older than the crate version");
    }

    /// The body font has no arrows etc.; keep bullets to characters it can draw.
    #[test]
    fn changelog_uses_drawable_characters() {
        for r in releases() {
            for i in &r.items {
                assert!(i.chars().all(|c| c.is_ascii() || "×…—–’“”·°Δ".contains(c)), "{}: undrawable character in {i:?}", r.version);
            }
        }
    }

    #[test]
    fn parser_format() {
        let rs = parse("# title\nintro\n\n## 1.2.0 — 2026-01-02\n- one\n  more\n- two\n## v1.1.0 - 2026-01-01\n- x\n");
        assert_eq!(rs.len(), 2);
        assert_eq!(rs[0], Release { version: "1.2.0".into(), date: "2026-01-02".into(), items: vec!["one more".into(), "two".into()] });
        assert_eq!((rs[1].version.as_str(), rs[1].date.as_str()), ("1.1.0", "2026-01-01"));
        assert!(newer("0.10.0", "0.9.9") && !newer("0.8.8", "0.8.8") && !newer("junk", "0.1.0"));
    }

    #[test]
    fn whats_new_highlights_since_last_seen() {
        let rs = releases();
        let n = PatchNotes::whats_new(&rs[0].version, Some(&rs[2].version));
        assert!(n.fresh(&rs[0]) && n.fresh(&rs[1]) && !n.fresh(&rs[2]));
        // unknown previous version: just the newest
        let n = PatchNotes::whats_new(&rs[0].version, None);
        assert!(n.fresh(&rs[0]) && !n.fresh(&rs[1]));
    }
}
