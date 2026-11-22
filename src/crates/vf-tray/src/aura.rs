//! The glow along the screen edges, Windows side: a transparent, click-through window the size of the monitor the
//! person was using, and a timer that runs only while the glow moves. What the glow does (colours, fades, holds)
//! is in `glow.rs`, shared with the other systems.

use std::sync::Mutex;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use crate::draw::Mon;
use crate::gfx;
use crate::glow::{self, Glow, Tick};

const TIMER: usize = 1;
const FRAME_MS: u32 = 16;

struct State {
    hwnd: isize,
    glow: Glow,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(|| State { hwnd: 0, glow: Glow::new() }))
}

/// The monitor the person is on now.
fn current_monitor() -> Mon {
    let (x, y) = gfx::cursor();
    gfx::monitor_at(x, y)
}

/// The glow's own state, as words, for the self-test.
pub fn describe() -> String {
    with(|s| s.glow.describe())
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
    let hwnd = ensure_window();
    if with(|s| s.glow.working(current_monitor)) {
        start_timer(hwnd);
    }
}

/// The work is over: orange out, then green (or red) in, hold, and out.
pub fn finish(ok: bool) {
    let hwnd = ensure_window();
    if with(|s| s.glow.finish(ok, current_monitor)) {
        start_timer(hwnd);
    }
}

/// The work was cancelled or left nothing to report: fade out without announcing anything.
pub fn stop() {
    let hwnd = ensure_window();
    if with(|s| s.glow.stop()) {
        start_timer(hwnd);
    }
}

fn tick(hwnd: HWND) {
    let result = with(|s| {
        let result = s.glow.tick();
        // draw this frame
        if let Some((pm, x, y, alpha)) = s.glow.frame() {
            let off = if gfx::offscreen() { 6000 } else { 0 };
            gfx::push(hwnd, pm, x + off, y, alpha);
            unsafe {
                let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            }
        }
        if result == Tick::Off {
            s.glow.clear(); // the monitor-sized bitmap is not kept while the glow is off
        }
        result
    });
    match result {
        Tick::Off => unsafe {
            let _ = KillTimer(Some(hwnd), TIMER);
            let _ = ShowWindow(hwnd, SW_HIDE);
        },
        // nothing queued: a steady colour needs no timer
        Tick::Idle => unsafe {
            let _ = KillTimer(Some(hwnd), TIMER);
        },
        Tick::Running => {}
    }
}

/// Writes the glow's bitmap for `color` to a PNG, to look at it (`--render-glow`).
pub fn render_png(color: [u8; 4], path: &str) {
    let r = gfx::Rect { left: 0, top: 0, right: 960, bottom: 540 };
    let mon = Mon { full: r, work: r, scale: 1.0 };
    let _ = glow::render(&mon, color).save_png(path);
}
