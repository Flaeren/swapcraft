//! The command-line helpers that only draw: the bar, the glow and the queue card as PNG files, to look at them without
//! a screen. They use the shared drawing, so they show what every system shows.
//!
//!   --render-glow <png> orange|green|red   the glow's bitmap
//!   --render-queue <png>                   the queue card with sample entries
//!   --render-bar <png> <ext> [n]           the drop bar for that file type, formats from the n-th on dimmed

use crate::bar_view::{View, render};
use crate::draw::{self, GREEN, Mon, ORANGE, RED, Rect};
use crate::{card_view, glow, offer, queue};

/// Runs a drawing command if `args` is one; true when it was.
pub fn render_command(args: &[String]) -> bool {
    let path = |i: usize, default: &str| args.get(i).cloned().unwrap_or_else(|| default.to_owned());
    match args.first().map(String::as_str) {
        Some("--render-glow") => {
            let color = match args.get(2).map(String::as_str) {
                Some("green") => GREEN,
                Some("red") => RED,
                _ => ORANGE,
            };
            let r = Rect { left: 0, top: 0, right: 960, bottom: 540 };
            let _ = glow::render(&Mon { full: r, work: r, scale: 1.0 }, color).save_png(path(1, "glow.png"));
        }
        Some("--render-queue") => {
            queue::push_demo();
            let (rows, c, finished) = card_view::current();
            let _ = card_view::render(1.0, &rows, &c, finished, -1, 0.35).save_png(path(1, "queue.png"));
        }
        Some("--render-bar") => {
            let ext = path(2, "docx");
            let mut o = offer::offer(&[ext]);
            if let Some(n) = args.get(3).and_then(|s| s.parse::<usize>().ok()) {
                for (i, s) in o.slots.iter_mut().chain(o.more.iter_mut()).enumerate() {
                    s.available = i < n;
                }
            }
            let expanded = !o.more.is_empty();
            let v = View { scale: 1.0, h: 720, hot: -1, offer: o, expanded, pick: false, accepted: None };
            let _ = render(&v).save_png(path(1, "bar.png"));
        }
        _ => return false,
    }
    let _ = draw::fonts; // the fonts are built on first use; nothing to do here
    true
}
