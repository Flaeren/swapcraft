//! The queue card that opens from the tray icon: what is converting, what waits, what is done (click a finished
//! one to see it in its folder) and what failed and why. Drawn like the bar (SwapCraft Black), it closes when it
//! loses focus. The working bar is a sliding segment, not a percentage: the engines do not report progress.

use std::sync::Mutex;

use tiny_skia::{FillRule, Pixmap, Stroke, Transform};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use crate::gfx::{self, CARD, CHIP, FAINT, GREEN, INK, LINE, MUTED, Mon, ORANGE, RED, STRONG};
use crate::queue::{self, Counts, Entry, Status};

const W: f32 = 340.0;
const PAD: f32 = 14.0;
const HEAD_H: f32 = 46.0;
const ROW_H: f32 = 46.0;
const FOOT_H: f32 = 52.0;
const EMPTY_H: f32 = 52.0;
const MAX_ROWS: usize = 6;
const TIMER: usize = 1;
const BTN_OPEN: i32 = 100;
const BTN_CLEAR: i32 = 101;
const BTN_CANCEL: i32 = 102;

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

/// Running first, then the waiting ones in order, then what finished, newest first.
fn ordered(all: Vec<Entry>) -> Vec<Entry> {
    let mut running: Vec<Entry> = all.iter().filter(|e| e.status == Status::Running).cloned().collect();
    running.extend(all.iter().filter(|e| e.status == Status::Waiting).cloned());
    running.extend(all.iter().rev().filter(|e| !matches!(e.status, Status::Running | Status::Waiting)).cloned());
    running.truncate(MAX_ROWS);
    running
}

fn summary(c: &Counts, empty: bool) -> String {
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

struct Layout {
    height: f32,
    /// (hit id, x, y, w, h, label) in logical pixels.
    buttons: Vec<(i32, f32, f32, f32, f32, &'static str)>,
}

fn layout(rows: usize, c: &Counts, has_finished: bool) -> Layout {
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

fn render(scale: f32, rows: &[Entry], c: &Counts, has_finished: bool, hot: i32, phase: f32) -> Pixmap {
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

fn hit(lx: f32, ly: f32, rows: usize, c: &Counts, has_finished: bool) -> i32 {
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

fn current() -> (Vec<Entry>, Counts, bool) {
    let all = queue::snapshot();
    let finished = all.iter().any(|e| !matches!(e.status, Status::Running | Status::Waiting));
    (ordered(all), queue::counts(), finished)
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
