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

use crate::bar_view::*;
use crate::gfx;
use crate::model::{ACCEPTED_MS, BarModel, Drop, PICK_TIMEOUT_MS};
use crate::offer;
use crate::{aura, queue, tray};


const TIMER_HIDE: usize = 1;
const TIMER_ANIM: usize = 2;
const TIMER_CONFIRM: usize = 3;
const TIMER_EXPAND: usize = 4;
const TIMER_PICK_TIMEOUT: usize = 5;
const TIMER_FADE: usize = 6;
pub const WM_SELECTION: u32 = WM_APP + 1;
pub const WM_QUEUE: u32 = WM_APP + 2;
const MOVE_PX: i32 = 8;
const CONFIRM_MAX_MS: u128 = 700;
const SELECTION_WAIT_MS: u128 = 250;

static MAIN: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED: AtomicIsize = AtomicIsize::new(0);

/// What this system's part knows (how the drag was recognised, what the selection was), and the bar's own state,
/// which is the shared model (`model.rs`).
struct State {
    pressed: bool,
    candidate: bool,
    origin: (i32, i32),
    pending_since: Option<Instant>,
    fg_at_down: isize,
    sel_ready: bool,
    sel_exts: Vec<String>,
    /// Which press this is, and whether it was on a file item (`item_at`); the answer comes a moment after the press.
    press_id: u64,
    item_hit: Option<bool>,
    /// How long after the pointer started to move the shell's drag picture was first seen (for the log).
    image_at: Option<u128>,
    m: BarModel,
}

static STATE: Mutex<State> = Mutex::new(State {
    pressed: false,
    candidate: false,
    origin: (0, 0),
    pending_since: None,
    fg_at_down: 0,
    sel_ready: false,
    sel_exts: Vec::new(),
    press_id: 0,
    item_hit: None,
    image_at: None,
    m: BarModel::new(),
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

/// Whether the point is inside the client area of `top` (all but its title bar and borders), worked out from the
/// window's own rectangles: nothing is sent to the program that owns it.
fn in_client_area(top: HWND, x: i32, y: i32) -> bool {
    unsafe {
        let mut rc = RECT::default();
        if GetClientRect(top, &mut rc).is_err() {
            return false;
        }
        let mut origin = POINT { x: 0, y: 0 };
        if !windows::Win32::Graphics::Gdi::ClientToScreen(top, &mut origin).as_bool() {
            return false;
        }
        x >= origin.x && x < origin.x + rc.right && y >= origin.y && y < origin.y + rc.bottom
    }
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

/// Whether the point is on a file or folder item of an Explorer window (a list item, a details row, a tree item),
/// asked of Windows' UI Automation, which Explorer answers within a few milliseconds. A press on an item followed by
/// a move is a file drag by itself: Windows draws its drag picture only a while later (long for a big photo or a
/// video, whose thumbnail it prepares first), and a quick drag was over before it came. `None`: it could not be asked.
fn item_at(x: i32, y: i32) -> Option<bool> {
    use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, UIA_DataItemControlTypeId, UIA_ListItemControlTypeId, UIA_TreeItemControlTypeId};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let ask = || -> Result<bool> {
            let auto: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let mut e = auto.ElementFromPoint(POINT { x, y })?;
            let walker = auto.ControlViewWalker()?;
            // the element under the pointer may be a part of the item (its icon, its name): look a few levels up
            for _ in 0..4 {
                let t = e.CurrentControlType()?;
                if t == UIA_ListItemControlTypeId || t == UIA_DataItemControlTypeId || t == UIA_TreeItemControlTypeId {
                    return Ok(true);
                }
                match walker.GetParentElement(&e) {
                    Ok(p) => e = p,
                    Err(_) => break,
                }
            }
            Ok(false)
        };
        let r = ask();
        CoUninitialize();
        r.ok()
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
fn apply_selection(paths: &[PathBuf]) {
    let exts = offer::exts_of(paths);
    let o = if exts.is_empty() { offer::generic() } else { offer::offer(&exts) };
    let mut s = st();
    s.sel_exts = exts;
    s.m.offer = o;
    s.m.set_files(paths.iter().filter(|p| p.is_file()).count());
}

fn view() -> View {
    st().m.view()
}

/// Window left edge for the current state: the base position, moved left when the list is open, plus the slide.
fn window_x(v: &View) -> i32 {
    st().m.window_x(v, Instant::now())
}

fn redraw(hwnd: HWND) {
    if st().m.fade_start.is_some() {
        return; // the fade draws the frames until the bar is gone
    }
    let v = view();
    let top = st().m.top;
    gfx::push(hwnd, &render(&v), window_x(&v), top, 255);
}

// ---------- show / hide ----------

fn show(hwnd: HWND) {
    let (cx, cy) = gfx::cursor();
    let mon = gfx::monitor_at(cx, cy);
    {
        let mut s = st();
        let known = !s.sel_exts.is_empty();
        s.m.begin_show(&mon, gfx::offscreen(), known, Instant::now());
    }
    redraw(hwnd);
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        SetTimer(Some(hwnd), TIMER_ANIM, 15, None);
    }
    let s = st();
    gfx::log(&format!("SHOW at ({cx},{cy}) selection={:?} slots={:?}", s.sel_exts, s.m.offer.slots.iter().map(|x| x.label.clone()).collect::<Vec<_>>()));
}

/// The gentle way out: the bar fades and slides back to the edge, then `hide` takes it away.
fn begin_hide(hwnd: HWND) {
    if !st().m.begin_fade(Instant::now()) {
        return;
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
    let now = Instant::now();
    let (fade, started, top) = {
        let s = st();
        (s.m.fade(now), s.m.fade_start, s.m.top)
    };
    let Some(fade) = fade else {
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_FADE);
        }
        return;
    };
    if fade.done {
        gfx::log(&format!("bar faded out in {} ms", started.map_or(0, |t| t.elapsed().as_millis())));
        hide(hwnd);
        return;
    }
    let v = view();
    gfx::push(hwnd, &render(&v), window_x(&v) + fade.dx, top, fade.alpha);
}

fn hide(hwnd: HWND) {
    unsafe {
        for t in [TIMER_HIDE, TIMER_ANIM, TIMER_CONFIRM, TIMER_EXPAND, TIMER_PICK_TIMEOUT, TIMER_FADE] {
            let _ = KillTimer(Some(hwnd), t);
        }
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
    let mut s = st();
    s.m.reset();
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
    let h = st().m.hover(hot);
    if !h.changed {
        return;
    }
    redraw(hwnd);
    // hovering "More" for a moment opens the full list
    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_EXPAND);
        if h.expand_timer {
            SetTimer(Some(hwnd), TIMER_EXPAND, h.expand_ms, None);
        }
    }
}

fn expand(hwnd: HWND) {
    if !st().m.expand() {
        return;
    }
    redraw(hwnd);
    gfx::log("EXPAND (full list or PDF choice opened)");
}

/// The files were handed to the queue: show it on the bar for a moment, then take the bar away.
fn accept(hwnd: HWND, hot: i32, taken: usize) {
    st().m.accept(hot, taken);
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
            let paths = dragged_paths(d);
            let exts = offer::exts_of(&paths);
            // (a different number of files with the same types matters too: the PDF choice needs two or more)
            let (known_exts, known_files) = {
                let s = st();
                (s.sel_exts.clone(), s.m.files)
            };
            if exts != known_exts || paths.iter().filter(|p| p.is_file()).count() != known_files {
                gfx::log(&format!("DRAGENTER corrects the formats: {exts:?}"));
                apply_selection(&paths);
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
        let paths = data.as_ref().map(dragged_paths).unwrap_or_default();
        let n = paths.len();
        let kept = paths.clone();
        // the lock is released before the arms run: they redraw and accept, which take it again (a guard kept in the
        // `match` expression would deadlock)
        let outcome = st().m.drop_on(id, paths);
        match outcome {
            Drop::Pick => {
                // dropped on "More": the model kept the files and opened the list; wait for a click on a format
                gfx::log(&format!("DROP on More: choose a format (files={n})"));
                redraw(self.hwnd);
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_HIDE);
                    SetTimer(Some(self.hwnd), TIMER_PICK_TIMEOUT, PICK_TIMEOUT_MS, None);
                    *effect = DROPEFFECT_COPY;
                }
                return Ok(());
            }
            Drop::Submit(slot) => {
                let taken = queue::submit(&kept, &slot.id, &slot.label);
                gfx::log(&format!("DROP label={} files={n} taken={taken} {:?}", slot.label, kept));
                if taken > 0 {
                    unsafe { *effect = DROPEFFECT_COPY };
                    accept(self.hwnd, id, taken);
                    return Ok(());
                }
                unsafe { *effect = DROPEFFECT_NONE };
            }
            Drop::Nothing => {
                gfx::log(&format!("DROP outside any format ({n} files): nothing done"));
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
                        (s.m.shown, s.m.pick_mode)
                    };
                    // a click outside the open list closes it
                    if shown && pick {
                        let mut r = RECT::default();
                        let _ = unsafe { GetWindowRect(hwnd, &mut r) };
                        if x < r.left || x >= r.right || y < r.top || y >= r.bottom {
                            begin_hide(hwnd);
                        }
                    }
                    let fg = unsafe { GetForegroundWindow() };
                    let front = matches!(class_of(fg).as_str(), "CabinetWClass" | "ExploreWClass" | "Progman" | "WorkerW");
                    let hit = if front { hit_test(x, y) } else { 0 };
                    // Not a title bar, not a border. Explorer answers WM_NCHITTEST (HTCLIENT = 1), but it may be too
                    // busy to answer in time (a folder of videos: it is preparing their thumbnails); then the window's own
                    // rectangles decide, which need no answer from it.
                    let in_client = front && hit != 1 && in_client_area(fg, x, y);
                    let client = front && (hit == 1 || (hit == 0 && in_client));
                    if front {
                        gfx::log(&format!("press at ({x},{y}) on {}: hit test {hit}, inside the client area {in_client}, candidate {client}", class_of(fg)));
                    }
                    let mut s = st();
                    s.pressed = true;
                    s.candidate = client;
                    s.origin = (x, y);
                    s.fg_at_down = unsafe { GetForegroundWindow() }.0 as isize;
                    s.pending_since = None;
                    s.press_id += 1;
                    s.item_hit = None;
                    let (press, candidate) = (s.press_id, s.candidate);
                    drop(s);
                    if candidate {
                        // is the press on an item? (asked off the UI thread; the answer is kept only for this press)
                        std::thread::spawn(move || {
                            let on_item = item_at(x, y);
                            let mut s = st();
                            if s.press_id == press {
                                s.item_hit = on_item;
                            }
                        });
                    }
                } else if flags & RI_MOUSE_LEFT_BUTTON_UP != 0 {
                    let (shown, pick, pending, image_at, ready, exts) = {
                        let mut s = st();
                        s.pressed = false;
                        s.candidate = false;
                        (s.m.shown, s.m.pick_mode, s.pending_since.take(), s.image_at.take(), s.sel_ready, s.sel_exts.clone())
                    };
                    unsafe {
                        let _ = KillTimer(Some(hwnd), TIMER_CONFIRM);
                    }
                    if let (Some(since), false) = (pending, shown) {
                        // what was known when the button came up, to tell a drag the bar missed from a plain click-and-move
                        gfx::log(&format!("cancel: button released {} ms after the drag started, before it was confirmed (drag image seen at {:?} ms, selection ready={ready} {exts:?})", since.elapsed().as_millis(), image_at));
                    }
                    if shown && !pick && st().m.accepted.is_none() {
                        // the drop, if any, is delivered just before or after this; hide a moment later either way
                        unsafe { SetTimer(Some(hwnd), TIMER_HIDE, 350, None) };
                    }
                } else {
                    let mut s = st();
                    if s.pressed && s.candidate && !s.m.shown && s.pending_since.is_none() {
                        let (x, y) = gfx::cursor();
                        if (x - s.origin.0).abs() >= MOVE_PX || (y - s.origin.1).abs() >= MOVE_PX {
                            s.pending_since = Some(Instant::now());
                            s.sel_ready = false;
                            s.sel_exts.clear();
                            s.m.offer = Default::default();
                            let fg = s.fg_at_down;
                            drop(s);
                            // read the selection off the UI thread: Explorer may be slow to answer
                            let me = hwnd.0 as isize;
                            std::thread::spawn(move || {
                                let paths = read_selection(fg);
                                apply_selection(&paths);
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
            if st().m.shown {
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
            if st().m.pick_mode {
                let (x, y) = (lp.0 as i16 as i32 as f32, (lp.0 >> 16) as i16 as i32 as f32);
                let id = view().hit(x, y);
                let chosen = st().m.click_pick(id);
                if let Some((slot, files)) = chosen {
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
            if st().m.pick_mode {
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
                        (s.m.top, s.m.slide_done(Instant::now()))
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
        (s.pending_since, s.sel_ready, s.m.shown)
    };
    let kill = || unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_CONFIRM);
    };
    let Some(since) = pending else {
        kill();
        return;
    };
    let ms = since.elapsed().as_millis();
    let image = !shown && drag_image_present();
    if image {
        let mut s = st();
        if s.image_at.is_none() {
            s.image_at = Some(ms); // the shell drew its drag picture ms after the pointer started to move
        }
    }
    // a press on a file item and a move is a file drag, without waiting for the shell's picture of it (which may
    // come late, or after a quick drag is over); the picture still confirms a drag whose item could not be asked about
    let on_item = !shown && st().item_hit == Some(true);
    if (image || on_item) && (ready || ms >= SELECTION_WAIT_MS) {
        gfx::log(&format!("confirmed {} ms after the drag started: by {}", ms, if on_item { "the item under the pointer" } else { "the drag picture" }));
        kill();
        let unsupported = {
            let s = st();
            ready && !s.sel_exts.is_empty() && s.m.offer.is_empty()
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
        gfx::log(&format!("skip: no drag image appeared in {CONFIRM_MAX_MS} ms (a selection rectangle, not a file drag)"));
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
    let v = View { scale: 1.0, h: 720, hot: -1, offer: o, expanded, pick: false, accepted: None, choice: false };
    let _ = render(&v).save_png(path);
}

/// Feeds a real shell data object for `file` to the bar as a drop on the format `to_id`, as Explorer would, then
/// waits for the queue to empty and reports. Everything stays off-screen.
pub fn selftest(file: &str, to_id: &str) -> Result<()> {
    gfx::FORCE_OFFSCREEN.store(true, Ordering::Relaxed);
    let hwnd = make_window()?;
    let target: IDropTarget = Target { hwnd }.into();
    let data = data_object_for(file)?;
    apply_selection(&[PathBuf::from(file)]);
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
    println!("after drop: accepted={:?} bar_visible={}", st().m.accepted, unsafe { IsWindowVisible(hwnd) }.as_bool());
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
        if c.active() == 0 && st().m.accepted.is_none() && t0.elapsed().as_millis() > 2500 {
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
