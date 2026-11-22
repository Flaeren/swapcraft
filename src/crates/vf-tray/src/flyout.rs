//! The queue card that opens from the tray icon: what is converting, what waits, what is done (click a finished
//! one to see it in its folder) and what failed and why. Drawn like the bar (SwapCraft Black), it closes when it
//! loses focus. The working bar is a sliding segment, not a percentage: the engines do not report progress.

use std::sync::Mutex;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use crate::card_view::*;
use crate::gfx::{self, Mon};
use crate::queue::{self, Entry, Status};

const TIMER: usize = 1;

struct State {
    hwnd: isize,
    visible: bool,
    mon: Option<Mon>,
    hot: i32,
    phase: f32,
    /// The rows as drawn (newest state), for the hit test.
    rows: Vec<Entry>,
}

static STATE: Mutex<State> = Mutex::new(State { hwnd: 0, visible: false, mon: None, hot: -1, phase: 0.0, rows: Vec::new() });

fn st() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn is_visible() -> bool {
    st().visible
}


/// Draws the card for the current queue and places it above the taskbar corner of its monitor.
fn redraw(hwnd: HWND) {
    let (rows, c, finished) = current();
    let (mon, hot, phase) = {
        let mut s = st();
        s.rows = rows.clone();
        (s.mon, s.hot, s.phase)
    };
    let Some(mon) = mon else { return };
    let pm = render(mon.scale, &rows, &c, finished, hot, phase);
    let off = if gfx::offscreen() { 6000 } else { 0 };
    let x = mon.work.right - pm.width() as i32 - (12.0 * mon.scale) as i32 + off;
    let y = mon.work.bottom - pm.height() as i32 - (12.0 * mon.scale) as i32;
    gfx::push(hwnd, &pm, x, y, 255);
    // the bar slides while something converts
    unsafe {
        if c.running > 0 {
            SetTimer(Some(hwnd), TIMER, 33, None);
        } else {
            let _ = KillTimer(Some(hwnd), TIMER);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ACTIVATE => {
            if (wp.0 & 0xffff) == 0 {
                hide();
            }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == TIMER => {
            st().phase += 0.03;
            redraw(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = (lp.0 as i16 as i32 as f32, (lp.0 >> 16) as i16 as i32 as f32);
            let (c, finished) = (queue::counts(), queue::snapshot().iter().any(|e| !matches!(e.status, Status::Running | Status::Waiting)));
            let (rows, scale) = {
                let s = st();
                (s.rows.len(), s.mon.map_or(1.0, |m| m.scale))
            };
            let h = hit(x / scale, y / scale, rows, &c, finished);
            let changed = {
                let mut s = st();
                std::mem::replace(&mut s.hot, h) != h
            };
            if changed {
                redraw(hwnd);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = (lp.0 as i16 as i32 as f32, (lp.0 >> 16) as i16 as i32 as f32);
            let (c, finished) = (queue::counts(), queue::snapshot().iter().any(|e| !matches!(e.status, Status::Running | Status::Waiting)));
            let (rows, scale) = {
                let s = st();
                (s.rows.clone(), s.mon.map_or(1.0, |m| m.scale))
            };
            let id = hit(x / scale, y / scale, rows.len(), &c, finished);
            match id {
                BTN_OPEN => {
                    open_hub();
                    hide();
                }
                BTN_CLEAR => queue::clear_finished(),
                BTN_CANCEL => queue::cancel_all(),
                i if i >= 0 => {
                    if let Some(Entry { status: Status::Done(p), .. }) = rows.get(i as usize) {
                        queue::reveal(p);
                        hide();
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_ACTIVATE as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Starts the SwapCraft hub that sits next to this program, if there is one.
fn open_hub() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };
    // SwapCraft.exe / voidflare.exe in the portable folder, voidflare-hub.exe in a build folder
    for name in ["SwapCraft.exe", "voidflare.exe", "voidflare-hub.exe"] {
        let p = dir.join(name);
        if p.is_file() {
            if let Err(e) = std::process::Command::new(&p).spawn() {
                gfx::log(&format!("open hub failed: {} ({e})", p.display()));
            }
            return;
        }
    }
    gfx::log(&format!("open hub: no SwapCraft.exe, voidflare.exe or voidflare-hub.exe in {}", dir.display()));
}

fn ensure_window() -> HWND {
    let existing = st().hwnd;
    if existing != 0 {
        return HWND(existing as *mut _);
    }
    let hwnd = unsafe {
        let hi = GetModuleHandleW(None).expect("module");
        let class = w!("SwapCraftQueue");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hi.into(), lpszClassName: class, hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), ..Default::default() };
        RegisterClassW(&wc);
        CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED, class, w!("SwapCraft queue"), WS_POPUP, 0, 0, 10, 10, None, None, Some(hi.into()), None).expect("queue window")
    };
    st().hwnd = hwnd.0 as isize;
    hwnd
}

/// Opens (or closes) the card on the monitor the cursor is on.
pub fn toggle() {
    if is_visible() {
        hide();
        return;
    }
    let hwnd = ensure_window();
    let (x, y) = gfx::cursor();
    {
        let mut s = st();
        s.mon = Some(gfx::monitor_at(x, y));
        s.visible = true;
        s.hot = -1;
    }
    redraw(hwnd);
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
        let _ = SetForegroundWindow(hwnd);
    }
}

pub fn hide() {
    let hwnd = {
        let mut s = st();
        if !s.visible {
            return;
        }
        s.visible = false;
        s.hwnd
    };
    unsafe {
        let h = HWND(hwnd as *mut _);
        let _ = KillTimer(Some(h), TIMER);
        let _ = ShowWindow(h, SW_HIDE);
    }
}

/// The queue changed: redraw the card if it is open.
pub fn refresh() {
    if !is_visible() {
        return;
    }
    // the lock must be released before `redraw` takes it again
    let hwnd = st().hwnd;
    redraw(HWND(hwnd as *mut _));
}

/// Writes the card for the current queue to a PNG, to look at it (`--render-queue`).
pub fn render_png(path: &str) {
    let (rows, c, finished) = current();
    let _ = render(1.0, &rows, &c, finished, -1, 0.35).save_png(path);
}
