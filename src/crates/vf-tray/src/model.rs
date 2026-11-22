//! What the drop bar is doing, without any window: placement and slide-in, which format is under the pointer, the
//! "More" list, what a drop means, the "started" confirmation and the soft fade out. A backend owns the window and
//! the timers; it feeds this model what the system reports (a drag arrived, the pointer is here, something was
//! dropped) and does what it answers. The same rules therefore hold on every system, and they are tested here.

use std::path::PathBuf;
use std::time::Instant;

use crate::bar_view::{BAR_W, View};
use crate::draw::Mon;
use crate::offer::{self, CHIP_BASE, Offer, Slot};

/// How long the bar slides in from the edge, and how long it takes to fade out again.
pub const ANIM_MS: f32 = 190.0;
pub const FADE_MS: f32 = 260.0;
/// How long "started" stays on the bar before it goes away.
pub const ACCEPTED_MS: u32 = 900;
/// How long the "More" list waits for a click after files were dropped on it.
pub const PICK_TIMEOUT_MS: u32 = 10_000;
/// Hovering "More" this long opens the full list.
pub const EXPAND_AFTER_MS: u32 = 260;
/// How far to the right the bar slides while it fades out (logical pixels).
const FADE_SLIDE: f32 = 36.0;
/// How far to the right the bar starts when it slides in (logical pixels).
const SLIDE_IN: f32 = 90.0;

pub struct BarModel {
    pub shown: bool,
    pub pick_mode: bool,
    pub pick_paths: Vec<PathBuf>,
    pub expanded: bool,
    /// The format under the pointer: a main slot index, `CHIP_BASE + k` for the "More" list, or -1.
    pub hot: i32,
    pub offer: Offer,
    /// After a drop that started conversions: how many files were taken.
    pub accepted: Option<usize>,
    // placement of the bar that is (or was last) on screen, in physical pixels (base = collapsed)
    pub scale_pct: u32,
    pub height: i32,
    pub base_x: i32,
    pub top: i32,
    pub anim_start: Option<Instant>,
    /// Set while the bar fades out (see `begin_fade`).
    pub fade_start: Option<Instant>,
}

/// What a hover changed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hover {
    pub changed: bool,
    /// Start the timer that opens the list ("More" is hot and the list is closed); otherwise cancel it.
    pub expand_timer: bool,
}

/// What a drop on the bar means.
#[derive(Clone, Debug, PartialEq)]
pub enum Drop {
    /// Dropped on "More": the files are kept, the list stays open, a click on a format chooses (timeout: `PICK_TIMEOUT_MS`).
    Pick,
    /// Dropped on a format: queue these files to become `slot`; then call `accept` with how many were taken.
    Submit(Slot),
    /// Dropped outside any format (or on one this PC cannot make): nothing happens.
    Nothing,
}

/// One frame of the fade out: how opaque the bar is (0..255) and how far right of its place it has slid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fade {
    pub alpha: u8,
    pub dx: i32,
    pub done: bool,
}

impl BarModel {
    pub const fn new() -> Self {
        BarModel {
            shown: false,
            pick_mode: false,
            pick_paths: Vec::new(),
            expanded: false,
            hot: -1,
            offer: Offer { slots: Vec::new(), more: Vec::new() },
            accepted: None,
            scale_pct: 100,
            height: 1000,
            base_x: 0,
            top: 0,
            anim_start: None,
            fade_start: None,
        }
    }

    pub fn scale(&self) -> f32 {
        self.scale_pct as f32 / 100.0
    }

    /// The part of the state the drawing and the hit test need.
    pub fn view(&self) -> View {
        View { scale: self.scale(), h: self.height, hot: self.hot, offer: self.offer.clone(), expanded: self.expanded && !self.offer.more.is_empty(), pick: self.pick_mode, accepted: self.accepted }
    }

    /// The bar appears on `mon` (the monitor the pointer is on), sliding in from its right edge. `offscreen`: tests
    /// keep it out of sight. `selection_known`: the files' types are known; if neither they nor an offer are, the
    /// formats most files can become are shown until the drag's own data corrects them.
    pub fn begin_show(&mut self, mon: &Mon, offscreen: bool, selection_known: bool, now: Instant) {
        let width = (BAR_W * mon.scale).round() as i32;
        self.scale_pct = (mon.scale * 100.0).round() as u32;
        self.height = mon.work.bottom - mon.work.top;
        self.base_x = if offscreen { mon.work.right + 5000 } else { mon.work.right - width };
        self.top = mon.work.top;
        self.hot = -1;
        self.shown = true;
        self.expanded = false;
        self.accepted = None;
        self.fade_start = None; // a new drag while the last bar was still fading
        self.anim_start = Some(now);
        if self.offer.is_empty() && !selection_known {
            self.offer = offer::generic();
        }
    }

    /// Everything back to rest: the bar is gone.
    pub fn reset(&mut self) {
        self.shown = false;
        self.hot = -1;
        self.expanded = false;
        self.pick_mode = false;
        self.accepted = None;
        self.pick_paths.clear();
        self.anim_start = None;
        self.fade_start = None;
        self.offer = Offer::default();
    }

    /// How far right of its place the bar still is while it slides in (physical pixels).
    pub fn slide_offset(&self, now: Instant) -> i32 {
        let Some(t0) = self.anim_start else { return 0 };
        let t = (now.saturating_duration_since(t0).as_secs_f32() * 1000.0 / ANIM_MS).min(1.0);
        let e = 1.0 - (1.0 - t).powi(3);
        ((1.0 - e) * SLIDE_IN * self.scale()) as i32
    }

    /// The slide in is over.
    pub fn slide_done(&self, now: Instant) -> bool {
        self.anim_start.map(|t| now.saturating_duration_since(t).as_secs_f32() * 1000.0 >= ANIM_MS).unwrap_or(true)
    }

    /// The window's left edge: its base place, moved left when the list is open, plus the slide.
    pub fn window_x(&self, v: &View, now: Instant) -> i32 {
        self.base_x - (v.extra() * v.scale).round() as i32 + self.slide_offset(now)
    }

    /// The pointer is over `hot` (see the field). Tells what to do about the timer that opens the list.
    pub fn hover(&mut self, hot: i32) -> Hover {
        let changed = std::mem::replace(&mut self.hot, hot) != hot;
        if !changed {
            return Hover { changed: false, expand_timer: false };
        }
        // hovering "More" for a moment opens the full list
        let expand_timer = hot >= 0 && hot < CHIP_BASE && self.offer.is_more_slot(hot as usize) && !self.expanded_now();
        Hover { changed: true, expand_timer }
    }

    fn expanded_now(&self) -> bool {
        self.expanded && !self.offer.more.is_empty()
    }

    /// Opens the full list. False when there is nothing behind "More" or it is open already.
    pub fn expand(&mut self) -> bool {
        if self.offer.more.is_empty() || self.expanded {
            return false;
        }
        self.expanded = true;
        true
    }

    /// What dropping `paths` where the pointer is (`hot`) means; the model changes only for `Pick`.
    pub fn drop_on(&mut self, hot: i32, paths: Vec<PathBuf>) -> Drop {
        if hot >= 0 && hot < CHIP_BASE && self.offer.is_more_slot(hot as usize) && self.offer.slots[hot as usize].available {
            self.pick_mode = true;
            self.pick_paths = paths;
            self.expanded = true;
            self.hot = -1;
            return Drop::Pick;
        }
        match self.offer.pick(hot) {
            Some(slot) => Drop::Submit(slot.clone()),
            None => Drop::Nothing,
        }
    }

    /// A click in pick mode on `hot`: the format and the files kept from the drop, or None (a click on nothing, or on a
    /// format this PC cannot make).
    pub fn click_pick(&mut self, hot: i32) -> Option<(Slot, Vec<PathBuf>)> {
        if !self.pick_mode {
            return None;
        }
        self.offer.pick(hot).map(|s| (s.clone(), self.pick_paths.clone()))
    }

    /// The files were handed to the queue: show "started" on the bar for `ACCEPTED_MS`, then take the bar away.
    pub fn accept(&mut self, hot: i32, taken: usize) {
        self.accepted = Some(taken);
        self.hot = hot;
        self.pick_mode = false;
        self.expanded = self.expanded && hot >= CHIP_BASE;
    }

    /// Starts the soft way out. False when the bar is not on screen or is fading already.
    pub fn begin_fade(&mut self, now: Instant) -> bool {
        if !self.shown || self.fade_start.is_some() {
            return false;
        }
        self.shown = false; // a new drag can bring the bar back at once
        self.fade_start = Some(now);
        true
    }

    /// The fade at `now`: fading out and sliding right a little, smoothly; `done` when it is over.
    pub fn fade(&self, now: Instant) -> Option<Fade> {
        let t0 = self.fade_start?;
        let t = (now.saturating_duration_since(t0).as_secs_f32() * 1000.0 / FADE_MS).min(1.0);
        let e = t * t * (3.0 - 2.0 * t); // smoothstep
        Some(Fade { alpha: (255.0 * (1.0 - e)) as u8, dx: (e * FADE_SLIDE * self.scale()) as i32, done: t >= 1.0 })
    }
}

impl Default for BarModel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::Rect;
    use std::time::Duration;

    fn mon() -> Mon {
        let r = Rect { left: 1920, top: 0, right: 3840, bottom: 1040 };
        Mon { full: Rect { bottom: 1080, ..r }, work: r, scale: 1.25 }
    }

    fn with_offer(exts: &[&str]) -> BarModel {
        let mut m = BarModel::new();
        m.offer = offer::offer(&exts.iter().map(|e| (*e).to_owned()).collect::<Vec<_>>());
        m
    }

    #[test]
    fn the_bar_slides_in_from_the_edge_of_the_monitor_under_the_pointer() {
        let mut m = with_offer(&["png"]);
        let t0 = Instant::now();
        m.begin_show(&mon(), false, true, t0);
        // the bar is as wide as BAR_W at 125 %, flush with the right edge of the work area
        assert_eq!(m.base_x, 3840 - (BAR_W * 1.25).round() as i32);
        assert_eq!((m.top, m.height, m.scale_pct, m.shown), (0, 1040, 125, true));
        assert!(m.slide_offset(t0) > 100, "starts well to the right: {}", m.slide_offset(t0));
        assert_eq!(m.slide_offset(t0 + Duration::from_millis(400)), 0, "and arrives");
        assert!(!m.slide_done(t0 + Duration::from_millis(50)) && m.slide_done(t0 + Duration::from_millis(250)));
        let v = m.view();
        assert_eq!(m.window_x(&v, t0 + Duration::from_millis(400)), m.base_x);
    }

    #[test]
    fn the_open_list_moves_the_window_left_by_its_width() {
        let mut m = with_offer(&["png"]);
        m.begin_show(&mon(), false, true, Instant::now());
        let later = Instant::now() + Duration::from_secs(1);
        let closed = m.window_x(&m.view(), later);
        assert!(m.expand());
        assert!(m.window_x(&m.view(), later) < closed);
        assert!(!m.expand(), "already open");
    }

    #[test]
    fn nothing_known_about_the_drag_shows_the_common_formats_until_it_is_known() {
        let mut m = BarModel::new();
        m.begin_show(&mon(), false, false, Instant::now());
        assert_eq!(m.offer, offer::generic());
        let mut m = BarModel::new();
        m.begin_show(&mon(), false, true, Instant::now());
        assert!(m.offer.is_empty(), "files with no conversion: nothing is invented");
    }

    #[test]
    fn hovering_more_asks_for_the_timer_once_and_only_while_the_list_is_closed() {
        let mut m = with_offer(&["png"]);
        m.begin_show(&mon(), false, true, Instant::now());
        let more = (m.offer.slots.len() - 1) as i32;
        assert_eq!(m.hover(0), Hover { changed: true, expand_timer: false });
        assert_eq!(m.hover(0), Hover { changed: false, expand_timer: false });
        assert_eq!(m.hover(more), Hover { changed: true, expand_timer: true });
        m.expand();
        assert_eq!(m.hover(0), Hover { changed: true, expand_timer: false });
        assert_eq!(m.hover(more), Hover { changed: true, expand_timer: false }, "the list is open: no timer");
    }

    #[test]
    fn a_drop_on_a_format_submits_it_and_a_drop_on_more_keeps_the_files_for_a_click() {
        let mut m = with_offer(&["png"]);
        m.begin_show(&mon(), false, true, Instant::now());
        let files = vec![PathBuf::from("a.png")];
        match m.drop_on(0, files.clone()) {
            Drop::Submit(s) if s.available => assert_eq!(s.id, m.offer.slots[0].id),
            Drop::Nothing => assert!(!m.offer.slots[0].available, "only a format without its engine refuses"),
            other => panic!("{other:?}"),
        }
        assert_eq!(m.drop_on(-1, files.clone()), Drop::Nothing);
        let more = (m.offer.slots.len() - 1) as i32;
        if m.offer.slots[more as usize].available {
            assert_eq!(m.drop_on(more, files.clone()), Drop::Pick);
            assert!(m.pick_mode && m.expanded && m.pick_paths == files);
            // a click on a chip of the list chooses it; a click on nothing does not
            assert!(m.click_pick(-1).is_none());
            if m.offer.more[0].available {
                let (slot, kept) = m.click_pick(CHIP_BASE).expect("the first chip");
                assert_eq!((slot.id, kept), (m.offer.more[0].id.clone(), files));
            }
        }
    }

    #[test]
    fn a_format_without_its_engine_takes_no_drop_and_no_click() {
        let mut m = with_offer(&["png"]);
        m.begin_show(&mon(), false, true, Instant::now());
        for s in m.offer.slots.iter_mut().chain(m.offer.more.iter_mut()) {
            s.available = false;
        }
        assert_eq!(m.drop_on(0, vec![]), Drop::Nothing);
        let more = (m.offer.slots.len() - 1) as i32;
        assert_eq!(m.drop_on(more, vec![]), Drop::Nothing, "More itself is shut when nothing behind it can be made");
        m.pick_mode = true;
        assert!(m.click_pick(CHIP_BASE).is_none());
    }

    #[test]
    fn accepting_shows_the_count_and_keeps_the_list_only_when_a_chip_was_chosen() {
        let mut m = with_offer(&["png"]);
        m.begin_show(&mon(), false, true, Instant::now());
        m.expanded = true;
        m.accept(1, 3);
        assert_eq!((m.accepted, m.hot, m.expanded, m.pick_mode), (Some(3), 1, false, false));
        m.expanded = true;
        m.accept(CHIP_BASE + 2, 1);
        assert!(m.expanded, "a chip of the list was chosen: the list stays while \"started\" shows");
    }

    #[test]
    fn the_fade_is_smooth_slides_right_and_ends_once() {
        let mut m = with_offer(&["png"]);
        let t0 = Instant::now();
        m.begin_show(&mon(), false, true, t0);
        assert!(m.fade(t0).is_none(), "no fade before it begins");
        assert!(m.begin_fade(t0));
        assert!(!m.begin_fade(t0), "already fading");
        assert!(!m.shown, "a new drag may bring the bar back at once");
        let f = |ms: u64| m.fade(t0 + Duration::from_millis(ms)).unwrap();
        assert_eq!((f(0).alpha, f(0).dx, f(0).done), (255, 0, false));
        let (a, b, c) = (f(65).alpha, f(130).alpha, f(195).alpha);
        assert!(a > b && b > c, "fades steadily: {a} {b} {c}");
        assert!(f(130).dx > 0 && f(130).dx < f(260).dx, "slides right");
        assert_eq!((f(260).alpha, f(260).done), (0, true));
        // showing again cancels the fade
        m.begin_show(&mon(), false, true, t0);
        assert!(m.fade(t0).is_none() && m.shown);
    }

    #[test]
    fn reset_brings_everything_back_to_rest() {
        let mut m = with_offer(&["png"]);
        m.begin_show(&mon(), false, true, Instant::now());
        m.drop_on((m.offer.slots.len() - 1) as i32, vec![PathBuf::from("a.png")]);
        m.reset();
        assert!(!m.shown && !m.pick_mode && !m.expanded && m.hot == -1 && m.accepted.is_none());
        assert!(m.pick_paths.is_empty() && m.offer.is_empty() && m.anim_start.is_none());
    }
}
