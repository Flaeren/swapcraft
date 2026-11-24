//! swapcraft-tray on Linux (X11): the same drop bar, queue card and edge glow as on Windows, with the shared model and
//! drawing, over the X protocol alone (pure Rust, no C library).
//!
//! How a file drag is seen: a program that starts a drag (every file manager) takes ownership of the `XdndSelection`.
//! X's XFixes extension reports that to anyone who asks, and the pointer's button 1 is down: that is a file drag, with
//! no polling and nothing to run at rest. The bar asks the drag's owner for the list of files (`text/uri-list`) to
//! offer only the formats they can become, shows itself on the monitor under the pointer, and is itself an XDND drop
//! target (the protocol every X file manager speaks), so dropping on a format starts the conversion.
//!
//! Without a compositing window manager a window cannot be transparent: the bar and the card are then drawn on a dark
//! background and the glow is left out. Wayland has no way for a program to see other programs' drags: on a pure
//! Wayland session this says so and exits (an X11 session, or XWayland with an X11 file manager, works).

mod xdnd_source;

use std::collections::VecDeque;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use tiny_skia::Pixmap;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::bar_view::{MISSING, View, render};
use crate::card_view::{self, BTN_CANCEL, BTN_CLEAR, BTN_OPEN};
use crate::draw::{self, Mon, Rect};
use crate::glow::{Glow, Tick};
use crate::model::{ACCEPTED_MS, BarModel, Drop, PICK_TIMEOUT_MS};
use crate::offer;
use crate::queue::{self, Entry, Status};
use crate::watcher::{GlowCmd, Watcher};

type Res<T> = Result<T, Box<dyn std::error::Error>>;

/// The other end of the pipe that wakes the event loop when the queue changes (written from the worker thread).
static WAKE_FD: AtomicI32 = AtomicI32::new(-1);

fn wake() {
    let fd = WAKE_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        let _ = unsafe { libc::write(fd, [1u8].as_ptr().cast(), 1) };
    }
}

macro_rules! atoms {
    ($($field:ident = $name:expr),* $(,)?) => {
        struct Atoms { $($field: Atom),* }
        impl Atoms {
            fn new(c: &RustConnection) -> Res<Atoms> {
                $(let $field = c.intern_atom(false, $name)?;)*
                Ok(Atoms { $($field: $field.reply()?.atom),* })
            }
        }
    };
}

atoms! {
    xdnd_aware = b"XdndAware",
    xdnd_enter = b"XdndEnter",
    xdnd_position = b"XdndPosition",
    xdnd_status = b"XdndStatus",
    xdnd_leave = b"XdndLeave",
    xdnd_drop = b"XdndDrop",
    xdnd_finished = b"XdndFinished",
    xdnd_selection = b"XdndSelection",
    xdnd_action_copy = b"XdndActionCopy",
    uri_list = b"text/uri-list",
    opacity = b"_NET_WM_WINDOW_OPACITY",
    wm_name = b"_NET_WM_NAME",
    utf8 = b"UTF8_STRING",
    workarea = b"_NET_WORKAREA",
    xembed_info = b"_XEMBED_INFO",
    tray_opcode = b"_NET_SYSTEM_TRAY_OPCODE",
    manager = b"MANAGER",
    data_prop = b"SWAPCRAFT_DATA",
    window_type = b"_NET_WM_WINDOW_TYPE",
    type_notification = b"_NET_WM_WINDOW_TYPE_NOTIFICATION",
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Timer {
    /// The drag's file list did not come in time: show the bar with the common formats.
    ShowFallback,
    Hide,
    Anim,
    Expand,
    PickTimeout,
    Fade,
    Glow,
    Card,
}

/// A top-level window of ours that the window manager leaves alone, with what it takes to draw into it.
struct Surface {
    win: Window,
    gc: Gcontext,
    depth: u8,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    mapped: bool,
}

struct App {
    conn: RustConnection,
    root: Window,
    root_depth: u8,
    screen_num: usize,
    /// A 32-bit visual with alpha and its colour map, when the server has one.
    argb: Option<(Visualid, Colormap)>,
    /// A compositing manager runs: transparent windows work.
    composited: bool,
    atoms: Atoms,
    ctl: Window,
    bar: Option<Surface>,
    glow_win: Option<Surface>,
    card: Option<Surface>,
    tray: Option<Surface>,
    m: BarModel,
    glow: Glow,
    glow_pushed: u64,
    watcher: Watcher,
    timers: Vec<(Timer, Instant)>,
    // the drag in progress
    drag: bool,
    sel_ready: bool,
    sel_exts: Vec<String>,
    /// A drop was made and the files are being asked for: the drag's source, and the format that was under the pointer.
    drop_pending: Option<(Window, i32)>,
    /// The files the bar was last given (from the drag's own selection), for the drop.
    dragged: Vec<PathBuf>,
    // the queue card
    card_open: bool,
    card_hot: i32,
    card_rows: Vec<Entry>,
    card_phase: f32,
    quit: bool,
    log_events: bool,
    offscreen_ok: bool,
}

fn log(s: &str) {
    draw::log(s);
}

// ---------------------------------------------------------------------------------------------- setup

impl App {
    fn new() -> Res<App> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let screen = conn.setup().roots[screen_num].clone();
        let root = screen.root;
        let atoms = Atoms::new(&conn)?;
        conn.xfixes_query_version(5, 0)?.reply()?;
        // a 32-bit TrueColor visual for windows with per-pixel alpha
        let argb = screen
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| d.visuals.iter())
            .find(|v| v.class == VisualClass::TRUE_COLOR && v.red_mask == 0xff0000 && v.green_mask == 0xff00 && v.blue_mask == 0xff)
            .map(|v| v.visual_id);
        let argb = match argb {
            Some(vis) => {
                let cmap = conn.generate_id()?;
                conn.create_colormap(ColormapAlloc::NONE, cmap, root, vis)?;
                Some((vis, cmap))
            }
            None => None,
        };
        // a compositing manager owns _NET_WM_CM_S<screen>
        let cm = conn.intern_atom(false, format!("_NET_WM_CM_S{screen_num}").as_bytes())?.reply()?.atom;
        let composited = conn.get_selection_owner(cm)?.reply()?.owner != 0;
        // an invisible window to receive selections and to be the target of XFixes' reports
        let ctl = conn.generate_id()?;
        conn.create_window(0, ctl, root, 0, 0, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new())?;
        conn.xfixes_select_selection_input(ctl, atoms.xdnd_selection, xfixes::SelectionEventMask::SET_SELECTION_OWNER | xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY | xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE)?;
        // the tray manager announces itself on the root window when it (re)starts
        conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY))?;
        conn.flush()?;
        Ok(App {
            conn,
            root,
            root_depth: screen.root_depth,
            screen_num,
            argb,
            composited,
            atoms,
            ctl,
            bar: None,
            glow_win: None,
            card: None,
            tray: None,
            m: BarModel::new(),
            glow: Glow::new(),
            glow_pushed: 0,
            watcher: Watcher::new(),
            timers: Vec::new(),
            drag: false,
            sel_ready: false,
            sel_exts: Vec::new(),
            drop_pending: None,
            dragged: Vec::new(),
            card_open: false,
            card_hot: -1,
            card_rows: Vec::new(),
            card_phase: 0.0,
            quit: false,
            log_events: std::env::var_os("SWAPCRAFT_TRAY_LOG").is_some(),
            offscreen_ok: false,
        })
    }

    // ------------------------------------------------------------------------------------ timers

    fn set_timer(&mut self, t: Timer, ms: u64) {
        self.kill_timer(t);
        self.timers.push((t, Instant::now() + Duration::from_millis(ms)));
    }

    fn kill_timer(&mut self, t: Timer) {
        self.timers.retain(|(k, _)| *k != t);
    }

    fn has_timer(&self, t: Timer) -> bool {
        self.timers.iter().any(|(k, _)| *k == t)
    }

    /// The timers that are due, taken out of the list.
    fn due(&mut self) -> Vec<Timer> {
        let now = Instant::now();
        let due: Vec<Timer> = self.timers.iter().filter(|(_, at)| *at <= now).map(|(t, _)| *t).collect();
        self.timers.retain(|(_, at)| *at > now);
        due
    }

    // ------------------------------------------------------------------------------------ the screen

    fn pointer(&self) -> Res<(i32, i32, bool)> {
        let p = self.conn.query_pointer(self.root)?.reply()?;
        Ok((p.root_x as i32, p.root_y as i32, p.mask.contains(KeyButMask::BUTTON1)))
    }

    /// The monitor the point is on, its work area (without panels) and its scale (Xft.dpi over 96).
    fn monitor_at(&self, x: i32, y: i32) -> Mon {
        let mut full = Rect { left: 0, top: 0, right: 1920, bottom: 1080 };
        if let Ok(g) = self.conn.get_geometry(self.root).and_then(|c| Ok(c.reply())) {
            if let Ok(g) = g {
                full = Rect { left: 0, top: 0, right: g.width as i32, bottom: g.height as i32 };
            }
        }
        if let Ok(c) = self.conn.randr_get_monitors(self.root, true) {
            if let Ok(r) = c.reply() {
                let rects: Vec<Rect> = r.monitors.iter().map(|m| Rect { left: m.x as i32, top: m.y as i32, right: m.x as i32 + m.width as i32, bottom: m.y as i32 + m.height as i32 }).collect();
                if let Some(m) = rects.iter().find(|m| x >= m.left && x < m.right && y >= m.top && y < m.bottom).or(rects.first()) {
                    full = *m;
                }
            }
        }
        // _NET_WORKAREA: x, y, width, height of each desktop; the first is the one in use
        let mut work = full;
        if let Ok(c) = self.conn.get_property(false, self.root, self.atoms.workarea, AtomEnum::CARDINAL, 0, 4) {
            if let Ok(p) = c.reply() {
                if let Some(v) = p.value32().map(|i| i.collect::<Vec<u32>>()).filter(|v| v.len() >= 4) {
                    let wa = Rect { left: v[0] as i32, top: v[1] as i32, right: (v[0] + v[2]) as i32, bottom: (v[1] + v[3]) as i32 };
                    let (l, t, r, b) = (full.left.max(wa.left), full.top.max(wa.top), full.right.min(wa.right), full.bottom.min(wa.bottom));
                    if r > l && b > t {
                        work = Rect { left: l, top: t, right: r, bottom: b };
                    }
                }
            }
        }
        Mon { full, work, scale: self.dpi_scale() }
    }

    fn dpi_scale(&self) -> f32 {
        let text = self.conn.get_property(false, self.root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 100_000).ok().and_then(|c| c.reply().ok()).map(|p| String::from_utf8_lossy(&p.value).into_owned()).unwrap_or_default();
        let dpi = text.lines().find_map(|l| l.strip_prefix("Xft.dpi:")).and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(96.0);
        ((dpi / 96.0 * 4.0).round() / 4.0).clamp(1.0, 4.0)
    }

    // ------------------------------------------------------------------------------------ drawing into a window

    fn new_surface(&mut self, w: u32, h: u32, mask: EventMask, argb: bool) -> Res<Surface> {
        let win = self.conn.generate_id()?;
        let (depth, visual, cmap) = match (argb, self.argb) {
            (true, Some((v, c))) if self.composited => (32u8, v, c),
            _ => (self.root_depth, 0, 0),
        };
        let mut aux = CreateWindowAux::new().override_redirect(1).event_mask(mask).border_pixel(0).background_pixel(0);
        if cmap != 0 {
            aux = aux.colormap(cmap);
        }
        self.conn.create_window(depth, win, self.root, 0, 0, w.max(1) as u16, h.max(1) as u16, 0, WindowClass::INPUT_OUTPUT, visual, &aux)?;
        let gc = self.conn.generate_id()?;
        self.conn.create_gc(gc, win, &CreateGCAux::new())?;
        Ok(Surface { win, gc, depth, x: 0, y: 0, w, h, mapped: false })
    }

    /// Sends the bitmap to the window: premultiplied RGBA becomes the server's BGRA, or BGR over a dark ground when
    /// the window cannot be transparent.
    fn put(&self, s: &Surface, pm: &Pixmap) -> Res<()> {
        let (w, h) = (pm.width() as usize, pm.height() as usize);
        let mut data = Vec::with_capacity(w * h * 4);
        for px in pm.data().chunks_exact(4) {
            if s.depth == 32 {
                data.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            } else {
                let back = |c: u8| (c as u32 + (255 - px[3] as u32) * 18 / 255).min(255) as u8;
                data.extend_from_slice(&[back(px[2]), back(px[1]), back(px[0]), 0]);
            }
        }
        let max_rows = ((self.conn.maximum_request_bytes().saturating_sub(64)) / (w * 4)).max(1);
        let mut y = 0;
        while y < h {
            let rows = max_rows.min(h - y);
            self.conn.put_image(ImageFormat::Z_PIXMAP, s.win, s.gc, w as u16, rows as u16, 0, y as i16, 0, s.depth, &data[y * w * 4..(y + rows) * w * 4])?;
            y += rows;
        }
        Ok(())
    }

    fn place(&self, s: &mut Surface, x: i32, y: i32, w: u32, h: u32) -> Res<()> {
        s.x = x;
        s.y = y;
        s.w = w;
        s.h = h;
        self.conn.configure_window(s.win, &ConfigureWindowAux::new().x(x).y(y).width(w.max(1)).height(h.max(1)).stack_mode(StackMode::ABOVE))?;
        if !s.mapped {
            self.conn.map_window(s.win)?;
            s.mapped = true;
        }
        Ok(())
    }

    fn hide_surface(&self, s: &mut Surface) -> Res<()> {
        if s.mapped {
            self.conn.unmap_window(s.win)?;
            s.mapped = false;
        }
        Ok(())
    }

    /// 255 = as drawn. The compositor fades the window, so a fade costs a few bytes a frame.
    fn set_opacity(&self, win: Window, alpha: u8) {
        if !self.composited {
            return;
        }
        if alpha == 255 {
            let _ = self.conn.delete_property(win, self.atoms.opacity);
        } else {
            let _ = self.conn.change_property32(PropMode::REPLACE, win, self.atoms.opacity, AtomEnum::CARDINAL, &[alpha as u32 * 0x0101_0101]);
        }
    }

    // ------------------------------------------------------------------------------------ the bar

    fn bar_view(&self) -> View {
        self.m.view()
    }

    fn redraw_bar(&mut self) {
        if self.m.fade_start.is_some() {
            return;
        }
        self.paint_bar(255, 0);
    }

    /// Draws the bar in its window at its place (plus `dx` while it fades away), with the given opacity.
    fn paint_bar(&mut self, alpha: u8, dx: i32) {
        let v = self.bar_view();
        let pm = render(&v);
        let x = self.m.window_x(&v, Instant::now()) + dx;
        let (w, h) = (pm.width(), pm.height());
        let Some(mut s) = self.bar.take() else { return };
        let resized = s.w != w || s.h != h;
        let _ = self.place(&mut s, x, self.m.top, w, h);
        if resized || true {
            let _ = self.put(&s, &pm);
        }
        self.set_opacity(s.win, alpha);
        let _ = self.conn.flush();
        self.bar = Some(s);
    }

    fn show_bar(&mut self) {
        let (px, py, _) = self.pointer().unwrap_or((0, 0, false));
        let mon = self.monitor_at(px, py);
        let known = !self.sel_exts.is_empty();
        self.m.begin_show(&mon, false, known, Instant::now());
        if self.bar.is_none() {
            let mask = EventMask::BUTTON_PRESS | EventMask::POINTER_MOTION | EventMask::EXPOSURE;
            let w = ((crate::bar_view::BAR_W + crate::bar_view::PANEL_W) * mon.scale).ceil() as u32;
            match self.new_surface(w, mon.work.bottom as u32 - mon.work.top as u32, mask, true) {
                Ok(s) => {
                    // XdndAware: this window takes drops (protocol version 5)
                    let _ = self.conn.change_property32(PropMode::REPLACE, s.win, self.atoms.xdnd_aware, AtomEnum::ATOM, &[5]);
                    self.bar = Some(s);
                }
                Err(e) => {
                    log(&format!("cannot make the bar window: {e}"));
                    return;
                }
            }
        }
        self.paint_bar(255, 0);
        self.set_timer(Timer::Anim, 15);
        let exts = self.sel_exts.clone();
        log(&format!("SHOW at ({px},{py}) selection={exts:?} slots={:?}", self.m.offer.slots.iter().map(|x| x.label.clone()).collect::<Vec<_>>()));
    }

    fn begin_hide(&mut self) {
        if !self.m.begin_fade(Instant::now()) {
            return;
        }
        for t in [Timer::Hide, Timer::Anim, Timer::Expand, Timer::PickTimeout, Timer::ShowFallback] {
            self.kill_timer(t);
        }
        self.set_timer(Timer::Fade, 15);
    }

    fn fade_tick(&mut self) {
        let now = Instant::now();
        let Some(f) = self.m.fade(now) else { return };
        if f.done {
            self.hide_bar();
            return;
        }
        self.paint_bar(f.alpha, f.dx);
        self.set_timer(Timer::Fade, 15);
    }

    fn hide_bar(&mut self) {
        for t in [Timer::Hide, Timer::Anim, Timer::Expand, Timer::PickTimeout, Timer::Fade, Timer::ShowFallback] {
            self.kill_timer(t);
        }
        if let Some(mut s) = self.bar.take() {
            let _ = self.hide_surface(&mut s);
            let _ = self.conn.flush();
            self.bar = Some(s);
        }
        self.m.reset();
        self.sel_ready = false;
        self.sel_exts.clear();
        self.dragged.clear();
        self.drag = false;
        self.drop_pending = None;
    }

    fn anim_tick(&mut self) {
        self.paint_bar(255, 0);
        if !self.m.slide_done(Instant::now()) {
            self.set_timer(Timer::Anim, 15);
        }
    }

    /// The hot format for a point in root coordinates.
    fn hot_at(&self, root_x: i32, root_y: i32) -> i32 {
        let Some(s) = &self.bar else { return -1 };
        self.bar_view().hit((root_x - s.x) as f32, (root_y - s.y) as f32)
    }

    fn set_hot(&mut self, hot: i32) {
        let h = self.m.hover(hot);
        if !h.changed {
            return;
        }
        self.redraw_bar();
        self.kill_timer(Timer::Expand);
        if h.expand_timer {
            self.set_timer(Timer::Expand, h.expand_ms as u64);
        }
    }

    fn expand(&mut self) {
        if self.m.expand() {
            self.redraw_bar();
            log("EXPAND (full list or PDF choice opened)");
        }
    }

    fn accept(&mut self, hot: i32, taken: usize) {
        self.m.accept(hot, taken);
        self.redraw_bar();
        self.kill_timer(Timer::PickTimeout);
        self.kill_timer(Timer::Expand);
        self.set_timer(Timer::Hide, ACCEPTED_MS as u64);
    }

    // ------------------------------------------------------------------------------------ a drag starts and ends

    /// The owner of `XdndSelection` changed: a drag began (with button 1 down) or ended.
    fn on_selection_owner(&mut self, owner: Window) {
        if owner != 0 && owner != self.ctl {
            let (_, _, button) = self.pointer().unwrap_or((0, 0, false));
            if !button || self.drag {
                return;
            }
            self.drag = true;
            self.sel_ready = false;
            self.sel_exts.clear();
            self.m.offer = Default::default();
            self.dragged.clear();
            // ask the drag for the list of files; the answer shows the bar (or, if it is slow, the common formats do)
            let _ = self.conn.convert_selection(self.ctl, self.atoms.xdnd_selection, self.atoms.uri_list, self.atoms.data_prop, x11rb::CURRENT_TIME);
            let _ = self.conn.flush();
            self.set_timer(Timer::ShowFallback, 250);
        } else if owner == 0 {
            // the drag is over without a drop on us: take the bar away unless a click is waited for
            self.drag = false;
            self.kill_timer(Timer::ShowFallback);
            if self.m.shown && !self.m.pick_mode && self.m.accepted.is_none() && self.drop_pending.is_none() {
                self.set_timer(Timer::Hide, 350);
            }
        }
    }

    fn apply_selection(&mut self, paths: &[PathBuf]) {
        self.dragged = paths.to_vec();
        self.m.set_files(paths.iter().filter(|p| p.is_file()).count());
        let exts = offer::exts_of(paths);
        self.m.offer = if exts.is_empty() { offer::generic() } else { offer::offer(&exts) };
        self.sel_exts = exts;
    }

    fn on_selection_notify(&mut self, e: &SelectionNotifyEvent) {
        if e.selection != self.atoms.xdnd_selection || e.property == 0 {
            // the owner refused (a drag with no files): a drop that was waiting for them ends here
            if let Some((src, _)) = self.drop_pending.take() {
                self.send_finished(src, false);
                self.begin_hide();
            }
            if self.drag && !self.sel_ready {
                self.sel_ready = true;
                self.kill_timer(Timer::ShowFallback);
                log("skip: the drag carries no files");
                self.drag = false;
            }
            return;
        }
        let paths = self.read_uris(e.requestor, e.property);
        if let Some((src, hot)) = self.drop_pending.take() {
            self.finish_drop(src, hot, paths);
            return;
        }
        self.kill_timer(Timer::ShowFallback);
        self.sel_ready = true;
        if paths.is_empty() {
            log("skip: the drag carries no files");
            self.drag = false;
            return;
        }
        self.apply_selection(&paths);
        if self.m.offer.is_empty() {
            log(&format!("skip: nothing to offer for {:?}", self.sel_exts));
            self.drag = false;
            return;
        }
        if self.m.shown {
            self.redraw_bar();
        } else {
            self.show_bar();
        }
    }

    /// The file paths of a `text/uri-list` property.
    fn read_uris(&self, win: Window, prop: Atom) -> Vec<PathBuf> {
        let Some(p) = self.conn.get_property(true, win, prop, AtomEnum::ANY, 0, 1 << 20).ok().and_then(|c| c.reply().ok()) else { return Vec::new() };
        parse_uri_list(&String::from_utf8_lossy(&p.value))
    }

    // ------------------------------------------------------------------------------------ XDND target

    fn send_status(&self, src: Window, accept: bool) {
        let action = if accept { self.atoms.xdnd_action_copy } else { 0 };
        let ev = ClientMessageEvent::new(32, src, self.atoms.xdnd_status, [self.bar.as_ref().map_or(0, |s| s.win), accept as u32, 0, 0, action]);
        let _ = self.conn.send_event(false, src, EventMask::NO_EVENT, ev);
        let _ = self.conn.flush();
    }

    fn send_finished(&self, src: Window, accepted: bool) {
        let action = if accepted { self.atoms.xdnd_action_copy } else { 0 };
        let ev = ClientMessageEvent::new(32, src, self.atoms.xdnd_finished, [self.bar.as_ref().map_or(0, |s| s.win), accepted as u32, action, 0, 0]);
        let _ = self.conn.send_event(false, src, EventMask::NO_EVENT, ev);
        let _ = self.conn.flush();
    }

    fn on_client_message(&mut self, e: ClientMessageEvent) {
        let d = e.data.as_data32();
        let a = &self.atoms;
        if e.type_ == a.xdnd_enter {
            // the source's version and types are not needed: the bar always asks for text/uri-list
        } else if e.type_ == a.xdnd_position {
            let (x, y) = ((d[2] >> 16) as i32, (d[2] & 0xffff) as i32);
            let hot = self.hot_at(x, y);
            self.set_hot(hot);
            self.send_status(d[0], hot >= 0);
        } else if e.type_ == a.xdnd_leave {
            self.set_hot(-1);
        } else if e.type_ == a.xdnd_drop {
            let src = d[0];
            let (px, py, _) = self.pointer().unwrap_or((0, 0, false));
            let hot = self.hot_at(px, py);
            let hot = if hot < 0 { self.m.hot } else { hot };
            // the files come with the selection; the drop is finished when they have arrived
            self.drop_pending = Some((src, hot));
            let _ = self.conn.convert_selection(self.ctl, self.atoms.xdnd_selection, self.atoms.uri_list, self.atoms.data_prop, d[2]);
            let _ = self.conn.flush();
        } else if e.type_ == a.manager {
            if self.tray.is_some() {
                self.dock_tray();
            }
        }
    }

    /// The drop's files arrived: what the drop means.
    fn finish_drop(&mut self, src: Window, hot: i32, paths: Vec<PathBuf>) {
        let paths = if paths.is_empty() { self.dragged.clone() } else { paths };
        let n = paths.len();
        let kept = paths.clone();
        match self.m.drop_on(hot, paths) {
            Drop::Pick => {
                log(&format!("DROP on More: choose a format (files={n})"));
                self.redraw_bar();
                self.kill_timer(Timer::Hide);
                self.set_timer(Timer::PickTimeout, PICK_TIMEOUT_MS as u64);
                self.send_finished(src, true);
            }
            Drop::Submit(slot) => {
                let taken = queue::submit(&kept, &slot.id, &slot.label);
                log(&format!("DROP label={} files={n} taken={taken} {:?}", slot.label, kept));
                self.send_finished(src, taken > 0);
                if taken > 0 {
                    self.accept(hot, taken);
                } else {
                    self.begin_hide();
                }
            }
            Drop::Nothing => {
                log(&format!("DROP outside any format ({n} files): nothing done"));
                self.send_finished(src, false);
                self.begin_hide();
            }
        }
    }

    fn on_bar_click(&mut self, x: i32, y: i32) {
        if !self.m.pick_mode {
            return;
        }
        let id = self.bar_view().hit(x as f32, y as f32);
        if let Some((slot, files)) = self.m.click_pick(id) {
            let taken = queue::submit(&files, &slot.id, &slot.label);
            log(&format!("PICK label={} files={} taken={taken}", slot.label, files.len()));
            if taken > 0 {
                self.accept(id, taken);
            } else {
                self.begin_hide();
            }
        }
    }

    // ------------------------------------------------------------------------------------ the glow

    fn glow_cmd(&mut self, cmd: GlowCmd) {
        let (px, py, _) = self.pointer().unwrap_or((0, 0, false));
        let mon = || self.monitor_at(px, py);
        let m = mon();
        let on = match cmd {
            GlowCmd::Working => self.glow.working(|| m),
            GlowCmd::Finish(ok) => self.glow.finish(ok, || m),
            GlowCmd::Stop => self.glow.stop(),
        };
        if on && self.composited {
            self.set_timer(Timer::Glow, 16);
        }
    }

    fn glow_tick(&mut self) {
        let result = self.glow.tick();
        if let Some((pm, x, y, alpha)) = self.glow.frame().map(|(p, x, y, a)| (p.clone(), x, y, a)) {
            if self.glow_win.is_none() {
                // click-through: no input region
                if let Ok(s) = self.new_surface(pm.width(), pm.height(), EventMask::NO_EVENT, true) {
                    let _ = self.conn.shape_rectangles(shape::SO::SET, shape::SK::INPUT, ClipOrdering::UNSORTED, s.win, 0, 0, &[]);
                    self.glow_win = Some(s);
                }
            }
            if let Some(mut s) = self.glow_win.take() {
                if self.glow_pushed != self.glow.generation() || s.w != pm.width() || s.h != pm.height() {
                    let _ = self.place(&mut s, x, y, pm.width(), pm.height());
                    let _ = self.put(&s, &pm);
                    self.glow_pushed = self.glow.generation();
                } else {
                    let _ = self.conn.configure_window(s.win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
                }
                self.set_opacity(s.win, alpha);
                let _ = self.conn.flush();
                self.glow_win = Some(s);
            }
        }
        match result {
            Tick::Running => self.set_timer(Timer::Glow, 16),
            Tick::Idle => {}
            Tick::Off => {
                self.glow.clear();
                self.glow_pushed = 0;
                if let Some(mut s) = self.glow_win.take() {
                    let _ = self.hide_surface(&mut s);
                    let _ = self.conn.flush();
                    self.glow_win = Some(s);
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------ the queue

    fn on_queue(&mut self) {
        let u = self.watcher.update(queue::counts(), &queue::snapshot(), self.card_open);
        match u.glow {
            Some(GlowCmd::Finish(ok)) => log(&format!("batch over: ok={ok}")),
            _ => {}
        }
        if let Some(c) = u.glow {
            self.glow_cmd(c);
        }
        if self.card_open {
            self.redraw_card();
        }
        if let Some(n) = u.notice {
            notify(&n.title, &n.text);
        }
    }

    // ------------------------------------------------------------------------------------ the queue card

    fn redraw_card(&mut self) {
        let (rows, c, finished) = card_view::current();
        self.card_rows = rows.clone();
        let (px, py, _) = self.pointer().unwrap_or((0, 0, false));
        let mon = self.monitor_at(px, py);
        let pm = card_view::render(mon.scale, &rows, &c, finished, self.card_hot, self.card_phase);
        let x = mon.work.right - pm.width() as i32 - (12.0 * mon.scale) as i32;
        let y = mon.work.bottom - pm.height() as i32 - (12.0 * mon.scale) as i32;
        if self.card.is_none() {
            match self.new_surface(pm.width(), pm.height(), EventMask::BUTTON_PRESS | EventMask::POINTER_MOTION, true) {
                Ok(s) => self.card = Some(s),
                Err(e) => {
                    log(&format!("cannot make the card window: {e}"));
                    return;
                }
            }
        }
        if let Some(mut s) = self.card.take() {
            let first = !s.mapped;
            let _ = self.place(&mut s, x, y, pm.width(), pm.height());
            let _ = self.put(&s, &pm);
            if first {
                // clicks anywhere come to the card, so a click outside closes it
                let _ = self.conn.grab_pointer(true, s.win, EventMask::BUTTON_PRESS | EventMask::POINTER_MOTION, GrabMode::ASYNC, GrabMode::ASYNC, 0u32, 0u32, x11rb::CURRENT_TIME);
            }
            let _ = self.conn.flush();
            self.card = Some(s);
        }
        if c.running > 0 {
            self.set_timer(Timer::Card, 33);
        } else {
            self.kill_timer(Timer::Card);
        }
    }

    fn toggle_card(&mut self) {
        if self.card_open {
            self.close_card();
        } else {
            self.card_open = true;
            self.card_hot = -1;
            self.redraw_card();
        }
    }

    fn close_card(&mut self) {
        self.card_open = false;
        self.kill_timer(Timer::Card);
        let _ = self.conn.ungrab_pointer(x11rb::CURRENT_TIME);
        if let Some(mut s) = self.card.take() {
            let _ = self.hide_surface(&mut s);
            self.card = Some(s);
        }
        let _ = self.conn.flush();
    }

    fn on_card_event(&mut self, x: i32, y: i32, press: bool) {
        let Some(s) = &self.card else { return };
        let scale = self.monitor_at(s.x, s.y).scale;
        let (inside, lx, ly) = (x >= 0 && y >= 0 && (x as u32) < s.w && (y as u32) < s.h, x as f32 / scale, y as f32 / scale);
        if press && !inside {
            self.close_card();
            return;
        }
        let (c, finished) = (queue::counts(), self.card_rows.iter().any(|e| !matches!(e.status, Status::Running | Status::Waiting)));
        let id = if inside { card_view::hit(lx, ly, self.card_rows.len(), &c, finished) } else { -1 };
        if !press {
            if id != self.card_hot {
                self.card_hot = id;
                self.redraw_card();
            }
            return;
        }
        match id {
            BTN_OPEN => {
                open_hub();
                self.close_card();
            }
            BTN_CLEAR => queue::clear_finished(),
            BTN_CANCEL => queue::cancel_all(),
            i if i >= 0 => {
                if let Some(Entry { status: Status::Done(p), .. }) = self.card_rows.get(i as usize) {
                    queue::reveal(p);
                    self.close_card();
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------------------------ the tray icon

    /// The icon in the notification area (the old XEmbed tray every desktop's panel still serves), if there is one.
    fn dock_tray(&mut self) {
        let sel = match self.conn.intern_atom(false, format!("_NET_SYSTEM_TRAY_S{}", self.screen_num).as_bytes()).ok().and_then(|c| c.reply().ok()) {
            Some(r) => r.atom,
            None => return,
        };
        let owner = self.conn.get_selection_owner(sel).ok().and_then(|c| c.reply().ok()).map_or(0, |r| r.owner);
        if owner == 0 {
            log("no system tray on this desktop: the bar works, the queue card has no icon to open it");
            return;
        }
        if self.tray.is_none() {
            match self.new_surface(24, 24, EventMask::BUTTON_PRESS | EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY, false) {
                Ok(mut s) => {
                    // a tray icon is a normal child window: not override-redirect, the panel embeds it
                    let _ = self.conn.change_window_attributes(s.win, &ChangeWindowAttributesAux::new().override_redirect(0));
                    let _ = self.conn.change_property32(PropMode::REPLACE, s.win, self.atoms.xembed_info, self.atoms.xembed_info, &[0, 1]);
                    let _ = self.conn.change_property8(PropMode::REPLACE, s.win, self.atoms.wm_name, self.atoms.utf8, b"SwapCraft");
                    s.w = 24;
                    s.h = 24;
                    self.tray = Some(s);
                }
                Err(e) => {
                    log(&format!("cannot make the tray icon: {e}"));
                    return;
                }
            }
        }
        if let Some(s) = &self.tray {
            let ev = ClientMessageEvent::new(32, owner, self.atoms.tray_opcode, [x11rb::CURRENT_TIME, 0, s.win, 0, 0]);
            let _ = self.conn.send_event(false, owner, EventMask::STRUCTURE_NOTIFY, ev);
            let _ = self.conn.flush();
        }
    }

    fn paint_tray(&mut self) {
        let Some(s) = self.tray.take() else { return };
        if let Ok(g) = self.conn.get_geometry(s.win).and_then(|c| Ok(c.reply())) {
            if let Ok(g) = g {
                let (w, h) = (g.width.max(8) as u32, g.height.max(8) as u32);
                let pm = tray_icon(w, h);
                let _ = self.put(&Surface { w, h, ..s }, &pm);
                let _ = self.conn.flush();
            }
        }
        self.tray = Some(s);
    }

    // ------------------------------------------------------------------------------------ the loop

    fn handle(&mut self, ev: Event) {
        match ev {
            Event::XfixesSelectionNotify(e) if e.selection == self.atoms.xdnd_selection => self.on_selection_owner(e.owner),
            Event::SelectionNotify(e) => self.on_selection_notify(&e),
            Event::ClientMessage(e) => self.on_client_message(e),
            Event::ButtonPress(e) => {
                if self.bar.as_ref().is_some_and(|s| s.win == e.event) {
                    self.on_bar_click(e.event_x as i32, e.event_y as i32);
                } else if self.card.as_ref().is_some_and(|s| s.win == e.event) {
                    self.on_card_event(e.event_x as i32, e.event_y as i32, true);
                } else if self.tray.as_ref().is_some_and(|s| s.win == e.event) && e.detail == 1 {
                    self.toggle_card();
                }
            }
            Event::MotionNotify(e) => {
                if self.bar.as_ref().is_some_and(|s| s.win == e.event) && self.m.pick_mode {
                    let id = self.bar_view().hit(e.event_x as f32, e.event_y as f32);
                    self.set_hot(id);
                } else if self.card.as_ref().is_some_and(|s| s.win == e.event) {
                    self.on_card_event(e.event_x as i32, e.event_y as i32, false);
                }
            }
            Event::Expose(e) => {
                if self.tray.as_ref().is_some_and(|s| s.win == e.window) {
                    self.paint_tray();
                } else if self.bar.as_ref().is_some_and(|s| s.win == e.window) && self.m.shown {
                    self.redraw_bar();
                }
            }
            Event::ConfigureNotify(e) => {
                if self.tray.as_ref().is_some_and(|s| s.win == e.window) {
                    self.paint_tray();
                }
            }
            _ => {}
        }
    }

    fn timer_fired(&mut self, t: Timer) {
        match t {
            Timer::ShowFallback => {
                if self.drag && !self.m.shown {
                    if self.m.offer.is_empty() {
                        self.m.offer = offer::generic();
                    }
                    self.show_bar();
                }
            }
            Timer::Hide | Timer::PickTimeout => self.begin_hide(),
            Timer::Anim => self.anim_tick(),
            Timer::Expand => self.expand(),
            Timer::Fade => self.fade_tick(),
            Timer::Glow => self.glow_tick(),
            Timer::Card => {
                self.card_phase += 0.03;
                if self.card_open {
                    self.redraw_card();
                }
            }
        }
    }

    /// Waits for the X server, the queue's wake-up pipe or the next timer, and deals with what came.
    fn step(&mut self, wake_read: i32, max_wait_ms: i32) -> Res<()> {
        let now = Instant::now();
        let mut wait = max_wait_ms;
        if let Some(next) = self.timers.iter().map(|(_, at)| *at).min() {
            let ms = next.saturating_duration_since(now).as_millis().min(i32::MAX as u128) as i32;
            wait = if wait < 0 { ms } else { wait.min(ms) };
        }
        let mut fds = [libc::pollfd { fd: self.conn.stream().as_raw_fd(), events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: wake_read, events: libc::POLLIN, revents: 0 }];
        // events already read into the connection's buffer must not wait for the socket
        let buffered = self.conn.poll_for_event()?;
        let mut queued: VecDeque<Event> = VecDeque::new();
        if let Some(e) = buffered {
            queued.push_back(e);
        } else {
            unsafe { libc::poll(fds.as_mut_ptr(), 2, wait) };
        }
        if fds[1].revents & libc::POLLIN != 0 {
            let mut buf = [0u8; 64];
            let mut changed = false;
            loop {
                let n = unsafe { libc::read(wake_read, buf.as_mut_ptr().cast(), buf.len()) };
                if n <= 0 {
                    break;
                }
                // 1 = the queue changed, 2 = SIGTERM / SIGINT
                for b in &buf[..n as usize] {
                    if *b == 2 {
                        self.quit = true;
                    } else {
                        changed = true;
                    }
                }
            }
            if changed {
                self.on_queue();
            }
        }
        while let Some(e) = self.conn.poll_for_event()? {
            queued.push_back(e);
        }
        for e in queued {
            self.handle(e);
        }
        for t in self.due() {
            self.timer_fired(t);
        }
        let _ = self.conn.flush();
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------- helpers

/// The local files named by a `text/uri-list` (one `file://` address per line, comments start with `#`).
pub(crate) fn parse_uri_list(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("file://"))
        .map(|rest| {
            // file://host/path: the host (empty or localhost) goes
            let path = rest.find('/').map_or(rest, |i| &rest[i..]);
            PathBuf::from(percent_decode(path))
        })
        .filter(|p| p.is_file())
        .collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 0 && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The tray icon: the SwapCraft icon scaled to the size the panel gave, on a dark ground (the icon is drawn without
/// alpha: not every panel can show a transparent one).
fn tray_icon(w: u32, h: u32) -> Pixmap {
    let mut pm = Pixmap::new(w, h).expect("icon bitmap");
    pm.fill(tiny_skia::Color::from_rgba8(30, 30, 29, 255));
    if let Some(src) = tiny_skia::PixmapRef::from_bytes(ICON_RGBA, 256, 256) {
        let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
        let s = w.min(h) as f32 / 256.0;
        pm.draw_pixmap(0, 0, src, &paint, tiny_skia::Transform::from_scale(s, s), None);
    }
    pm
}

/// The program icon, 256x256 RGBA premultiplied is not needed: tiny-skia takes straight RGBA here and the icon's edges are opaque.
static ICON_RGBA: &[u8] = include_bytes!("../../../../hub/assets/app_icon.rgba");

/// A desktop notification through `notify-send` (libnotify's own command; nothing happens when it is not there).
fn notify(title: &str, text: &str) {
    let _ = std::process::Command::new("notify-send").args(["--app-name=SwapCraft", "--icon=swapcraft", title, text]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
}

/// Starts the SwapCraft hub that sits next to this program, if there is one.
fn open_hub() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };
    for name in ["swapcraft", "SwapCraft"] {
        let p = dir.join(name);
        if p.is_file() {
            if let Err(e) = std::process::Command::new(&p).current_dir(dir).spawn() {
                log(&format!("open hub failed: {} ({e})", p.display()));
            }
            return;
        }
    }
    log(&format!("open hub: no swapcraft next to {}", exe.display()));
}

/// One copy per person: the lock file also holds the process id, so the hub can ask it to quit.
fn single_instance() -> Option<File> {
    let path = vf_core::tray::pid_file()?;
    let f = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&path).ok()?;
    f.try_lock().ok()?;
    use std::io::Write;
    let mut w = &f;
    let _ = f.set_len(0);
    let _ = write!(w, "{}", std::process::id());
    Some(f)
}

fn wake_pipe() -> (i32, i32) {
    let mut fds = [0i32; 2];
    unsafe {
        libc::pipe(fds.as_mut_ptr());
        for fd in fds {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
    (fds[0], fds[1])
}

/// SIGTERM and SIGINT end the loop cleanly (the pipe is how the handler tells it).
static QUIT_FD: AtomicI32 = AtomicI32::new(-1);
extern "C" fn on_signal(_: libc::c_int) {
    let fd = QUIT_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        let _ = unsafe { libc::write(fd, [2u8].as_ptr().cast(), 1) };
    }
}

// ---------------------------------------------------------------------------------------------- entry points

/// The resident program: the loop, until it is told to quit.
pub fn run() -> Result<(), String> {
    if std::env::var_os("DISPLAY").is_none() {
        return Err(if std::env::var_os("WAYLAND_DISPLAY").is_some() { "This is a Wayland session with no X11 display: the drop bar needs X11 (or XWayland with an X11 file manager).".into() } else { "No display: the drop bar needs a desktop session.".into() });
    }
    let Some(_lock) = single_instance() else { return Ok(()) }; // a second copy just exits
    let mut app = App::new().map_err(|e| e.to_string())?;
    let (rd, wr) = wake_pipe();
    WAKE_FD.store(wr, Ordering::Relaxed);
    QUIT_FD.store(wr, Ordering::Relaxed);
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as usize);
        libc::signal(libc::SIGINT, on_signal as usize);
    }
    queue::on_change(wake);
    app.dock_tray();
    log("resident: waiting");
    while !app.quit {
        if let Err(e) = app.step(rd, -1) {
            return Err(e.to_string());
        }
    }
    // take the lock file's process id away, so nobody asks a program that is gone to quit
    if let Some(p) = vf_core::tray::pid_file() {
        let _ = std::fs::remove_file(p);
    }
    log("resident: asked to quit");
    Ok(())
}

/// `--selftest <file> <format>`: the bar for <file> on the real X server, the drop on <format> as the XDND handler gets
/// it, the conversion, and the glow, then the result in words. With `--xdnd` the drop comes from a real XDND source.
pub fn selftest(file: &str, to_id: &str, via_xdnd: bool) -> Result<(), String> {
    let mut app = App::new().map_err(|e| e.to_string())?;
    let (rd, wr) = wake_pipe();
    WAKE_FD.store(wr, Ordering::Relaxed);
    queue::on_change(wake);
    let path = PathBuf::from(file);
    let source = if via_xdnd { Some(xdnd_source::Source::start(file)) } else { None };
    if via_xdnd {
        // the drag is another X client: the bar has to see it begin, ask it for the files and show itself, by itself
        let t0 = Instant::now();
        while !app.m.shown && t0.elapsed() < Duration::from_secs(5) {
            app.step(rd, 20).map_err(|e| e.to_string())?;
        }
        if !app.m.shown {
            return Err("the bar did not appear: the drag was not detected".into());
        }
        println!("the bar appeared by itself {} ms after the drag began", t0.elapsed().as_millis());
    } else {
        app.drag = true;
        app.apply_selection(&[path.clone()]);
        app.sel_ready = true;
        app.show_bar();
    }
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(400) {
        app.step(rd, 20).map_err(|e| e.to_string())?;
    }
    // SWAPCRAFT_TRAY_SHOT=<png>: what the bar window really shows on the server, to look at it
    if let (Some(out), Some(s)) = (std::env::var_os("SWAPCRAFT_TRAY_SHOT"), &app.bar) {
        let img = app.conn.get_image(ImageFormat::Z_PIXMAP, s.win, 0, 0, s.w as u16, s.h as u16, !0).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
        if let Some(mut pm) = Pixmap::new(s.w, s.h) {
            for (d, px) in pm.data_mut().chunks_exact_mut(4).zip(img.data.chunks_exact(4)) {
                d.copy_from_slice(&[px[2], px[1], px[0], 255]);
            }
            let _ = pm.save_png(out);
        }
    }
    let v = app.bar_view();
    println!("offer for {file}: slots={:?} more={:?} composited={}", v.offer.slots.iter().map(|s| &s.label).collect::<Vec<_>>(), v.offer.more.iter().map(|s| &s.label).collect::<Vec<_>>(), app.composited);
    let in_main = v.offer.slots.iter().position(|s| s.id == to_id);
    let in_more = v.offer.more.iter().position(|s| s.id == to_id);
    let (win, bx, by) = {
        let s = app.bar.as_ref().ok_or("the bar has no window")?;
        (s.win, s.x, s.y)
    };
    let (cx, cy) = if let Some(i) = in_main {
        let b = v.slot_box(i);
        (b.0 + b.2 / 2.0, b.1 + b.3 / 2.0)
    } else if let Some(k) = in_more {
        // the list opens when the pointer rests on "More"; here it is opened directly
        app.expand();
        let v2 = app.bar_view();
        let b = v2.chip_box(k);
        (b.0 + b.2 / 2.0, b.1 + b.3 / 2.0)
    } else {
        return Err(format!("{to_id} is not offered for this file"));
    };
    // after the list opened the window moved left
    let (bx, by) = if in_more.is_some() { let s = app.bar.as_ref().ok_or("no bar")?; (s.x, s.y) } else { (bx, by) };
    let (rx, ry) = (bx + cx as i32, by + cy as i32);
    println!("drop at ({rx},{ry}) on the format {to_id}");
    if let Some(s) = &source {
        s.go(xdnd_source::Target { x: rx, y: ry, window: win });
    } else {
        let hot = app.hot_at(rx, ry);
        app.set_hot(hot);
        app.finish_drop(0, hot, vec![path]);
    }
    let t0 = Instant::now();
    let mut last = String::new();
    while t0.elapsed() < Duration::from_secs(15) {
        app.step(rd, 20).map_err(|e| e.to_string())?;
        let c = queue::counts();
        let line = format!("queue: waiting={} running={} done={} failed={} glow={}", c.waiting, c.running, c.done, c.failed, app.glow.describe());
        if line != last {
            println!("[{:>5} ms] {line}", t0.elapsed().as_millis());
            last = line;
        }
        if c.active() == 0 && c.done + c.failed > 0 && app.glow.describe() == "off" && !app.m.shown && app.m.fade_start.is_none() {
            break;
        }
    }
    if let Some(s) = source {
        s.finish().map_err(|e| format!("the drag source says: {e}"))?;
        println!("the XDND handshake finished: the bar accepted the drop");
    }
    for e in queue::snapshot() {
        println!("entry: {} -> {} : {:?}", e.name, e.to, e.status);
    }
    println!("bar visible at the end: {}", app.bar.as_ref().is_some_and(|s| s.mapped));
    // SWAPCRAFT_TRAY_SHOT_CARD=<png>: open the queue card, keep a picture of it, press "Svuota" as a click would
    if let Some(out) = std::env::var_os("SWAPCRAFT_TRAY_SHOT_CARD") {
        app.toggle_card();
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(300) {
            app.step(rd, 20).map_err(|e| e.to_string())?;
        }
        let (win, w, h) = app.card.as_ref().map(|s| (s.win, s.w, s.h)).ok_or("the queue card has no window")?;
        let img = app.conn.get_image(ImageFormat::Z_PIXMAP, win, 0, 0, w as u16, h as u16, !0).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
        if let Some(mut pm) = Pixmap::new(w, h) {
            for (d, px) in pm.data_mut().chunks_exact_mut(4).zip(img.data.chunks_exact(4)) {
                d.copy_from_slice(&[px[2], px[1], px[0], 255]);
            }
            let _ = pm.save_png(out);
        }
        let (rows, c, finished) = card_view::current();
        let layout = card_view::layout(rows.len(), &c, finished);
        let clear = layout.buttons.iter().find(|b| b.0 == BTN_CLEAR).ok_or("the card has no Svuota button")?;
        println!("queue before: {} entries", queue::snapshot().len());
        app.on_card_event((clear.1 + clear.3 / 2.0) as i32, (clear.2 + clear.4 / 2.0) as i32, true);
        println!("queue after pressing Svuota: {} entries", queue::snapshot().len());
        app.close_card();
        println!("the card is closed: {}", !app.card_open);
    }
    Ok(())
}

pub fn first_missing_text() -> &'static str {
    MISSING
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uri_list_gives_the_local_files_with_their_odd_names() {
        let dir = std::env::temp_dir().join(format!("vf-uri-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let odd = dir.join("gita \u{e0} Roma #1.png");
        std::fs::write(&odd, b"x").unwrap();
        let encoded = format!("file://{}", odd.to_string_lossy().replace(' ', "%20").replace('#', "%23").replace('\u{e0}', "%C3%A0"));
        let list = format!("# a comment\r\n{encoded}\r\nhttp://example.test/a.png\r\nfile:///no/such/file.png\r\n");
        assert_eq!(parse_uri_list(&list), vec![odd]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_host_in_the_address_is_skipped() {
        let dir = std::env::temp_dir().join(format!("vf-uri-host-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(parse_uri_list(&format!("file://localhost{}", f.display())), vec![f]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[allow(dead_code)]
fn unused(_: &Path) {}
