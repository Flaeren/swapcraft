//! The glow along the screen edges: orange while files convert, green for a moment when they are done, red (and
//! still) when one failed. A transparent, click-through window the size of the monitor the person was using; its
//! colours fade in and out. It draws nothing and runs no timer while it is off or holding a steady colour, so it
//! costs nothing at rest. The light is kept low and never flashes.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;

use tiny_skia::{Color, GradientStop, LinearGradient, Paint, Pixmap, Point, SpreadMode, Transform};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use crate::gfx::{self, GREEN, Mon, ORANGE, RED};

const TIMER: usize = 1;
const FRAME_MS: u32 = 16;
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

struct State {
    hwnd: isize,
    mon: Option<Mon>,
    color: [u8; 4],
    alpha: f32,
    from: f32,
    started: Instant,
    steps: VecDeque<Step>,
    pm: Option<Pixmap>,
    /// What the glow is heading for: `Some(color)` when it is on and holding that colour (nothing else queued).
    steady: Option<[u8; 4]>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let st = g.get_or_insert_with(|| State { hwnd: 0, mon: None, color: ORANGE, alpha: 0.0, from: 0.0, started: Instant::now(), steps: VecDeque::new(), pm: None, steady: None });
    f(st)
}

/// The glow's own state, as words, for the self-test.
pub fn describe() -> String {
    with(|s| match (&s.steady, s.steps.is_empty(), s.alpha > 0.5) {
        (Some(c), _, true) if *c == ORANGE => "orange".into(),
        (Some(c), _, true) if *c == GREEN => "green".into(),
        (Some(c), _, true) if *c == RED => "red".into(),
        (_, false, _) => "changing".into(),
        _ => "off".into(),
    })
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The glow bitmap: four soft bands from the edges, in `color`.
fn render(mon: &Mon, color: [u8; 4]) -> Pixmap {
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

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if wp.0 == TIMER => {
            tick(hwnd);
            LRESULT(0)
        }
        WM_NCHITTEST => LRESULT(-1), // HTTRANSPARENT: the mouse goes to whatever is below
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn ensure_window() -> HWND {
    let existing = with(|s| s.hwnd);
    if existing != 0 {
        return HWND(existing as *mut _);
    }
    let hwnd = unsafe {
        let hi = GetModuleHandleW(None).expect("module");
        let class = w!("SwapCraftGlow");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hi.into(), lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        let ex = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT;
        CreateWindowExW(ex, class, w!("SwapCraft glow"), WS_POPUP, 0, 0, 10, 10, None, None, Some(hi.into()), None).expect("glow window")
    };
    with(|s| s.hwnd = hwnd.0 as isize);
    hwnd
}

fn start_timer(hwnd: HWND) {
    unsafe { SetTimer(Some(hwnd), TIMER, FRAME_MS, None) };
}

/// Files are converting: fade the orange glow in (and keep it) on the monitor the person is using.
pub fn working() {
    // already on (or on its way to) orange: leave it alone, so the fade is not restarted at every file
    if with(|s| (s.steady == Some(ORANGE) && s.steps.is_empty()) || s.steps.back().is_some_and(|st| st.color == ORANGE && st.to == 255)) {
        return;
    }
    let hwnd = ensure_window();
    with(|s| {
        if s.mon.is_none() {
            let (x, y) = gfx::cursor();
            s.mon = Some(gfx::monitor_at(x, y));
        }
        s.steps.clear();
        // from whatever is showing: out, then orange in
        if s.alpha > 0.5 && s.steady != Some(ORANGE) {
            s.steps.push_back(Step { color: s.color, to: 0, ms: 120, hide: false });
        }
        s.steps.push_back(Step { color: ORANGE, to: 255, ms: 240, hide: false });
        s.from = s.alpha;
        s.started = Instant::now();
        s.steady = None;
    });
    start_timer(hwnd);
}

/// The work is over: orange out, then green (or red) in, hold, and out. Red holds longer and is the one thing
/// that stays on screen long enough to be noticed.
pub fn finish(ok: bool) {
    let hwnd = ensure_window();
    with(|s| {
        if s.mon.is_none() {
            let (x, y) = gfx::cursor();
            s.mon = Some(gfx::monitor_at(x, y));
        }
        let (color, hold) = if ok { (GREEN, 900) } else { (RED, 6000) };
        s.steps.clear();
        if s.alpha > 0.5 {
            s.steps.push_back(Step { color: s.color, to: 0, ms: 140, hide: false });
        }
        s.steps.push_back(Step { color, to: 255, ms: 200, hide: false });
        s.steps.push_back(Step { color, to: 255, ms: hold, hide: false });
        s.steps.push_back(Step { color, to: 0, ms: 600, hide: true });
        s.from = s.alpha;
        s.started = Instant::now();
        s.steady = None;
    });
    start_timer(hwnd);
}

/// The work was cancelled or left nothing to report: fade out without announcing anything.
pub fn stop() {
    let hwnd = ensure_window();
    let any = with(|s| {
        if s.alpha <= 0.5 && s.steps.is_empty() {
            return false;
        }
        s.steps.clear();
        let color = s.color;
        s.steps.push_back(Step { color, to: 0, ms: 300, hide: true });
        s.from = s.alpha;
        s.started = Instant::now();
        s.steady = None;
        true
    });
    if any {
        start_timer(hwnd);
    }
}

fn tick(hwnd: HWND) {
    let done_hide = with(|s| {
        let Some(step) = s.steps.front() else {
            return None;
        };
        let t = s.started.elapsed().as_millis() as f32 / step.ms.max(1) as f32;
        // a step that changes colour starts from nothing: switch the bitmap when it begins
        if s.color != step.color && s.alpha <= 0.5 {
            s.color = step.color;
            s.pm = s.mon.as_ref().map(|m| render(m, step.color));
        } else if s.pm.is_none() {
            s.color = step.color;
            s.pm = s.mon.as_ref().map(|m| render(m, step.color));
        }
        let e = ease(t);
        s.alpha = s.from + (step.to as f32 - s.from) * e;
        if t >= 1.0 {
            s.alpha = step.to as f32;
            let hide = step.hide;
            let (color, to) = (step.color, step.to);
            s.steps.pop_front();
            s.from = s.alpha;
            s.started = Instant::now();
            if s.steps.is_empty() {
                s.steady = if to > 0 { Some(color) } else { None };
            }
            return Some(hide);
        }
        Some(false)
    });
    // draw this frame
    let draw = with(|s| {
        let (Some(pm), Some(mon)) = (s.pm.as_ref(), s.mon.as_ref()) else { return None };
        let off = if gfx::offscreen() { 6000 } else { 0 };
        Some((pm.clone(), mon.full.left + off, mon.full.top, s.alpha.round().clamp(0.0, 255.0) as u8))
    });
    if let Some((pm, x, y, a)) = draw {
        gfx::push(hwnd, &pm, x, y, a);
        unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }
    match done_hide {
        Some(true) => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER);
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            with(|s| {
                s.pm = None; // the monitor-sized bitmap is not kept while the glow is off
                s.mon = None;
                s.alpha = 0.0;
                s.from = 0.0;
                s.steady = None;
            });
        }
        Some(false) => {}
        None => {
            // nothing queued: a steady colour needs no timer
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER);
            }
        }
    }
}

/// Writes the glow's bitmap for `color` to a PNG, to look at it (`--render-glow`).
pub fn render_png(color: [u8; 4], path: &str) {
    let mon = Mon { full: windows::Win32::Foundation::RECT { left: 0, top: 0, right: 960, bottom: 540 }, work: windows::Win32::Foundation::RECT { left: 0, top: 0, right: 960, bottom: 540 }, scale: 1.0 };
    let _ = render(&mon, color).save_png(path);
}
