//! What the queue card looks like: the rows (running first, then waiting, then what finished), the summary line,
//! the buttons, the bitmap and the hit test. Nothing here touches a window, so every system draws the same card.
//! The working bar is a sliding segment, not a percentage: the engines do not report progress.

use tiny_skia::{FillRule, Pixmap, Stroke, Transform};

use crate::draw::{self as gfx, CARD, CHIP, FAINT, GREEN, INK, LINE, MUTED, ORANGE, RED, STRONG};
use crate::queue::{self, Counts, Entry, Status};

pub const W: f32 = 340.0;
pub const PAD: f32 = 14.0;
pub const HEAD_H: f32 = 46.0;
pub const ROW_H: f32 = 46.0;
pub const FOOT_H: f32 = 52.0;
pub const EMPTY_H: f32 = 52.0;
pub const MAX_ROWS: usize = 6;
pub const BTN_OPEN: i32 = 100;
pub const BTN_CLEAR: i32 = 101;
pub const BTN_CANCEL: i32 = 102;

/// Running first, then the waiting ones in order, then what finished, newest first.
pub fn ordered(all: Vec<Entry>) -> Vec<Entry> {
    let mut running: Vec<Entry> = all.iter().filter(|e| e.status == Status::Running).cloned().collect();
    running.extend(all.iter().filter(|e| e.status == Status::Waiting).cloned());
    running.extend(all.iter().rev().filter(|e| !matches!(e.status, Status::Running | Status::Waiting)).cloned());
    running.truncate(MAX_ROWS);
    running
}

pub fn summary(c: &Counts, empty: bool) -> String {
    if empty {
        "Nessun lavoro".to_owned()
    } else if c.running > 0 && c.waiting > 0 {
        format!("{} in corso \u{b7} {} in coda", c.running, c.waiting)
    } else if c.running > 0 {
        format!("{} in corso", c.running)
    } else if c.waiting > 0 {
        format!("{} in coda", c.waiting)
    } else if c.failed > 0 {
        "Finito, con errori".to_owned()
    } else {
        "Tutto fatto".to_owned()
    }
}

pub struct Layout {
    pub height: f32,
    /// (hit id, x, y, w, h, label) in logical pixels.
    pub buttons: Vec<(i32, f32, f32, f32, f32, &'static str)>,
}

pub fn layout(rows: usize, c: &Counts, has_finished: bool) -> Layout {
    let f = gfx::fonts();
    let body = if rows == 0 { EMPTY_H } else { rows as f32 * ROW_H };
    let height = HEAD_H + body + FOOT_H;
    let mut labels: Vec<(i32, &'static str)> = vec![(BTN_OPEN, "Apri SwapCraft")];
    if has_finished {
        labels.push((BTN_CLEAR, "Svuota"));
    }
    if c.active() > 0 {
        labels.push((BTN_CANCEL, "Annulla tutto"));
    }
    let y = HEAD_H + body + 11.0;
    let mut x = PAD;
    let mut buttons = Vec::new();
    for (id, label) in labels {
        let w = gfx::text_width(&f.inter, 12.0, 0.0, label) + 28.0;
        buttons.push((id, x, y, w, 30.0, label));
        x += w + 8.0;
    }
    Layout { height, buttons }
}

pub fn render(scale: f32, rows: &[Entry], c: &Counts, has_finished: bool, hot: i32, phase: f32) -> Pixmap {
    let f = gfx::fonts();
    let l = layout(rows.len(), c, has_finished);
    let (pw, ph) = ((W * scale).round() as u32, (l.height * scale).round() as u32);
    let mut pm = Pixmap::new(pw, ph).expect("card bitmap");
    let s = scale;
    // the card
    let card = gfx::round_rect(0.5 * s, 0.5 * s, pw as f32 - s, ph as f32 - s, 14.0 * s);
    pm.fill_path(&card, &gfx::paint(CARD), FillRule::Winding, Transform::identity(), None);
    pm.stroke_path(&card, &gfx::paint([51, 51, 51, 255]), &Stroke { width: s, ..Default::default() }, Transform::identity(), None);
    // header
    gfx::draw_text(&mut pm, &f.semi, 13.0 * s, 0.0, PAD * s, 23.0 * s, STRONG, "SwapCraft");
    gfx::draw_text_right(&mut pm, &f.inter, 11.5 * s, (W - PAD) * s, 23.0 * s, MUTED, &summary(c, rows.is_empty()));
    let hline = |pm: &mut Pixmap, y: f32| {
        if let Some(r) = tiny_skia::Rect::from_xywh(PAD * s, y * s, (W - 2.0 * PAD) * s, s.max(1.0)) {
            pm.fill_rect(r, &gfx::paint(CHIP), Transform::identity(), None);
        }
    };
    hline(&mut pm, HEAD_H - 1.0);
    if rows.is_empty() {
        gfx::draw_text(&mut pm, &f.inter, 12.0 * s, 0.0, PAD * s, (HEAD_H + EMPTY_H / 2.0) * s, FAINT, "Trascina un file sul bordo dello schermo.");
    }
    for (i, e) in rows.iter().enumerate() {
        let y0 = HEAD_H + i as f32 * ROW_H;
        if hot == i as i32 {
            pm.fill_path(&gfx::round_rect(6.0 * s, (y0 + 2.0) * s, (W - 12.0) * s, (ROW_H - 4.0) * s, 8.0 * s), &gfx::paint(gfx::with_alpha(CHIP, 200)), FillRule::Winding, Transform::identity(), None);
        }
        let (status, color) = match &e.status {
            Status::Running => ("in corso", ORANGE),
            Status::Waiting => ("in coda", FAINT),
            Status::Done(_) => ("fatto", GREEN),
            Status::Failed(_) => ("errore", RED),
            Status::Cancelled => ("annullato", FAINT),
        };
        let sw = gfx::text_width(&f.plex, 10.5 * s, 0.0, status);
        let name = gfx::fit(&f.inter, 12.5 * s, (W - 2.0 * PAD) * s - sw - 14.0 * s, &e.name);
        gfx::draw_text(&mut pm, &f.inter, 12.5 * s, 0.0, PAD * s, (y0 + 16.0) * s, INK, &name);
        gfx::draw_text_right(&mut pm, &f.plex, 10.5 * s, (W - PAD) * s, (y0 + 16.0) * s, color, status);
        let max2 = (W - 2.0 * PAD) * s;
        match &e.status {
            Status::Running => {
                let (tx, ty, tw, th) = (PAD * s, (y0 + 31.0) * s, (W - 2.0 * PAD) * s, 4.0 * s);
                pm.fill_path(&gfx::pill(tx, ty, tw, th), &gfx::paint(CHIP), FillRule::Winding, Transform::identity(), None);
                // a segment that slides along the track (no percentage: the engines do not report one)
                let seg = tw * 0.32;
                let pos = (phase % 1.0) * (tw + seg) - seg;
                let (x0, x1) = (pos.max(0.0), (pos + seg).min(tw));
                if x1 - x0 > th {
                    pm.fill_path(&gfx::pill(tx + x0, ty, x1 - x0, th), &gfx::paint(ORANGE), FillRule::Winding, Transform::identity(), None);
                }
            }
            Status::Waiting => gfx::draw_text(&mut pm, &f.inter, 11.0 * s, 0.0, PAD * s, (y0 + 33.0) * s, FAINT, &format!("\u{2192} {}", e.to)),
            Status::Done(p) => {
                let out = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                gfx::draw_text(&mut pm, &f.inter, 11.0 * s, 0.0, PAD * s, (y0 + 33.0) * s, FAINT, &gfx::fit(&f.inter, 11.0 * s, max2, &format!("\u{2192} {out}")));
            }
            Status::Failed(m) => gfx::draw_text(&mut pm, &f.inter, 11.0 * s, 0.0, PAD * s, (y0 + 33.0) * s, gfx::with_alpha(RED, 220), &gfx::fit(&f.inter, 11.0 * s, max2, m)),
            Status::Cancelled => {}
        }
    }
    hline(&mut pm, HEAD_H + if rows.is_empty() { EMPTY_H } else { rows.len() as f32 * ROW_H } + 3.0);
    for (id, x, y, w, h, label) in &l.buttons {
        let is_hot = hot == *id;
        let path = gfx::pill(x * s, y * s, w * s, h * s);
        pm.fill_path(&path, &gfx::paint(if is_hot { [38, 38, 37, 255] } else { CHIP }), FillRule::Winding, Transform::identity(), None);
        pm.stroke_path(&path, &gfx::paint(if is_hot { [90, 90, 88, 255] } else { LINE }), &Stroke { width: s, ..Default::default() }, Transform::identity(), None);
        let tw = gfx::text_width(&f.inter, 12.0 * s, 0.0, label);
        gfx::draw_text(&mut pm, &f.inter, 12.0 * s, 0.0, (x + w / 2.0) * s - tw / 2.0, (y + h / 2.0) * s, if is_hot { STRONG } else { INK }, label);
    }
    pm
}

pub fn hit(lx: f32, ly: f32, rows: usize, c: &Counts, has_finished: bool) -> i32 {
    let l = layout(rows, c, has_finished);
    for (id, x, y, w, h, _) in &l.buttons {
        if lx >= *x && lx < x + w && ly >= *y && ly < y + h {
            return *id;
        }
    }
    if ly >= HEAD_H && ly < HEAD_H + rows as f32 * ROW_H {
        return ((ly - HEAD_H) / ROW_H) as i32;
    }
    -1
}

pub fn current() -> (Vec<Entry>, Counts, bool) {
    let all = queue::snapshot();
    let finished = all.iter().any(|e| !matches!(e.status, Status::Running | Status::Waiting));
    (ordered(all), queue::counts(), finished)
}
