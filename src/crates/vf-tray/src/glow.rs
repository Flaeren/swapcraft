//! The glow along the screen edges, without any window: orange while files convert, green for a moment when they
//! are done, red (and still) when one failed. This is the state machine and the bitmap; a platform backend owns the
//! transparent click-through window, calls `tick` from a timer while `tick` says it is needed, and shows the frame.
//! It draws nothing and wants no timer while it is off or holding a steady colour, so it costs nothing at rest.
//! The light is kept low and never flashes.

use std::collections::VecDeque;
use std::time::Instant;

use tiny_skia::{Color, GradientStop, LinearGradient, Paint, Pixmap, Point, SpreadMode, Transform};

use crate::draw::{GREEN, Mon, ORANGE, RED};

/// How far in from the edge the light reaches, and how strong it is at the very edge (0..255 of the bitmap's alpha).
const REACH: f32 = 64.0;
const EDGE_ALPHA: u8 = 84;

struct Step {
    color: [u8; 4],
    /// Opacity of the whole glow at the end of the step (0 = invisible, 255 = as drawn).
    to: u8,
    ms: u32,
    /// Take the window down when the step ends.
    hide: bool,
}

#[derive(Default)]
pub struct Glow {
    mon: Option<Mon>,
    color: [u8; 4],
    alpha: f32,
    from: f32,
    started: Option<Instant>,
    steps: VecDeque<Step>,
    pm: Option<Pixmap>,
    /// What the glow is heading for: `Some(color)` when it is on and holding that colour (nothing else queued).
    steady: Option<[u8; 4]>,
    /// Counts the bitmaps made, so a backend that fades the window by its opacity (no new picture per frame) knows
    /// when the picture itself changed and has to be sent again.
    generation: u64,
}

/// What a timer tick did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tick {
    /// Nothing queued: a steady colour needs no timer (stop it).
    Idle,
    /// Still moving: show `frame()` and call `tick` again at the next timer.
    Running,
    /// The last step ended and the glow is off: take the window down and stop the timer.
    Off,
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The glow bitmap: four soft bands from the edges, in `color`.
pub fn render(mon: &Mon, color: [u8; 4]) -> Pixmap {
    let (w, h) = ((mon.full.right - mon.full.left) as u32, (mon.full.bottom - mon.full.top) as u32);
    let mut pm = Pixmap::new(w, h).expect("glow bitmap");
    let reach = REACH * mon.scale;
    let c = |a: u8| Color::from_rgba8(color[0], color[1], color[2], a);
    let stops = vec![GradientStop::new(0.0, c(EDGE_ALPHA)), GradientStop::new(0.35, c(EDGE_ALPHA / 3)), GradientStop::new(1.0, c(0))];
    let band = |pm: &mut Pixmap, from: (f32, f32), to: (f32, f32), rect: (f32, f32, f32, f32)| {
        let Some(shader) = LinearGradient::new(Point::from_xy(from.0, from.1), Point::from_xy(to.0, to.1), stops.clone(), SpreadMode::Pad, Transform::identity()) else { return };
        let mut p = Paint::default();
        p.shader = shader;
        if let Some(r) = tiny_skia::Rect::from_xywh(rect.0, rect.1, rect.2, rect.3) {
            pm.fill_rect(r, &p, Transform::identity(), None);
        }
    };
    let (wf, hf) = (w as f32, h as f32);
    band(&mut pm, (0.0, 0.0), (0.0, reach), (0.0, 0.0, wf, reach));
    band(&mut pm, (0.0, hf), (0.0, hf - reach), (0.0, hf - reach, wf, reach));
    band(&mut pm, (0.0, 0.0), (reach, 0.0), (0.0, 0.0, reach, hf));
    band(&mut pm, (wf, 0.0), (wf - reach, 0.0), (wf - reach, 0.0, reach, hf));
    pm
}

impl Glow {
    pub fn new() -> Self {
        Glow { color: ORANGE, ..Default::default() }
    }

    /// The glow's own state, as words, for the self-test.
    pub fn describe(&self) -> String {
        match (&self.steady, self.steps.is_empty(), self.alpha > 0.5) {
            (Some(c), _, true) if *c == ORANGE => "orange".into(),
            (Some(c), _, true) if *c == GREEN => "green".into(),
            (Some(c), _, true) if *c == RED => "red".into(),
            (_, false, _) => "changing".into(),
            _ => "off".into(),
        }
    }

    fn begin(&mut self, mon: impl FnOnce() -> Mon) {
        if self.mon.is_none() {
            self.mon = Some(mon());
        }
        self.from = self.alpha;
        self.started = Some(Instant::now());
        self.steady = None;
    }

    /// Files are converting: fade the orange glow in (and keep it) on the monitor the person is using, `mon` says
    /// which. False when it was already on, or on its way to orange: the fade is not restarted at every file.
    /// True when the timer must run.
    pub fn working(&mut self, mon: impl FnOnce() -> Mon) -> bool {
        if (self.steady == Some(ORANGE) && self.steps.is_empty()) || self.steps.back().is_some_and(|st| st.color == ORANGE && st.to == 255) {
            return false;
        }
        self.steps.clear();
        // from whatever is showing: out, then orange in
        if self.alpha > 0.5 && self.steady != Some(ORANGE) {
            self.steps.push_back(Step { color: self.color, to: 0, ms: 120, hide: false });
        }
        self.steps.push_back(Step { color: ORANGE, to: 255, ms: 240, hide: false });
        self.begin(mon);
        true
    }

    /// The work is over: orange out, then green (or red) in, hold, and out. Red holds longer and is the one thing
    /// that stays on screen long enough to be noticed. True: the timer must run.
    pub fn finish(&mut self, ok: bool, mon: impl FnOnce() -> Mon) -> bool {
        let (color, hold) = if ok { (GREEN, 900) } else { (RED, 6000) };
        self.steps.clear();
        if self.alpha > 0.5 {
            self.steps.push_back(Step { color: self.color, to: 0, ms: 140, hide: false });
        }
        self.steps.push_back(Step { color, to: 255, ms: 200, hide: false });
        self.steps.push_back(Step { color, to: 255, ms: hold, hide: false });
        self.steps.push_back(Step { color, to: 0, ms: 600, hide: true });
        self.begin(mon);
        true
    }

    /// The work was cancelled or left nothing to report: fade out without announcing anything. True: the timer must
    /// run (false when there was nothing on screen).
    pub fn stop(&mut self) -> bool {
        if self.alpha <= 0.5 && self.steps.is_empty() {
            return false;
        }
        self.steps.clear();
        let color = self.color;
        self.steps.push_back(Step { color, to: 0, ms: 300, hide: true });
        self.from = self.alpha;
        self.started = Some(Instant::now());
        self.steady = None;
        true
    }

    /// One timer tick: moves the glow along its steps.
    pub fn tick(&mut self) -> Tick {
        let Some(step) = self.steps.front() else { return Tick::Idle };
        let started = self.started.unwrap_or_else(Instant::now);
        let t = started.elapsed().as_millis() as f32 / step.ms.max(1) as f32;
        // a step that changes colour starts from nothing: switch the bitmap when it begins
        if (self.color != step.color && self.alpha <= 0.5) || self.pm.is_none() {
            self.color = step.color;
            self.pm = self.mon.as_ref().map(|m| render(m, step.color));
            self.generation += 1;
        }
        self.alpha = self.from + (step.to as f32 - self.from) * ease(t);
        if t < 1.0 {
            return Tick::Running;
        }
        self.alpha = step.to as f32;
        let (hide, color, to) = (step.hide, step.color, step.to);
        self.steps.pop_front();
        self.from = self.alpha;
        self.started = Some(Instant::now());
        if self.steps.is_empty() {
            self.steady = if to > 0 { Some(color) } else { None };
        }
        if hide { Tick::Off } else { Tick::Running }
    }

    /// The bitmap to show, where its top-left corner goes, and its opacity (0..255), or None while there is none.
    pub fn frame(&self) -> Option<(&Pixmap, i32, i32, u8)> {
        let (pm, mon) = (self.pm.as_ref()?, self.mon.as_ref()?);
        Some((pm, mon.full.left, mon.full.top, self.alpha.round().clamp(0.0, 255.0) as u8))
    }

    /// Which bitmap `frame` holds: it changes whenever a new one was drawn.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The glow went off: forget the monitor-sized bitmap (it is not kept while the glow is off).
    pub fn clear(&mut self) {
        self.pm = None;
        self.mon = None;
        self.alpha = 0.0;
        self.from = 0.0;
        self.steady = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::Rect;

    fn mon() -> Mon {
        let r = Rect { left: 0, top: 0, right: 200, bottom: 120 };
        Mon { full: r, work: r, scale: 1.0 }
    }

    /// Runs the glow's timer to its end as fast as ticks allow (the steps are short: a test waits for real time).
    fn run_until(g: &mut Glow, want: Tick, ms: u64) -> bool {
        let t0 = Instant::now();
        while t0.elapsed().as_millis() < ms as u128 {
            if g.tick() == want {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn working_fades_in_orange_and_stays_without_a_timer() {
        let mut g = Glow::new();
        assert_eq!(g.describe(), "off");
        assert!(g.working(mon), "the first call starts the fade");
        assert!(!g.working(mon), "a second file does not restart it");
        assert!(run_until(&mut g, Tick::Idle, 1500), "after the fade the glow holds and wants no timer");
        assert_eq!(g.describe(), "orange");
        let (pm, x, y, a) = g.frame().expect("a frame while it is on");
        assert_eq!((pm.width(), pm.height(), x, y, a), (200, 120, 0, 0, 255));
    }

    #[test]
    fn finishing_goes_orange_green_then_off_and_frees_the_bitmap() {
        let mut g = Glow::new();
        g.working(mon);
        assert!(run_until(&mut g, Tick::Idle, 1500));
        assert!(g.finish(true, mon));
        assert!(run_until(&mut g, Tick::Off, 4000), "orange out, green in, hold, out");
        g.clear();
        assert_eq!(g.describe(), "off");
        assert!(g.frame().is_none());
    }

    #[test]
    fn a_failure_is_red_and_holds() {
        let mut g = Glow::new();
        g.working(mon);
        assert!(run_until(&mut g, Tick::Idle, 1500));
        g.finish(false, mon);
        // after the colour change and the fade in, red is on screen for seconds
        std::thread::sleep(std::time::Duration::from_millis(900));
        for _ in 0..200 {
            g.tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(g.color, RED);
        assert!(g.alpha > 200.0, "red holds at full strength: {}", g.alpha);
    }

    #[test]
    fn stop_with_nothing_showing_does_nothing() {
        let mut g = Glow::new();
        assert!(!g.stop());
    }
}
