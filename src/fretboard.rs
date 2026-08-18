//! TBSS-FR-0013 · E4 — fretboard pictograph renderer.
//!
//! One guitar chord [`Voicing`] → one horizontal fretboard diagram, rasterised
//! to an `image::RgbaImage`. A single renderer with two consumers: the E2
//! panel shows the image as an egui texture, and E5 feeds the very same frames
//! to ffmpeg. Pure software rasteriser — no GPU, no font dependency — so the
//! output is deterministic (and therefore testable) and identical in both
//! consumers.
//!
//! Conventions (locked, TBSS-FR-0013 E4):
//!   * **horizontal** neck — nut / low frets at the left, frets increase to
//!     the right;
//!   * six strings as horizontal lines, **low-E at the bottom** (TAB-like);
//!   * fingers drawn as **numbers 1–4** inside the dots;
//!   * **open = ring**, **muted = ✕**, both in the left margin before the nut;
//!   * `mirror` flips the string order at *draw* time for left-handed players
//!     — the stored [`Voicing`] itself stays canonical right-handed.
//!
//! No text beyond the finger digits and an "Nfr" position label needs a font,
//! so a compact 3×5 bitmap of `0–9`, `f`, `r` suffices; the chord *name* is not
//! baked here (the panel labels it with egui; E5 can overlay it via ffmpeg
//! `drawtext`).
#![allow(dead_code)]

use crate::chorddb::Voicing;
use image::{Rgba, RgbaImage};

/// Rendering options. [`Default`] is a light "paper" diagram sized for a video
/// frame; override colours/size per consumer.
#[derive(Clone, Copy, Debug)]
pub struct RenderOpts {
    pub width: u32,
    pub height: u32,
    /// Left-handed mirror (flips string order top↔bottom). Data stays canonical.
    pub mirror: bool,
    /// Fret spaces drawn in the window.
    pub frets_shown: u8,
    pub bg: [u8; 4],
    pub line: [u8; 4],
    pub dot: [u8; 4],
    pub dot_text: [u8; 4],
    pub open_muted: [u8; 4],
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self {
            width: 640,
            height: 360,
            mirror: false,
            frets_shown: 5,
            bg: [250, 248, 244, 255],
            line: [40, 40, 44, 255],
            dot: [30, 90, 160, 255],
            dot_text: [255, 255, 255, 255],
            open_muted: [40, 40, 44, 255],
        }
    }
}

/// Render `v` with default options.
pub fn render_default(v: &Voicing) -> RgbaImage {
    render(v, &RenderOpts::default())
}

/// Rasterise a voicing to a horizontal fretboard diagram.
pub fn render(v: &Voicing, opts: &RenderOpts) -> RgbaImage {
    let w = opts.width.max(16) as i32;
    let h = opts.height.max(16) as i32;
    let mut img = RgbaImage::from_pixel(w as u32, h as u32, Rgba(opts.bg));

    // Window anchoring. Open strings ring at the nut, so any voicing that uses
    // one is drawn from the nut — never shifted up. Only a pure fretted shape
    // that won't fit in the nut window gets shifted to an "Nfr" position, the
    // standard chord-diagram convention.
    let base_shown = opts.frets_shown.max(1) as i32;
    let max_fret = v.frets.iter().copied().max().unwrap_or(0).max(0) as i32;
    let has_open = v.frets.contains(&0);
    let (start_fret, frets) = if !has_open && v.base_fret as i32 > 1 && max_fret > base_shown {
        (v.base_fret as i32, base_shown)
    } else {
        // Anchor at the nut; widen the window if a fretted note reaches past it.
        (1, base_shown.max(max_fret))
    };
    let at_nut = start_fret == 1;

    // Margins: a wide left margin for the open/muted markers + position label.
    let ml = (w as f32 * 0.16) as i32;
    let mr = (w as f32 * 0.05) as i32;
    let mt = (h as f32 * 0.14) as i32;
    let mb = (h as f32 * 0.14) as i32;

    let gx0 = ml;
    let gx1 = w - mr;
    let gy0 = mt;
    let gy1 = h - mb;
    let dx = (gx1 - gx0) as f32 / frets as f32; // one fret space
    let dy = (gy1 - gy0) as f32 / 5.0; // six strings → five gaps

    let xfret = |j: i32| gx0 + (j as f32 * dx) as i32;
    let ystr = |k: i32| gy0 + (k as f32 * dy) as i32;
    // String index (0 = low-E) → display row (0 = top). Canonical puts low-E at
    // the bottom (row 5); the left-handed mirror flips that.
    let row = |i: usize| -> i32 {
        if opts.mirror {
            i as i32
        } else {
            5 - i as i32
        }
    };

    // ── grid ────────────────────────────────────────────────────────────
    for k in 0..6 {
        line(&mut img, gx0, ystr(k), gx1, ystr(k), 2, opts.line);
    }
    for j in 0..=frets {
        let th = if j == 0 && at_nut { 6 } else { 2 };
        line(&mut img, xfret(j), gy0, xfret(j), gy1, th, opts.line);
    }

    // Position label ("5fr") when the window doesn't start at the nut.
    if !at_nut {
        let scale = ((h as f32 * 0.05 / 5.0) as i32).max(1);
        draw_text(
            &mut img,
            2,
            gy0,
            &format!("{start_fret}fr"),
            scale,
            opts.line,
        );
    }

    // ── barres (drawn under the dots) ─────────────────────────────────────
    let dot_r = (dx.min(dy) * 0.34) as i32;
    let bar_w = (dot_r as f32 * 1.5) as i32;
    for (fret, lo, hi) in v.barres() {
        let s = fret as i32 - start_fret;
        if s < 0 || s >= frets {
            continue;
        }
        let cx = gx0 + ((s as f32 + 0.5) * dx) as i32;
        let ya = ystr(row(lo)).min(ystr(row(hi)));
        let yb = ystr(row(lo)).max(ystr(row(hi)));
        fill_rect(&mut img, cx - bar_w / 2, ya, bar_w, yb - ya, opts.dot);
        fill_circle(&mut img, cx, ya, bar_w / 2, opts.dot);
        fill_circle(&mut img, cx, yb, bar_w / 2, opts.dot);
    }

    // ── per-string markers + dots ─────────────────────────────────────────
    let mx = ml / 2; // centre of the left margin
    let mk_r = (dy * 0.26) as i32;
    for i in 0..6 {
        let cy = ystr(row(i));
        match v.frets[i] {
            MUTED_SENTINEL => {
                let s = mk_r;
                line(&mut img, mx - s, cy - s, mx + s, cy + s, 2, opts.open_muted);
                line(&mut img, mx - s, cy + s, mx + s, cy - s, 2, opts.open_muted);
            }
            0 => ring(&mut img, mx, cy, mk_r, 2, opts.open_muted),
            f => {
                let s = f as i32 - start_fret;
                if s < 0 || s >= frets {
                    continue;
                }
                let cx = gx0 + ((s as f32 + 0.5) * dx) as i32;
                fill_circle(&mut img, cx, cy, dot_r, opts.dot);
                let fg = v.fingers[i];
                if (1..=4).contains(&fg) {
                    let scale = ((dot_r as f32 * 2.0 / 5.0 * 0.8) as i32).max(1);
                    draw_char_centered(&mut img, cx, cy, (b'0' + fg) as char, scale, opts.dot_text);
                }
            }
        }
    }

    img
}

/// Convert a rendered diagram into an egui image for the E2 panel texture.
pub fn to_color_image(img: &RgbaImage) -> egui::ColorImage {
    let (w, h) = img.dimensions();
    egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw())
}

// `Voicing::frets` uses -1 for a muted string; name it for the match arm.
const MUTED_SENTINEL: i8 = -1;

// ── drawing primitives ──────────────────────────────────────────────────

#[inline]
fn put(img: &mut RgbaImage, x: i32, y: i32, c: [u8; 4]) {
    if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
        img.put_pixel(x as u32, y as u32, Rgba(c));
    }
}

fn fill_rect(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, c: [u8; 4]) {
    for yy in y..y + h {
        for xx in x..x + w {
            put(img, xx, yy, c);
        }
    }
}

fn fill_circle(img: &mut RgbaImage, cx: i32, cy: i32, r: i32, c: [u8; 4]) {
    let r2 = r * r;
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r2 {
                put(img, cx + dx, cy + dy, c);
            }
        }
    }
}

fn ring(img: &mut RgbaImage, cx: i32, cy: i32, r: i32, th: i32, c: [u8; 4]) {
    let ro = r * r;
    let ri = (r - th).max(0) * (r - th).max(0);
    for dy in -r..=r {
        for dx in -r..=r {
            let d = dx * dx + dy * dy;
            if d <= ro && d >= ri {
                put(img, cx + dx, cy + dy, c);
            }
        }
    }
}

/// Bresenham line with square thickness.
fn line(img: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, th: i32, c: [u8; 4]) {
    let (mut x0, mut y0) = (x0, y0);
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let half = (th / 2).max(0);
    loop {
        fill_rect(img, x0 - half, y0 - half, th.max(1), th.max(1), c);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

// ── 3×5 bitmap font (digits + f/r) ──────────────────────────────────────

/// Rows top→bottom; the three low bits are the columns left→right.
fn glyph(ch: char) -> Option<[u8; 5]> {
    Some(match ch {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        'f' => [0b011, 0b100, 0b110, 0b100, 0b100],
        'r' => [0b110, 0b101, 0b100, 0b100, 0b100],
        _ => return None,
    })
}

fn draw_glyph(img: &mut RgbaImage, x0: i32, y0: i32, rows: [u8; 5], scale: i32, c: [u8; 4]) {
    for (ry, r) in rows.iter().enumerate() {
        for cx in 0..3 {
            if r & (1 << (2 - cx)) != 0 {
                fill_rect(
                    img,
                    x0 + cx * scale,
                    y0 + ry as i32 * scale,
                    scale,
                    scale,
                    c,
                );
            }
        }
    }
}

fn draw_char_centered(img: &mut RgbaImage, cx: i32, cy: i32, ch: char, scale: i32, c: [u8; 4]) {
    if let Some(rows) = glyph(ch) {
        draw_glyph(
            img,
            cx - (3 * scale) / 2,
            cy - (5 * scale) / 2,
            rows,
            scale,
            c,
        );
    }
}

fn draw_text(img: &mut RgbaImage, x: i32, y: i32, s: &str, scale: i32, c: [u8; 4]) {
    let mut cx = x;
    for ch in s.chars() {
        if let Some(rows) = glyph(ch) {
            draw_glyph(img, cx, y, rows, scale, c);
        }
        cx += 4 * scale;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chorddb::ChordDb;

    fn open_c() -> Voicing {
        // x32010, fingers _32_1_ — the canonical open C from the golden set.
        ChordDb::build()
            .best(0, crate::chorddb::Quality::Maj)
            .unwrap()
            .clone()
    }

    fn count_non_bg(img: &RgbaImage, bg: [u8; 4]) -> usize {
        img.pixels().filter(|p| p.0 != bg).count()
    }

    #[test]
    fn renders_expected_dimensions() {
        let opts = RenderOpts {
            width: 500,
            height: 300,
            ..Default::default()
        };
        let img = render(&open_c(), &opts);
        assert_eq!(img.dimensions(), (500, 300));
    }

    #[test]
    fn draws_non_trivial_content() {
        let opts = RenderOpts::default();
        let img = render(&open_c(), &opts);
        // The grid + dots + markers must paint a meaningful number of pixels.
        assert!(count_non_bg(&img, opts.bg) > 1000);
    }

    #[test]
    fn render_is_deterministic() {
        let v = open_c();
        let a = render_default(&v);
        let b = render_default(&v);
        assert_eq!(a.as_raw(), b.as_raw());
    }

    #[test]
    fn mirror_changes_an_asymmetric_shape() {
        let v = open_c(); // x32010 is asymmetric top↔bottom
        let normal = render(&v, &RenderOpts::default());
        let mirrored = render(
            &v,
            &RenderOpts {
                mirror: true,
                ..Default::default()
            },
        );
        assert_ne!(normal.as_raw(), mirrored.as_raw());
    }

    #[test]
    fn renders_every_flavour_without_panicking() {
        let db = ChordDb::build();
        use crate::chorddb::Quality::*;
        // Open (C), full barre (F), muted-string (D), a high-position voicing,
        // and a power chord — across mirror on/off.
        let mut samples: Vec<Voicing> = vec![
            db.best(0, Maj).unwrap().clone(),    // C, open
            db.best(5, Maj).unwrap().clone(),    // F, barre
            db.best(2, Maj).unwrap().clone(),    // D, muted low strings
            db.best(7, Power5).unwrap().clone(), // G5
        ];
        // A deliberately high voicing to exercise the "Nfr" label path.
        if let Some(high) = db
            .voicings(1, Maj)
            .iter()
            .find(|v| v.base_fret > 1)
            .cloned()
        {
            samples.push(high);
        }
        for v in &samples {
            for mirror in [false, true] {
                let img = render(
                    v,
                    &RenderOpts {
                        mirror,
                        ..Default::default()
                    },
                );
                assert_eq!(img.dimensions(), (640, 360));
            }
        }
    }
}
