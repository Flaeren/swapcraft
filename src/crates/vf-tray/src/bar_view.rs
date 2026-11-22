//! What the drop bar looks like: its sizes, the view of the state it draws and the bitmap. Nothing here touches
//! a window or the system, so every platform draws the same bar and a test can render it to a PNG.

use tiny_skia::{Color, FillRule, GradientStop, LinearGradient, Paint, Pixmap, Point, SpreadMode, Stroke, StrokeDash, Transform};

use crate::draw::{self as gfx, FAINT, INK, LINE, STRONG, VIOLET};
use crate::offer::{CHIP_BASE, Offer, Slot};

// logical (96 dpi) sizes, multiplied by the monitor's scale
pub const BAR_W: f32 = 210.0;
pub const SLOT_X: f32 = 64.0;
pub const SLOT_W: f32 = 134.0;
pub const SLOT_H: f32 = 34.0;
pub const SLOT_GAP: f32 = 8.0;
pub const TITLE_H: f32 = 26.0;
pub const TITLE_GAP: f32 = 14.0;
pub const CHIP_W: f32 = 112.0;
pub const CHIP_H: f32 = 30.0;
pub const CHIP_GAP: f32 = 8.0;
/// Extra width when the "More" list is open: [ramp 40][2 columns of formats][gap] then the slots.
pub const PANEL_W: f32 = 224.0;
pub const PANEL_X: f32 = 40.0;
/// What a format shows in place of its extension when its engine is not on this PC.
pub const MISSING: &str = "manca";

// ---------- layout and drawing ----------

/// Everything the drawing and the hit test need, copied out of the state.
#[derive(Clone)]
pub struct View {
    pub scale: f32,
    pub h: i32,
    pub hot: i32,
    pub offer: Offer,
    pub expanded: bool,
    pub pick: bool,
    pub accepted: Option<usize>,
}


impl View {
    pub fn extra(&self) -> f32 {
        if self.expanded { PANEL_W } else { 0.0 }
    }
    pub fn width_px(&self) -> i32 {
        ((BAR_W + self.extra()) * self.scale).round() as i32
    }
    pub fn first_slot_y(&self) -> f32 {
        let n = self.offer.slots.len();
        let total = TITLE_H + TITLE_GAP + n as f32 * SLOT_H + (n.max(1) - 1) as f32 * SLOT_GAP;
        (self.h as f32 - total * self.scale) / 2.0 + (TITLE_H + TITLE_GAP) * self.scale
    }
    pub fn title_top(&self) -> f32 {
        self.first_slot_y() - (TITLE_H + TITLE_GAP) * self.scale
    }
    pub fn slot_box(&self, i: usize) -> (f32, f32, f32, f32) {
        ((self.extra() + SLOT_X) * self.scale, self.first_slot_y() + i as f32 * (SLOT_H + SLOT_GAP) * self.scale, SLOT_W * self.scale, SLOT_H * self.scale)
    }
    pub fn chip_box(&self, k: usize) -> (f32, f32, f32, f32) {
        let (col, row) = ((k % 2) as f32, (k / 2) as f32);
        ((PANEL_X + col * (CHIP_W + CHIP_GAP)) * self.scale, self.first_slot_y() + row * (CHIP_H + CHIP_GAP) * self.scale, CHIP_W * self.scale, CHIP_H * self.scale)
    }
    /// The hot id under a point in window coordinates: a main slot index, `CHIP_BASE + k`, or -1.
    pub fn hit(&self, lx: f32, ly: f32) -> i32 {
        let inside = |b: (f32, f32, f32, f32)| lx >= b.0 && lx < b.0 + b.2 && ly >= b.1 && ly < b.1 + b.3;
        // a format this PC cannot make (its engine is missing) is drawn dimmed and never answers
        for i in 0..self.offer.slots.len() {
            if self.offer.slots[i].available && inside(self.slot_box(i)) {
                return i as i32;
            }
        }
        if self.expanded {
            for k in 0..self.offer.more.len() {
                if self.offer.more[k].available && inside(self.chip_box(k)) {
                    return CHIP_BASE + k as i32;
                }
            }
        }
        -1
    }
}

pub fn render(v: &View) -> Pixmap {
    let f = gfx::fonts();
    let (w, h, s) = (v.width_px() as u32, v.h, v.scale);
    let mut pm = Pixmap::new(w, h as u32).expect("bar bitmap");
    // the page fades in from the left, like the mockup: transparent -> #121212
    let bg = |a: u8| Color::from_rgba8(18, 18, 18, a);
    let grad = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(w as f32, 0.0),
        vec![GradientStop::new(0.0, bg(0)), GradientStop::new((0.28 * BAR_W * s / w as f32).min(1.0), bg(245)), GradientStop::new(1.0, bg(245))],
        SpreadMode::Pad,
        Transform::identity(),
    );
    if let Some(shader) = grad {
        let mut p = Paint::default();
        p.shader = shader;
        if let Some(r) = tiny_skia::Rect::from_xywh(0.0, 0.0, w as f32, h as f32) {
            pm.fill_rect(r, &p, Transform::identity(), None);
        }
    }
    let tt = v.title_top();
    let tx = (v.extra() + SLOT_X) * s + 2.0 * s;
    let (l1, l2) = if let Some(n) = v.accepted {
        ("AVVIATO".to_owned(), if n == 1 { "1 FILE".to_owned() } else { format!("{n} FILE") })
    } else if v.offer.is_empty() {
        ("NESSUNA".to_owned(), "CONVERSIONE".to_owned())
    } else if !v.offer.slots.iter().any(|s| s.available) {
        // every format needs an engine this PC does not have
        ("MOTORE NON".to_owned(), "INSTALLATO".to_owned())
    } else if v.pick {
        ("SCEGLI IL".to_owned(), "FORMATO".to_owned())
    } else {
        ("LASCIA QUI PER".to_owned(), "CONVERTIRE".to_owned())
    };
    gfx::draw_text(&mut pm, &f.plex, 10.0 * s, 0.9 * s, tx, tt + 5.0 * s, VIOLET, &l1);
    gfx::draw_text(&mut pm, &f.plex, 10.0 * s, 0.9 * s, tx, tt + 18.0 * s, VIOLET, &l2);

    let draw_pill = |pm: &mut Pixmap, b: (f32, f32, f32, f32), hot: bool, dashed: bool, enabled: bool, label: &str, ext: &str| {
        let (x, y, sw, sh) = b;
        let path = gfx::pill(x, y, sw, sh);
        let fill = if hot {
            [203, 176, 247, 40]
        } else if enabled {
            [30, 30, 29, 215]
        } else {
            [22, 22, 21, 150]
        };
        pm.fill_path(&path, &gfx::paint(fill), FillRule::Winding, Transform::identity(), None);
        let mut stroke = Stroke { width: if hot { 1.4 * s } else { s }, ..Default::default() };
        if !hot && (dashed || !enabled) {
            stroke.dash = StrokeDash::new(vec![3.5 * s, 3.0 * s], 0.0);
        }
        let line = if enabled { LINE } else { gfx::with_alpha(LINE, 110) };
        pm.stroke_path(&path, &gfx::paint(if hot { VIOLET } else { line }), &stroke, Transform::identity(), None);
        let cy = y + sh / 2.0;
        let (font, color) = if hot {
            (&f.semi, STRONG)
        } else if enabled {
            (&f.inter, INK)
        } else {
            (&f.inter, gfx::with_alpha(FAINT, 150))
        };
        let label = gfx::fit(font, 12.5 * s, sw - 28.0 * s - gfx::text_width(&f.plex, 10.0 * s, 0.0, ext) - 8.0 * s, label);
        gfx::draw_text(pm, font, 12.5 * s, 0.0, x + 14.0 * s, cy, color, &label);
        gfx::draw_text_right(pm, &f.plex, 10.0 * s, x + sw - 14.0 * s, cy, FAINT, ext);
    };

    let ext_text = |slot: &Slot, more: bool| -> String {
        if more {
            "\u{203a}".to_owned()
        } else if slot.ext.is_empty() {
            String::new()
        } else {
            format!(".{}", slot.ext)
        }
    };
    for (i, slot) in v.offer.slots.iter().enumerate() {
        let more = v.offer.is_more_slot(i);
        let chosen = v.accepted.is_some() && v.hot == i as i32;
        let ext = if chosen { "avviato".to_owned() } else { ext_text(slot, more) };
        let ext = if slot.available || more { ext } else { MISSING.to_owned() };
        draw_pill(&mut pm, v.slot_box(i), v.hot == i as i32, true, slot.available, &slot.label, &ext);
    }
    if v.expanded {
        gfx::draw_text(&mut pm, &f.plex, 10.0 * s, 0.9 * s, PANEL_X * s + 2.0 * s, tt + 5.0 * s, VIOLET, "TUTTI I FORMATI");
        for (k, slot) in v.offer.more.iter().enumerate() {
            let chosen = v.accepted.is_some() && v.hot == CHIP_BASE + k as i32;
            let ext = if chosen { "avviato".to_owned() } else if slot.available { ext_text(slot, false) } else { MISSING.to_owned() };
            draw_pill(&mut pm, v.chip_box(k), v.hot == CHIP_BASE + k as i32, false, slot.available, &slot.label, &ext);
        }
    }
    pm
}
