//! The drop bar on the screen edge, and the program's main window (the bar window also receives the raw mouse
//! input, the tray icon's messages and the queue's "something changed").
//!
//! Idle cost is zero: the process sleeps in GetMessage and is woken only by raw mouse input (RIDEV_INPUTSINK) and
//! by OLE drag-and-drop calls. A file drag is recognised by what Windows itself does: the left button went down on a
//! CLIENT area of Explorer or the Desktop (not a title bar), the cursor moved 8 px, and the shell then draws its drag
//! picture in a window of class `SysDragImage` (a window being moved, or a selection rectangle, never gets one).
//! While that is confirmed a worker thread reads the selection of the Explorer window (the front-most tab), so the
//! bar offers only formats the files do not already have; the drag's own data object corrects it before a drop.

use std::ffi::c_void;
use std::mem::size_of;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Instant;

use tiny_skia::{Color, FillRule, GradientStop, LinearGradient, Paint, Pixmap, Point, SpreadMode, Stroke, StrokeDash, Transform};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Input::*;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, Interface, PCWSTR, Ref, Result, implement, w};

use crate::gfx::{self, FAINT, INK, LINE, STRONG, VIOLET};
use crate::offer::{self, CHIP_BASE, Offer, Slot};
use crate::{aura, queue, tray};

// logical (96 dpi) sizes, multiplied by the monitor's scale
const BAR_W: f32 = 210.0;
const SLOT_X: f32 = 64.0;
const SLOT_W: f32 = 134.0;
const SLOT_H: f32 = 34.0;
const SLOT_GAP: f32 = 8.0;
const TITLE_H: f32 = 26.0;
const TITLE_GAP: f32 = 14.0;
const CHIP_W: f32 = 112.0;
const CHIP_H: f32 = 30.0;
const CHIP_GAP: f32 = 8.0;
/// Extra width when the "More" list is open: [ramp 40][2 columns of formats][gap] then the slots.
const PANEL_W: f32 = 224.0;
const PANEL_X: f32 = 40.0;

const TIMER_HIDE: usize = 1;
const TIMER_ANIM: usize = 2;
const TIMER_CONFIRM: usize = 3;
const TIMER_EXPAND: usize = 4;
const TIMER_PICK_TIMEOUT: usize = 5;
const TIMER_FADE: usize = 6;
/// How long the bar takes to fade out and slide back to the edge when it goes away.
const FADE_MS: f32 = 260.0;
/// What a format shows in place of its extension when its engine is not on this PC.
const MISSING: &str = "manca";
pub const WM_SELECTION: u32 = WM_APP + 1;
pub const WM_QUEUE: u32 = WM_APP + 2;
const MOVE_PX: i32 = 8;
const ANIM_MS: f32 = 190.0;
const CONFIRM_MAX_MS: u128 = 700;
const SELECTION_WAIT_MS: u128 = 250;
const ACCEPTED_MS: u32 = 900;

static MAIN: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED: AtomicIsize = AtomicIsize::new(0);

#[derive(Default)]
struct State {
    pressed: bool,
    candidate: bool,
    origin: (i32, i32),
    pending_since: Option<Instant>,
    fg_at_down: isize,
    sel_ready: bool,
    sel_exts: Vec<String>,
    shown: bool,
    pick_mode: bool,
    pick_paths: Vec<PathBuf>,
    expanded: bool,
    hot: i32,
    offer: Offer,
    /// After a drop that started conversions: how many files were taken.
    accepted: Option<usize>,
    // placement of the bar that is (or was last) on screen, in physical pixels (base = collapsed)
    scale_pct: u32,
    height: i32,
    base_x: i32,
    top: i32,
    anim_start: Option<Instant>,
    /// Set while the bar fades out (see `begin_hide`).
    fade_start: Option<Instant>,
}

static STATE: Mutex<State> = Mutex::new(State {
    pressed: false,
    candidate: false,
    origin: (0, 0),
    pending_since: None,
    fg_at_down: 0,
    sel_ready: false,
    sel_exts: Vec::new(),
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
});

fn st() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Tells the main window (from any thread) that the queue changed.
pub fn notify_queue() {
    let h = MAIN.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut c_void)), WM_QUEUE, WPARAM(0), LPARAM(0));
        }
    }
}

// ---------- what the system says about the drag ----------

fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 96];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

fn explorer_in_front() -> bool {
    let fg = unsafe { GetForegroundWindow() };
    matches!(class_of(fg).as_str(), "CabinetWClass" | "ExploreWClass" | "Progman" | "WorkerW")
}

/// WM_NCHITTEST of the window under the cursor: 1 = client area, 2 = title bar, others = borders and buttons.
fn hit_test(x: i32, y: i32) -> i32 {
    unsafe {
        let h = WindowFromPoint(POINT { x, y });
        if h.0.is_null() {
            return 0;
        }
        let mut result = 0usize;
        let lp = LPARAM(((y as u32 as isize) << 16) | (x as u32 as isize & 0xffff));
        let r = SendMessageTimeoutW(h, WM_NCHITTEST, WPARAM(0), lp, SMTO_ABORTIFHUNG, 50, Some(&mut result));
        if r.0 == 0 { 0 } else { result as i32 as i16 as i32 }
    }
}

unsafe extern "system" fn find_drag_image(hwnd: HWND, lp: LPARAM) -> BOOL {
    if unsafe { IsWindowVisible(hwnd) }.as_bool() && class_of(hwnd) == "SysDragImage" {
        unsafe { *(lp.0 as *mut bool) = true };
        return BOOL(0);
    }
    BOOL(1)
}

/// The shell draws the picture under the cursor in a window of this class for every real file drag.
fn drag_image_present() -> bool {
    let mut found = false;
    unsafe {
        let _ = EnumWindows(Some(find_drag_image), LPARAM(&mut found as *mut _ as isize));
    }
    found
}

// ---------- the selection of an Explorer window ----------

fn items_to_paths(items: &IShellItemArray) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let count = unsafe { items.GetCount() }.unwrap_or(0);
    for i in 0..count {
        if let Ok(item) = unsafe { items.GetItemAt(i) } {
            if let Ok(name) = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) } {
                let s = unsafe { name.to_string() };
                unsafe { CoTaskMemFree(Some(name.0 as *const c_void)) };
                if let Ok(s) = s {
                    out.push(PathBuf::from(s));
                }
            }
        }
    }
    out
}

/// The tab window (class ShellTabWindowClass) a shell view lives in, 0 for windows without tabs.
fn tab_window_of(view_hwnd: HWND) -> isize {
    let mut c = view_hwnd;
    for _ in 0..8 {
        if c.0.is_null() {
            break;
        }
        if class_of(c) == "ShellTabWindowClass" {
            return c.0 as isize;
        }
        c = unsafe { GetParent(c) }.unwrap_or_default();
    }
    0
}

/// The tab on top of an Explorer window: its front-most ShellTabWindowClass child. All tabs report visible and share
/// one rectangle (checked on a 3-tab window), and the window title names the same tab as the top of the z-order.
fn front_tab(top: HWND) -> isize {
    unsafe {
        let mut c = GetWindow(top, GW_CHILD).unwrap_or_default();
        for _ in 0..40 {
            if c.0.is_null() {
                break;
            }
            if class_of(c) == "ShellTabWindowClass" {
                return c.0 as isize;
            }
            c = GetWindow(c, GW_HWNDNEXT).unwrap_or_default();
        }
    }
    0
}

/// The selection of one shell window/tab and the tab window it belongs to (tabs share one top-level window).
/// An empty selection makes Items() fail with "element not found": that is an empty list, not an error.
fn selection_of(disp: &IDispatch) -> Result<(isize, Vec<PathBuf>)> {
    unsafe {
        let sp: IServiceProvider = disp.cast()?;
        let sb: IShellBrowser = sp.QueryService(&SID_STopLevelBrowser)?;
        let view = sb.QueryActiveShellView()?;
        let tab = view.GetWindow().map(tab_window_of).unwrap_or(0);
        let fv: IFolderView = view.cast()?;
        let paths = match fv.Items::<IShellItemArray>(SVGIO_SELECTION) {
            Ok(arr) => items_to_paths(&arr),
            Err(_) => Vec::new(),
        };
        Ok((tab, paths))
    }
}

/// Files selected in the Explorer window (or the Desktop) whose top-level window is `fg`.
fn read_selection(fg: isize) -> Vec<PathBuf> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let run = || -> Result<Vec<PathBuf>> {
            let sw: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL)?;
            let fg_h = HWND(fg as *mut c_void);
            if matches!(class_of(fg_h).as_str(), "Progman" | "WorkerW") {
                let empty = VARIANT::default();
                let mut h = 0i32;
                let disp = sw.FindWindowSW(&empty, &empty, SWC_DESKTOP, &mut h, SWFO_NEEDDISPATCH)?;
                return selection_of(&disp).map(|(_, p)| p);
            }
            // several tabs = several entries with the same top-level window: take the front-most tab
            let front = front_tab(fg_h);
            let mut fallback: Option<Vec<PathBuf>> = None;
            for i in 0..sw.Count()? {
                let Ok(disp) = sw.Item(&VARIANT::from(i)) else { continue };
                let Ok(wb) = disp.cast::<IWebBrowser2>() else { continue };
                if wb.HWND().map(|h| h.0 as isize).unwrap_or(0) != fg {
                    continue;
                }
                if let Ok((tab, paths)) = selection_of(&disp) {
                    if front == 0 || tab == front {
                        return Ok(paths);
                    }
                    fallback.get_or_insert(paths);
                }
            }
            Ok(fallback.unwrap_or_default())
        };
        let r = run().unwrap_or_default();
        CoUninitialize();
        r
    }
}

/// The files' types are known (or are not): choose what the bar offers.
fn apply_selection(exts: Vec<String>) {
    let o = if exts.is_empty() { offer::generic() } else { offer::offer(&exts) };
    let mut s = st();
    s.sel_exts = exts;
    s.offer = o;
}

// ---------- layout and drawing ----------

/// Everything the drawing and the hit test need, copied out of the state.
#[derive(Clone)]
struct View {
    scale: f32,
    h: i32,
    hot: i32,
    offer: Offer,
    expanded: bool,
    pick: bool,
    accepted: Option<usize>,
}

fn view() -> View {
    let s = st();
    View { scale: s.scale_pct as f32 / 100.0, h: s.height, hot: s.hot, offer: s.offer.clone(), expanded: s.expanded && !s.offer.more.is_empty(), pick: s.pick_mode, accepted: s.accepted }
}

impl View {
    fn extra(&self) -> f32 {
        if self.expanded { PANEL_W } else { 0.0 }
    }
    fn width_px(&self) -> i32 {
        ((BAR_W + self.extra()) * self.scale).round() as i32
    }
    fn first_slot_y(&self) -> f32 {
        let n = self.offer.slots.len();
        let total = TITLE_H + TITLE_GAP + n as f32 * SLOT_H + (n.max(1) - 1) as f32 * SLOT_GAP;
        (self.h as f32 - total * self.scale) / 2.0 + (TITLE_H + TITLE_GAP) * self.scale
    }
    fn title_top(&self) -> f32 {
        self.first_slot_y() - (TITLE_H + TITLE_GAP) * self.scale
    }
    fn slot_box(&self, i: usize) -> (f32, f32, f32, f32) {
        ((self.extra() + SLOT_X) * self.scale, self.first_slot_y() + i as f32 * (SLOT_H + SLOT_GAP) * self.scale, SLOT_W * self.scale, SLOT_H * self.scale)
    }
    fn chip_box(&self, k: usize) -> (f32, f32, f32, f32) {
        let (col, row) = ((k % 2) as f32, (k / 2) as f32);
        ((PANEL_X + col * (CHIP_W + CHIP_GAP)) * self.scale, self.first_slot_y() + row * (CHIP_H + CHIP_GAP) * self.scale, CHIP_W * self.scale, CHIP_H * self.scale)
    }
    /// The hot id under a point in window coordinates: a main slot index, `CHIP_BASE + k`, or -1.
    fn hit(&self, lx: f32, ly: f32) -> i32 {
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

fn render(v: &View) -> Pixmap {
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

fn eased_offset() -> i32 {
    let s = st();
    let Some(t0) = s.anim_start else { return 0 };
    let t = (t0.elapsed().as_secs_f32() * 1000.0 / ANIM_MS).min(1.0);
    let e = 1.0 - (1.0 - t).powi(3);
    ((1.0 - e) * 90.0 * s.scale_pct as f32 / 100.0) as i32
}

/// Window left edge for the current state: the base position, moved left when the list is open, plus the slide.
fn window_x(v: &View) -> i32 {
    // the lock is released before eased_offset() takes it again (a guard kept in the same expression would deadlock)
    let base = st().base_x;
    base - (v.extra() * v.scale).round() as i32 + eased_offset()
}

fn redraw(hwnd: HWND) {
    if st().fade_start.is_some() {
        return; // the fade draws the frames until the bar is gone
    }
    let v = view();
    let top = st().top;
    gfx::push(hwnd, &render(&v), window_x(&v), top, 255);
}

// ---------- show / hide ----------

fn show(hwnd: HWND) {
    let (cx, cy) = gfx::cursor();
    let mon = gfx::monitor_at(cx, cy);
    let width = (BAR_W * mon.scale).round() as i32;
    {
        let mut s = st();
        s.scale_pct = (mon.scale * 100.0).round() as u32;
        s.height = mon.work.bottom - mon.work.top;
        s.base_x = if gfx::offscreen() { mon.work.right + 5000 } else { mon.work.right - width };
        s.top = mon.work.top;
        s.hot = -1;
        s.shown = true;
        s.expanded = false;
        s.accepted = None;
        s.fade_start = None; // a new drag while the last bar was still fading
        s.anim_start = Some(Instant::now());
        if s.offer.is_empty() && s.sel_exts.is_empty() {
            s.offer = offer::generic();
        }
    }
    redraw(hwnd);
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        SetTimer(Some(hwnd), TIMER_ANIM, 15, None);
    }
    let s = st();
    gfx::log(&format!("SHOW at ({cx},{cy}) selection={:?} slots={:?}", s.sel_exts, s.offer.slots.iter().map(|x| x.label.clone()).collect::<Vec<_>>()));
}

/// The gentle way out: the bar fades and slides back to the edge, then `hide` takes it away.
fn begin_hide(hwnd: HWND) {
    {
        let mut s = st();
        if !s.shown || s.fade_start.is_some() {
            return;
        }
        s.shown = false; // a new drag can bring the bar back at once
        s.fade_start = Some(Instant::now());
    }
    unsafe {
        for t in [TIMER_HIDE, TIMER_ANIM, TIMER_CONFIRM, TIMER_EXPAND, TIMER_PICK_TIMEOUT] {
            let _ = KillTimer(Some(hwnd), t);
        }
        SetTimer(Some(hwnd), TIMER_FADE, 15, None);
    }
}

/// One frame of the fade: less opaque and a little further to the right each time, `hide` at the end.
fn fade_tick(hwnd: HWND) {
    let Some(t0) = st().fade_start else {
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_FADE);
        }
        return;
    };
    let t = (t0.elapsed().as_secs_f32() * 1000.0 / FADE_MS).min(1.0);
    if t >= 1.0 {
        gfx::log(&format!("bar faded out in {} ms", t0.elapsed().as_millis()));
        hide(hwnd);
        return;
    }
    let e = t * t * (3.0 - 2.0 * t); // smoothstep
    let v = view();
    let (top, scale) = {
        let s = st();
        (s.top, s.scale_pct as f32 / 100.0)
    };
    let x = window_x(&v) + (e * 36.0 * scale) as i32;
    gfx::push(hwnd, &render(&v), x, top, (255.0 * (1.0 - e)) as u8);
}

fn hide(hwnd: HWND) {
    unsafe {
        for t in [TIMER_HIDE, TIMER_ANIM, TIMER_CONFIRM, TIMER_EXPAND, TIMER_PICK_TIMEOUT, TIMER_FADE] {
            let _ = KillTimer(Some(hwnd), t);
        }
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
    let mut s = st();
    s.shown = false;
    s.hot = -1;
    s.expanded = false;
    s.pick_mode = false;
    s.accepted = None;
    s.pick_paths.clear();
    s.anim_start = None;
    s.fade_start = None;
    s.offer = Offer::default();
    s.sel_ready = false;
    s.sel_exts.clear();
    s.pending_since = None;
}

fn slot_at(hwnd: HWND, x: i32, y: i32) -> i32 {
    let mut r = RECT::default();
    let _ = unsafe { GetWindowRect(hwnd, &mut r) };
    view().hit((x - r.left) as f32, (y - r.top) as f32)
}

fn set_hot(hwnd: HWND, hot: i32) {
    let changed = std::mem::replace(&mut st().hot, hot) != hot;
    if !changed {
        return;
    }
    redraw(hwnd);
    // hovering "More" for a moment opens the full list
    let v = view();
    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_EXPAND);
        if hot >= 0 && hot < CHIP_BASE && v.offer.is_more_slot(hot as usize) && !v.expanded {
            SetTimer(Some(hwnd), TIMER_EXPAND, 260, None);
        }
    }
}

fn expand(hwnd: HWND) {
    {
        let mut s = st();
        if s.offer.more.is_empty() || s.expanded {
            return;
        }
        s.expanded = true;
    }
    redraw(hwnd);
    gfx::log("EXPAND (full list opened)");
}

/// The files were handed to the queue: show it on the bar for a moment, then take the bar away.
fn accept(hwnd: HWND, hot: i32, taken: usize) {
    {
        let mut s = st();
        s.accepted = Some(taken);
        s.hot = hot;
        s.pick_mode = false;
        s.expanded = s.expanded && hot >= CHIP_BASE;
    }
    redraw(hwnd);
    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_PICK_TIMEOUT);
        let _ = KillTimer(Some(hwnd), TIMER_EXPAND);
        SetTimer(Some(hwnd), TIMER_HIDE, ACCEPTED_MS, None);
    }
}

// ---------- drop target ----------

fn dragged_paths(data: &IDataObject) -> Vec<PathBuf> {
    match unsafe { SHCreateShellItemArrayFromDataObject::<_, IShellItemArray>(data) } {
        Ok(items) => items_to_paths(&items),
        Err(_) => Vec::new(),
    }
}

#[implement(IDropTarget)]
struct Target {
    hwnd: HWND,
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(&self, data: Ref<'_, IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        // the data object is the authority: if the files are not what the selection said, offer the right formats
        if let Some(d) = data.as_ref() {
            let exts = offer::exts_of(&dragged_paths(d));
            if exts != st().sel_exts {
                gfx::log(&format!("DRAGENTER corrects the formats: {exts:?}"));
                apply_selection(exts);
                st().sel_ready = true;
                redraw(self.hwnd);
            }
        }
        unsafe { *effect = DROPEFFECT_COPY };
        Ok(())
    }
    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let hot = slot_at(self.hwnd, pt.x, pt.y);
        set_hot(self.hwnd, hot);
        unsafe { *effect = if hot >= 0 { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        Ok(())
    }
    fn DragLeave(&self) -> Result<()> {
        set_hot(self.hwnd, -1);
        Ok(())
    }
    fn Drop(&self, data: Ref<'_, IDataObject>, _keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let id = slot_at(self.hwnd, pt.x, pt.y);
        let v = view();
        let paths = data.as_ref().map(dragged_paths).unwrap_or_default();
        if id >= 0 && id < CHIP_BASE && v.offer.is_more_slot(id as usize) {
            // dropped on "More": keep the files, keep the list open, wait for a click on a format
            gfx::log(&format!("DROP on More: choose a format (files={})", paths.len()));
            {
                let mut s = st();
                s.pick_mode = true;
                s.pick_paths = paths;
                s.expanded = true;
                s.hot = -1;
            }
            redraw(self.hwnd);
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_HIDE);
                SetTimer(Some(self.hwnd), TIMER_PICK_TIMEOUT, 10_000, None);
                *effect = DROPEFFECT_COPY;
            }
            return Ok(());
        }
        match v.offer.pick(id) {
            Some(slot) => {
                let taken = queue::submit(&paths, &slot.id, &slot.label);
                gfx::log(&format!("DROP label={} files={} taken={taken} {:?}", slot.label, paths.len(), paths));
                if taken > 0 {
                    unsafe { *effect = DROPEFFECT_COPY };
                    accept(self.hwnd, id, taken);
                    return Ok(());
                }
                unsafe { *effect = DROPEFFECT_NONE };
            }
            None => {
                gfx::log(&format!("DROP outside any format ({} files): nothing done", paths.len()));
                unsafe { *effect = DROPEFFECT_NONE };
            }
        }
        begin_hide(self.hwnd);
        Ok(())
    }
}

// ---------- the window ----------

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_INPUT => {
            let mut ri: RAWINPUT = unsafe { std::mem::zeroed() };
            let mut size = size_of::<RAWINPUT>() as u32;
            let got = unsafe { GetRawInputData(HRAWINPUT(lp.0 as *mut c_void), RID_INPUT, Some(&mut ri as *mut _ as *mut c_void), &mut size, size_of::<RAWINPUTHEADER>() as u32) };
            if got != u32::MAX && ri.header.dwType == RIM_TYPEMOUSE.0 {
                let flags = unsafe { ri.data.mouse.Anonymous.Anonymous.usButtonFlags } as u32;
                if flags & RI_MOUSE_LEFT_BUTTON_DOWN != 0 {
                    let (x, y) = gfx::cursor();
                    let (shown, pick) = {
                        let s = st();
                        (s.shown, s.pick_mode)
                    };
                    // a click outside the open list closes it
                    if shown && pick {
                        let mut r = RECT::default();
                        let _ = unsafe { GetWindowRect(hwnd, &mut r) };
                        if x < r.left || x >= r.right || y < r.top || y >= r.bottom {
                            begin_hide(hwnd);
                        }
                    }
                    let front = explorer_in_front();
                    let hit = if front { hit_test(x, y) } else { 0 };
                    let mut s = st();
                    s.pressed = true;
                    s.candidate = front && hit == 1; // HTCLIENT: not a title bar, not a border
                    s.origin = (x, y);
                    s.fg_at_down = unsafe { GetForegroundWindow() }.0 as isize;
                    s.pending_since = None;
                } else if flags & RI_MOUSE_LEFT_BUTTON_UP != 0 {
                    let (shown, pick, was_pending) = {
                        let mut s = st();
                        s.pressed = false;
                        s.candidate = false;
                        (s.shown, s.pick_mode, s.pending_since.take().is_some())
                    };
                    unsafe {
                        let _ = KillTimer(Some(hwnd), TIMER_CONFIRM);
                    }
                    if was_pending && !shown {
                        gfx::log("cancel: button released before the drag was confirmed");
                    }
                    if shown && !pick && st().accepted.is_none() {
                        // the drop, if any, is delivered just before or after this; hide a moment later either way
                        unsafe { SetTimer(Some(hwnd), TIMER_HIDE, 350, None) };
                    }
                } else {
                    let mut s = st();
                    if s.pressed && s.candidate && !s.shown && s.pending_since.is_none() {
                        let (x, y) = gfx::cursor();
                        if (x - s.origin.0).abs() >= MOVE_PX || (y - s.origin.1).abs() >= MOVE_PX {
                            s.pending_since = Some(Instant::now());
                            s.sel_ready = false;
                            s.sel_exts.clear();
                            s.offer = Offer::default();
                            let fg = s.fg_at_down;
                            drop(s);
                            // read the selection off the UI thread: Explorer may be slow to answer
                            let me = hwnd.0 as isize;
                            std::thread::spawn(move || {
                                let paths = read_selection(fg);
                                apply_selection(offer::exts_of(&paths));
                                st().sel_ready = true;
                                unsafe {
                                    let _ = PostMessageW(Some(HWND(me as *mut c_void)), WM_SELECTION, WPARAM(0), LPARAM(0));
                                }
                            });
                            unsafe { SetTimer(Some(hwnd), TIMER_CONFIRM, 30, None) };
                        }
                    }
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_SELECTION => {
            // the selection arrived after the bar was already shown: switch its formats
            if st().shown {
                redraw(hwnd);
            }
            LRESULT(0)
        }
        WM_QUEUE => {
            tray::on_queue(hwnd);
            LRESULT(0)
        }
        tray::WM_TRAY => {
            tray::on_tray(hwnd, lp);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            // click mode (after a drop on "More")
            if st().pick_mode {
                let (x, y) = (lp.0 as i16 as i32 as f32, (lp.0 >> 16) as i16 as i32 as f32);
                let v = view();
                let id = v.hit(x, y);
                if let Some(slot) = v.offer.pick(id) {
                    let files = st().pick_paths.clone();
                    let taken = queue::submit(&files, &slot.id, &slot.label);
                    gfx::log(&format!("PICK label={} files={} taken={taken}", slot.label, files.len()));
                    if taken > 0 {
                        accept(hwnd, id, taken);
                    } else {
                        begin_hide(hwnd);
                    }
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if st().pick_mode {
                let (x, y) = (lp.0 as i16 as i32 as f32, (lp.0 >> 16) as i16 as i32 as f32);
                set_hot(hwnd, view().hit(x, y));
            }
            LRESULT(0)
        }
        WM_TIMER => {
            match wp.0 {
                TIMER_HIDE | TIMER_PICK_TIMEOUT => begin_hide(hwnd),
                TIMER_FADE => fade_tick(hwnd),
                TIMER_EXPAND => {
                    unsafe {
                        let _ = KillTimer(Some(hwnd), TIMER_EXPAND);
                    }
                    expand(hwnd);
                }
                TIMER_ANIM => {
                    let v = view();
                    let (top, done) = {
                        let s = st();
                        (s.top, s.anim_start.map(|t| t.elapsed().as_secs_f32() * 1000.0 >= ANIM_MS).unwrap_or(true))
                    };
                    unsafe {
                        let _ = SetWindowPos(hwnd, None, window_x(&v), top, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER);
                        if done {
                            let _ = KillTimer(Some(hwnd), TIMER_ANIM);
                        }
                    }
                }
                TIMER_CONFIRM => confirm_tick(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_CLOSE => {
            tray::remove_icon(hwnd);
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) as u32 => {
            tray::add_icon(hwnd); // Explorer restarted: the icon is gone
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// While a drag is not confirmed: wait for the shell's drag picture, and for the selection (at most 250 ms).
fn confirm_tick(hwnd: HWND) {
    let (pending, ready, shown) = {
        let s = st();
        (s.pending_since, s.sel_ready, s.shown)
    };
    let kill = || unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_CONFIRM);
    };
    let Some(since) = pending else {
        kill();
        return;
    };
    let ms = since.elapsed().as_millis();
    if !shown && drag_image_present() && (ready || ms >= SELECTION_WAIT_MS) {
        kill();
        let unsupported = {
            let s = st();
            ready && !s.sel_exts.is_empty() && s.offer.is_empty()
        };
        if unsupported {
            // files SwapCraft cannot convert: no bar, no false hope
            gfx::log(&format!("skip: nothing to offer for {:?}", st().sel_exts));
            let mut s = st();
            s.candidate = false;
            s.pending_since = None;
        } else {
            show(hwnd);
        }
    } else if !shown && ms >= CONFIRM_MAX_MS {
        kill();
        let mut s = st();
        s.candidate = false;
        s.pending_since = None;
        drop(s);
        gfx::log("skip: no drag image appeared (a selection rectangle, not a file drag)");
    }
}

fn make_window() -> Result<HWND> {
    unsafe {
        let hi = GetModuleHandleW(None)?;
        let class = w!("SwapCraftTray");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hi.into(), lpszClassName: class, hCursor: LoadCursorW(None, IDC_ARROW)?, ..Default::default() };
        RegisterClassW(&wc);
        let ex = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED;
        let hwnd = CreateWindowExW(ex, class, w!("SwapCraft"), WS_POPUP, 0, 0, 10, 10, None, None, Some(hi.into()), None)?;
        MAIN.store(hwnd.0 as isize, Ordering::Relaxed);
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")) as isize, Ordering::Relaxed);
        queue::on_change(notify_queue);
        let target: IDropTarget = Target { hwnd }.into();
        RegisterDragDrop(hwnd, &target)?;
        std::mem::forget(target); // lives as long as the window
        Ok(hwnd)
    }
}

fn pump() {
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn pump_for(ms: u128) {
    let t0 = Instant::now();
    while t0.elapsed().as_millis() < ms {
        let mut msg = MSG::default();
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// The resident program: bar window, raw mouse input, tray icon, message loop.
pub fn run() -> Result<()> {
    let hwnd = make_window()?;
    let dev = RAWINPUTDEVICE { usUsagePage: 0x01, usUsage: 0x02, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd };
    unsafe { RegisterRawInputDevices(&[dev], size_of::<RAWINPUTDEVICE>() as u32)? };
    tray::add_icon(hwnd);
    gfx::log("resident: waiting");
    pump();
    Ok(())
}

// ---------- tests without a mouse ----------

fn data_object_for(file: &str) -> Result<IDataObject> {
    let wide: Vec<u16> = file.encode_utf16().chain(std::iter::once(0)).collect();
    let mut pidl = std::ptr::null_mut();
    unsafe { SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None)? };
    let items = [pidl as *const ITEMIDLIST];
    let array = unsafe { SHCreateShellItemArrayFromIDLists(&items)? };
    unsafe { array.BindToHandler(None, &BHID_DataObject) }
}

/// Writes the bar for files of type `ext` to a PNG, to look at it (`--render-bar`). With `missing_from`, the formats
/// from that position on (the main slots, then the "More" list) are drawn as if their engine were not installed.
pub fn render_png(path: &str, ext: &str, missing_from: Option<usize>) {
    let mut o = offer::offer(&[ext.to_owned()]);
    if let Some(n) = missing_from {
        for (i, s) in o.slots.iter_mut().chain(o.more.iter_mut()).enumerate() {
            s.available = i < n;
        }
    }
    let expanded = !o.more.is_empty();
    let v = View { scale: 1.0, h: 720, hot: -1, offer: o, expanded, pick: false, accepted: None };
    let _ = render(&v).save_png(path);
}

/// Feeds a real shell data object for `file` to the bar as a drop on the format `to_id`, as Explorer would, then
/// waits for the queue to empty and reports. Everything stays off-screen.
pub fn selftest(file: &str, to_id: &str) -> Result<()> {
    gfx::FORCE_OFFSCREEN.store(true, Ordering::Relaxed);
    let hwnd = make_window()?;
    let target: IDropTarget = Target { hwnd }.into();
    let data = data_object_for(file)?;
    apply_selection(offer::exts_of(&[PathBuf::from(file)]));
    st().sel_ready = true;
    show(hwnd);
    pump_for(400); // let the slide-in finish, like a person who takes a moment to reach the bar
    let v = view();
    println!("offer for {file}: slots={:?} more={:?}", v.offer.slots.iter().map(|s| &s.label).collect::<Vec<_>>(), v.offer.more.iter().map(|s| &s.label).collect::<Vec<_>>());
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r)? };
    let center = |b: (f32, f32, f32, f32), r: &RECT| POINTL { x: r.left + (b.0 + b.2 / 2.0) as i32, y: r.top + (b.1 + b.3 / 2.0) as i32 };
    let mut effect = DROPEFFECT_COPY;
    let in_main = v.offer.slots.iter().position(|s| s.id == to_id);
    let in_more = v.offer.more.iter().position(|s| s.id == to_id);
    let pt = unsafe {
        if let Some(i) = in_main {
            center(v.slot_box(i), &r)
        } else if let Some(k) = in_more {
            // hover "More" until the list opens, then aim at the format in it
            let pm = center(v.slot_box(v.offer.slots.len() - 1), &r);
            target.DragEnter(&data, MODIFIERKEYS_FLAGS(0), pm, &mut effect)?;
            target.DragOver(MODIFIERKEYS_FLAGS(0), pm, &mut effect)?;
            pump_for(450);
            GetWindowRect(hwnd, &mut r)?;
            center(view().chip_box(k), &r)
        } else {
            println!("the bar does not offer {to_id} for this file");
            return Ok(());
        }
    };
    unsafe {
        target.DragEnter(&data, MODIFIERKEYS_FLAGS(0), pt, &mut effect)?;
        target.DragOver(MODIFIERKEYS_FLAGS(0), pt, &mut effect)?;
        println!("hot={} effect={}", view().hot, effect.0);
        target.Drop(&data, MODIFIERKEYS_FLAGS(0), pt, &mut effect)?;
    }
    println!("after drop: accepted={:?} bar_visible={}", st().accepted, unsafe { IsWindowVisible(hwnd) }.as_bool());
    let t0 = Instant::now();
    let mut last = String::new();
    while t0.elapsed().as_secs() < 240 {
        pump_for(200);
        let c = queue::counts();
        let line = format!("queue: waiting={} running={} done={} failed={} glow={}", c.waiting, c.running, c.done, c.failed, aura::describe());
        if line != last {
            println!("[{:>5} ms] {line}", t0.elapsed().as_millis());
            last = line;
        }
        if c.active() == 0 && st().accepted.is_none() && t0.elapsed().as_millis() > 2500 {
            break;
        }
    }
    for e in queue::snapshot() {
        println!("entry: {} -> {} : {:?}", e.name, e.to, e.status);
    }
    pump_for(3500); // the glow's green phase and its fade-out
    println!("glow at the end: {}", aura::describe());
    Ok(())
}
