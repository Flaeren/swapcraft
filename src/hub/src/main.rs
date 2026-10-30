//! voidflare hub: one window that lists the voidflare converters and starts them.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod catalog;
mod fx;
mod icons;
mod routes;

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use catalog::APPS;
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Rect, RichText, Sense, StrokeKind, UiBuilder,
    text::LayoutJob, pos2, vec2,
};
use vf_core::i18n;
use vf_core::job::{self, Event};
use vf_core::update;
use vf_ui::shell::{SHELL_CATALOG, drain, dropped_files, finish};
use vf_ui::theme::{self, Tokens, rgb};
use vf_ui::widgets::{self, Kind};

const CARD_MIN_W: f32 = 320.0;
const CARD_H: f32 = 248.0;
const PAD: f32 = 20.0;
const STRIP_H: f32 = 36.0;
const ROW_H: f32 = 68.0;
const BAR_H: f32 = 56.0;
const FOOTER_H: f32 = 36.0;

#[derive(Clone, Copy, PartialEq)]
enum View {
    Grid,
    List,
}

#[derive(Clone, Copy, PartialEq)]
enum Sort {
    NameAsc,
    NameDesc,
}

/// Where the update check stands. Nothing happens until the user clicks in the footer.
enum Upd {
    Idle,
    Checking,
    UpToDate,
    Available(update::Release),
    Downloading(u64, u64),
    Saved(PathBuf),
    Failed(String),
}

enum UpdMsg {
    Checked(Result<update::Release, String>),
    Progress(u64, u64),
    Done(Result<PathBuf, String>),
}

struct Hub {
    upd: Upd,
    upd_rx: Option<mpsc::Receiver<UpdMsg>>,
    query: String,
    view: View,
    sort: Sort,
    /// Where each app's portable build was found, same order as `APPS`.
    found: Vec<Option<PathBuf>>,
    notice: Option<String>,
    /// The drop panel: the files waiting for a target format, and the run that converts them.
    files: Vec<PathBuf>,
    /// Combined size of `files`, read once when they arrive.
    bytes: u64,
    running: bool,
    rx: Option<mpsc::Receiver<Event>>,
    cancel: Arc<AtomicBool>,
    log: Vec<(String, Kind)>,
    progress: (usize, usize),
    last_dir: Option<PathBuf>,
    /// The torch and dot grid; `drop_rect` is where the torch goes while a file is dragged over the window.
    fx: fx::Fx,
    torch: fx::Torch,
    drop_rect: Option<Rect>,
    flame: icons::Flame,
    icons: icons::AppIcons,
}

impl Hub {
    fn new(ctx: &egui::Context) -> Self {
        Self {
            upd: Upd::Idle,
            upd_rx: None,
            query: String::new(),
            view: View::Grid,
            sort: Sort::NameAsc,
            found: APPS.iter().map(catalog::locate).collect(),
            notice: None,
            files: Vec::new(),
            bytes: 0,
            running: false,
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            log: Vec::new(),
            progress: (0, 0),
            last_dir: None,
            fx: fx::Fx::default(),
            torch: fx::Torch::default(),
            drop_rect: None,
            flame: icons::Flame::new(ctx),
            icons: icons::AppIcons::new(ctx, &APPS.iter().map(|a| a.id).collect::<Vec<_>>()),
        }
    }

    fn launch(&mut self, i: usize) {
        let Some(path) = &self.found[i] else { return };
        let mut cmd = Command::new(path);
        if let Some(dir) = path.parent() {
            cmd.current_dir(dir);
        }
        self.notice = match cmd.spawn() {
            Ok(_) => None,
            Err(e) => Some(format!("Could not start {}: {e}", APPS[i].name)),
        };
    }

    /// Indices of the apps matching the search, available ones first, then by name.
    fn visible(&self) -> (Vec<usize>, Vec<usize>) {
        let q = self.query.trim().to_lowercase();
        let mut idx: Vec<usize> = (0..APPS.len())
            .filter(|&i| {
                let a = &APPS[i];
                q.is_empty() || [a.name, a.tagline, a.engine].iter().any(|s| s.to_lowercase().contains(&q))
            })
            .collect();
        idx.sort_by(|&a, &b| {
            let o = APPS[a].name.to_lowercase().cmp(&APPS[b].name.to_lowercase());
            if self.sort == Sort::NameDesc { o.reverse() } else { o }
        });
        idx.into_iter().partition(|&i| self.found[i].is_some())
    }
}

impl eframe::App for Hub {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let t = Tokens::get(ui.ctx());
        self.poll();
        self.poll_update();
        if self.running {
            ui.ctx().request_repaint_after(Duration::from_millis(60));
        }
        let dropped = dropped_files(ui.ctx());
        if !dropped.is_empty() && !self.running {
            self.set_files(&dropped);
        }
        self.torch = self.fx.update(ui.ctx(), &t, self.drop_rect.map(|r| r.center()));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg)).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            self.torch.backdrop(ui.painter(), ui.max_rect(), &t);
            self.header(ui, &t);
            self.drop_panel(ui, &t);
            self.toolbar(ui, &t);
            let (ready, planned) = self.visible();
            let body_h = (ui.available_height() - FOOTER_H).max(0.0);
            egui::ScrollArea::vertical().max_height(body_h).auto_shrink([false, false]).show(ui, |ui| {
                if ready.is_empty() && planned.is_empty() {
                    ui.add_space(48.0);
                    ui.vertical_centered(|ui| ui.label(RichText::new("No apps match your search.").color(t.muted)));
                    return;
                }
                for (label, group) in [("Installed", &ready), ("Coming soon", &planned)] {
                    if group.is_empty() {
                        continue;
                    }
                    section_label(ui, &t, label, group.len());
                    match self.view {
                        View::Grid => self.grid(ui, &t, group),
                        View::List => self.list(ui, &t, group),
                    }
                }
            });
            self.footer(ui, &t);
        });
    }
}

impl Hub {
    /// Replaces the waiting files with what was dropped or picked (folders are searched one level deep).
    fn set_files(&mut self, paths: &[PathBuf]) {
        self.log.clear();
        self.last_dir = None;
        self.progress = (0, 0);
        self.files = job::collect(paths, &routes::known_exts(), false);
        self.bytes = self.files.iter().filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum();
        self.notice = if self.files.is_empty() {
            let ext = paths.first().map(|p| routes::ext_of(p)).unwrap_or_default();
            Some(if ext.is_empty() { "Nothing here can be converted yet.".to_owned() } else { format!("No converter for .{ext} files yet.") })
        } else {
            None
        };
    }

    /// The user asked: look at the project's latest GitHub release (one HTTPS request, nothing sent about them).
    fn check_updates(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        self.upd = Upd::Checking;
        self.upd_rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(UpdMsg::Checked(update::latest()));
            ctx.request_repaint();
        });
    }

    /// The user asked: download the release ZIP into Downloads and verify its published checksum.
    fn download_update(&mut self, ctx: &egui::Context, release: update::Release) {
        let dest = PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join("Downloads");
        let (tx, rx) = mpsc::channel();
        self.upd = Upd::Downloading(0, release.zip.as_ref().map_or(0, |z| z.size));
        self.upd_rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let progress = |done, total| {
                let _ = tx.send(UpdMsg::Progress(done, total));
                ctx.request_repaint();
            };
            let result = update::download_verified(&release, &dest, &progress, &AtomicBool::new(false));
            let _ = tx.send(UpdMsg::Done(result));
            ctx.request_repaint();
        });
    }

    fn poll_update(&mut self) {
        let Some(rx) = &self.upd_rx else { return };
        let mut finished = false;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                UpdMsg::Checked(Ok(release)) => {
                    self.upd = if update::is_newer(&release.version, env!("CARGO_PKG_VERSION")) { Upd::Available(release) } else { Upd::UpToDate };
                    finished = true;
                }
                UpdMsg::Checked(Err(e)) => {
                    self.upd = Upd::Failed(e);
                    finished = true;
                }
                UpdMsg::Progress(done, total) => self.upd = Upd::Downloading(done, total),
                UpdMsg::Done(Ok(path)) => {
                    self.upd = Upd::Saved(path);
                    finished = true;
                }
                UpdMsg::Done(Err(e)) => {
                    self.upd = Upd::Failed(e);
                    finished = true;
                }
            }
        }
        if finished {
            self.upd_rx = None;
        }
    }

    fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        if let Some(all) = drain(rx, &mut self.log, &mut self.progress) {
            self.running = false;
            self.rx = None;
            finish(&all, &mut self.log, false);
            self.last_dir = all
                .iter()
                .rev()
                .find_map(|o| o.result.as_ref().ok())
                .and_then(|p| if p.is_dir() { Some(p.clone()) } else { p.parent().map(PathBuf::from) });
        }
    }

    fn convert(&mut self, route: usize) {
        let r = &routes::ROUTES[route];
        let (mine, other): (Vec<PathBuf>, Vec<PathBuf>) = self.files.iter().cloned().partition(|f| r.from.contains(&routes::ext_of(f).as_str()));
        self.log.clear();
        self.last_dir = None;
        if !other.is_empty() {
            self.log.push((format!("{} file(s) of another type skipped.", other.len()), Kind::Plain));
        }
        let (tx, rx) = mpsc::channel();
        self.cancel = Arc::new(AtomicBool::new(false));
        self.progress = (0, mine.len());
        match routes::start(r, mine, tx, self.cancel.clone()) {
            Ok(()) => {
                self.running = true;
                self.rx = Some(rx);
            }
            Err(msg) => self.log.push((msg, Kind::Bad)),
        }
    }

    /// The big panel on top: drop (or click to choose) a file, then pick the format to convert it to.
    fn drop_panel(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        let has_files = !self.files.is_empty();
        // Room for a second row of format chips when the file has many targets (a PDF has seven).
        let chips_w: f32 = self.files.first().map_or(0.0, |f| routes::targets(&routes::ext_of(f)).iter().map(|&i| routes::ROUTES[i].label.len() as f32 * 9.0 + 36.0).sum());
        let rows = (chips_w / (ui.available_width() - 4.0 * PAD)).ceil().max(1.0) as usize;
        let height = if has_files { 176.0 + 36.0 * rows as f32 } else { 220.0 };
        let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), height + 2.0 * PAD), Sense::hover());
        let rect = outer.shrink2(vec2(PAD, PAD));
        let zone = ui.interact(rect, ui.scope_id().with("drop-zone"), if has_files || self.running { Sense::hover() } else { Sense::click() });
        let lit = ui.ctx().animate_bool_with_time(ui.scope_id().with("drop-lit"), hovering || (zone.hovered() && !has_files), 0.2);
        {
            let p = ui.painter();
            // Translucent, so the dots and the torch read through.
            p.rect_filled(rect, CornerRadius::ZERO, t.bg.lerp_to_gamma(t.accent, 0.04 + 0.08 * lit).gamma_multiply(0.86));
            self.torch.border(p, rect, Some(t.line_strong.lerp_to_gamma(t.accent, lit)), t.accent_ink, 6.0, 5.0, 170.0);
        }
        self.drop_rect = Some(rect);
        if zone.hovered() && !has_files {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if zone.clicked()
            && let Some(picked) = rfd::FileDialog::new().set_title("Choose files to convert").add_filter("Supported files", &routes::known_exts()).pick_files()
        {
            self.set_files(&picked);
        }

        if !has_files {
            let p = ui.painter();
            let c = rect.center();
            p.text(pos2(c.x, c.y - 26.0), Align2::CENTER_CENTER, "DROP A FILE HERE", mono(20.0), t.ink_strong);
            p.text(pos2(c.x, c.y + 6.0), Align2::CENTER_CENTER, "or click to choose, then pick what to convert it to", FontId::proportional(14.0), t.muted);
            p.text(pos2(c.x, c.y + 44.0), Align2::CENTER_CENTER, "DOCX · XLSX · PPTX · PDF · DWG · CSV · RTF · ODT", mono(11.0), t.faint);
            if let Some(msg) = &self.notice {
                p.text(pos2(c.x, rect.bottom() - 18.0), Align2::CENTER_CENTER, msg, mono(11.5), t.bad);
            }
        } else {
            let mut inner = ui.new_child(UiBuilder::new().max_rect(rect.shrink(18.0)).layout(Layout::top_down(Align::Min)));
            self.panel_files(&mut inner, t);
        }
        if !self.log.is_empty() {
            egui::Frame::new().inner_margin(egui::Margin { left: PAD as i8, right: PAD as i8, top: 0, bottom: PAD as i8 }).show(ui, |ui| {
                if self.running || self.progress.0 < self.progress.1 {
                    widgets::progress(ui, self.progress.0, self.progress.1);
                    ui.add_space(6.0);
                }
                widgets::log_view(ui, t, &self.log);
            });
        }
    }

    fn panel_files(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let n = self.files.len();
        let ext = routes::ext_of(&self.files[0]);
        ui.horizontal(|ui| {
            ui.label(RichText::new("READY TO CONVERT").font(mono(11.0)).color(t.faint));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::button(ui, "Clear").clicked() && !self.running {
                    self.files.clear();
                    self.log.clear();
                    self.last_dir = None;
                }
                if let Some(dir) = self.last_dir.clone()
                    && widgets::button(ui, "Open folder").clicked()
                {
                    let _ = Command::new("explorer").arg(dir).spawn();
                }
            });
        });
        if self.files.is_empty() {
            return;
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            file_tile(ui, t, &ext, n);
            ui.vertical(|ui| {
                let name = |f: &PathBuf| f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if n == 1 {
                    ui.label(RichText::new(name(&self.files[0])).size(18.0).strong().color(t.ink_strong));
                    let dir = self.files[0].parent().map(|p| p.display().to_string()).unwrap_or_default();
                    ui.label(RichText::new(format!("{} · {}", human_size(self.bytes), dir.trim_start_matches(r"\\?\"))).font(mono(11.0)).color(t.faint));
                } else {
                    ui.label(RichText::new(format!("{n} {} files", ext.to_uppercase())).size(18.0).strong().color(t.ink_strong));
                    let shown: Vec<String> = self.files.iter().take(3).map(name).collect();
                    let more = if n > 3 { format!("  + {} more", n - 3) } else { String::new() };
                    ui.label(RichText::new(format!("{}{more}", shown.join("  ·  "))).size(13.0).color(t.muted));
                    ui.label(RichText::new(format!("{} in total", human_size(self.bytes))).font(mono(11.0)).color(t.faint));
                }
            });
        });
        ui.add_space(12.0);
        let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().hline(line.x_range(), line.center().y, t.hairline());
        ui.add_space(10.0);
        ui.label(RichText::new("CONVERT TO  →").font(mono(11.0)).color(t.faint));
        ui.add_space(6.0);
        let mut chosen = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for i in routes::targets(&ext) {
                let route = &routes::ROUTES[i];
                let more = APPS.iter().find(|a| a.id == route.app).map_or(route.app, |a| a.name);
                let button = widgets::primary(ui, t, route.label, !self.running).on_hover_text(format!("Default settings. More options in {more}."));
                if button.clicked() {
                    chosen = Some(i);
                }
            }
        });
        if let Some(i) = chosen {
            self.convert(i);
        }
    }
}

fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// A text link in the footer whose right edge is at `right`; opens `url` in the browser. Returns its left edge.
fn footer_link(ui: &mut egui::Ui, t: &Tokens, bar: Rect, right: f32, label: &str, url: &str) -> f32 {
    let galley = ui.painter().layout_no_wrap(label.to_owned(), mono(11.0), t.faint);
    let rect = Rect::from_min_size(pos2(right - galley.size().x, bar.top()), vec2(galley.size().x, bar.height()));
    let resp = ui.interact(rect, ui.scope_id().with(("footer-link", url)), Sense::click());
    let color = if resp.hovered() { t.ink_strong } else { t.faint };
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().text(pos2(rect.left(), bar.center().y), Align2::LEFT_CENTER, label, mono(11.0), color);
    if resp.hovered() {
        ui.painter().hline(rect.x_range(), bar.center().y + 8.0, egui::Stroke::new(1.0, color));
    }
    if resp.clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
    rect.left()
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{bytes} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

/// A colour per kind of file, so a Word document and a drawing do not look alike.
fn kind_color(ext: &str) -> Color32 {
    match ext {
        "docx" | "doc" | "rtf" | "odt" => Color32::from_rgb(76, 139, 245),
        "xlsx" | "csv" | "tsv" => Color32::from_rgb(52, 168, 110),
        "pptx" | "deckcraft" => Color32::from_rgb(232, 126, 62),
        "pdf" => Color32::from_rgb(226, 82, 82),
        "dwg" => Color32::from_rgb(38, 178, 190),
        _ => Color32::from_rgb(150, 150, 160),
    }
}

/// The file's icon: a tinted square with the extension, with a second sheet behind it for several files.
fn file_tile(ui: &mut egui::Ui, t: &Tokens, ext: &str, count: usize) {
    let (rect, _) = ui.allocate_exact_size(vec2(84.0, 84.0), Sense::hover());
    let tile = rect.shrink2(vec2(6.0, 6.0));
    let color = kind_color(ext);
    let p = ui.painter();
    if count > 1 {
        let back = tile.translate(vec2(6.0, -6.0));
        p.rect_filled(back, CornerRadius::ZERO, t.sunken);
        p.rect_stroke(back, CornerRadius::ZERO, egui::Stroke::new(1.0, color.gamma_multiply(0.55)), StrokeKind::Inside);
    }
    p.rect_filled(tile, CornerRadius::ZERO, t.bg.lerp_to_gamma(color, 0.18));
    p.rect_stroke(tile, CornerRadius::ZERO, egui::Stroke::new(1.5, color), StrokeKind::Inside);
    // Folded corner.
    let fold = 14.0;
    let (a, b, c) = (pos2(tile.right() - fold, tile.top()), pos2(tile.right(), tile.top() + fold), pos2(tile.right() - fold, tile.top() + fold));
    p.add(egui::Shape::convex_polygon(vec![a, b, c], color.gamma_multiply(0.6), egui::Stroke::NONE));
    p.text(tile.center() + vec2(0.0, 4.0), Align2::CENTER_CENTER, ext.to_uppercase(), FontId::monospace(if ext.len() > 4 { 14.0 } else { 18.0 }), color);
}

fn section_label(ui: &mut egui::Ui, t: &Tokens, label: &str, n: usize) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
    let p = ui.painter();
    p.text(pos2(rect.left() + PAD, rect.bottom() - 12.0), Align2::LEFT_BOTTOM, format!("{} · {n}", label.to_uppercase()), mono(11.0), t.faint);
    p.hline(rect.x_range(), rect.bottom() - 0.5, t.hairline());
}

impl Hub {
    fn header(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), BAR_H + 8.0), Sense::hover());
        let p = ui.painter();
        let c = pos2(rect.left() + PAD, rect.center().y);
        let w = self.flame.paint(p, c.x, c.y, 30.0, t.dark);
        p.text(pos2(c.x + w + 12.0, c.y), Align2::LEFT_CENTER, "voidflare", FontId::proportional(22.0), t.ink_strong);
        p.text(
            pos2(rect.right() - PAD, c.y),
            Align2::RIGHT_CENTER,
            "OFFLINE CONVERTERS · YOUR FILES STAY ON THIS PC",
            mono(11.0),
            t.faint,
        );
        p.hline(rect.x_range(), rect.bottom() - 0.5, t.hairline());
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), BAR_H), Sense::hover());
        let mut bar = ui.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(PAD, 0.0))).layout(Layout::left_to_right(Align::Center)));
        bar.spacing_mut().item_spacing.x = 12.0;
        bar.add(egui::TextEdit::singleline(&mut self.query).hint_text("Search apps").desired_width(260.0).margin(vec2(10.0, 7.0)));
        bar.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.selectable_value(&mut self.view, View::List, RichText::new("LIST").font(mono(11.0)));
            ui.selectable_value(&mut self.view, View::Grid, RichText::new("GRID").font(mono(11.0)));
            ui.add_space(8.0);
            let label = if self.sort == Sort::NameAsc { "NAME A–Z" } else { "NAME Z–A" };
            egui::ComboBox::from_id_salt("sort").selected_text(RichText::new(label).font(mono(11.0))).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.sort, Sort::NameAsc, "Name A–Z");
                ui.selectable_value(&mut self.sort, Sort::NameDesc, "Name Z–A");
            });
        });
        ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, t.hairline());
    }

    fn footer(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), FOOTER_H), Sense::hover());
        let p = ui.painter();
        p.hline(rect.x_range(), rect.top() + 0.5, t.hairline());
        p.text(pos2(rect.left() + PAD, rect.center().y), Align2::LEFT_CENTER, "VOIDFLARE.APP", mono(11.0), t.faint);

        // Updates: only on a click, and the label says what the click will do.
        let (label, color, hint): (String, Color32, &str) = match &self.upd {
            Upd::Idle => ("CHECK FOR UPDATES".into(), t.faint, "Asks github.com/Flaeren/voidflare for the latest release. Nothing else is sent."),
            Upd::Checking => ("CHECKING GITHUB…".into(), t.faint, ""),
            Upd::UpToDate => ("UP TO DATE ✓ · CHECK AGAIN".into(), t.good, ""),
            Upd::Available(r) => match &r.zip {
                Some(z) if r.sums.is_some() => (format!("v{} AVAILABLE · DOWNLOAD {}", r.version, human_size(z.size)), t.accent_ink, "Downloads the ZIP from GitHub into your Downloads folder and checks its checksum. It does not install anything."),
                _ => (format!("v{} AVAILABLE ↗", r.version), t.accent_ink, "Opens the release page in your browser."),
            },
            Upd::Downloading(done, total) => (format!("DOWNLOADING {}%", if *total > 0 { done * 100 / total } else { 0 }), t.faint, ""),
            Upd::Saved(_) => ("DOWNLOADED AND VERIFIED · SHOW FILE".into(), t.good, "Unzip it to a new folder, then run the new voidflare.exe."),
            Upd::Failed(msg) => (format!("{} · CLICK TO CLOSE", msg.chars().take(70).collect::<String>()), t.bad, ""),
        };
        let clickable = !matches!(self.upd, Upd::Checking | Upd::Downloading(..));
        let x = rect.left() + PAD + 130.0;
        let galley = ui.painter().layout_no_wrap(label.clone(), mono(11.0), color);
        let action = Rect::from_min_size(pos2(x, rect.top()), vec2(galley.size().x, rect.height()));
        let resp = ui.interact(action, ui.scope_id().with("update-action"), if clickable { Sense::click() } else { Sense::hover() });
        let shown = if resp.hovered() && clickable { t.ink_strong } else { color };
        ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, &label, mono(11.0), shown);
        if resp.hovered() && clickable {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            ui.painter().hline(action.x_range(), rect.center().y + 8.0, egui::Stroke::new(1.0, shown));
        }
        if !hint.is_empty() {
            resp.clone().on_hover_text(hint);
        }
        if resp.clicked() {
            let ctx = ui.ctx().clone();
            match std::mem::replace(&mut self.upd, Upd::Idle) {
                Upd::Idle | Upd::UpToDate => self.check_updates(&ctx),
                Upd::Available(r) if r.zip.is_some() && r.sums.is_some() => self.download_update(&ctx, r),
                Upd::Available(r) => {
                    ctx.open_url(egui::OpenUrl::new_tab(&r.page));
                    self.upd = Upd::Available(r);
                }
                Upd::Saved(path) => {
                    let _ = Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
                    self.upd = Upd::Saved(path);
                }
                Upd::Failed(_) => {}
                other => self.upd = other,
            }
        }

        let p = ui.painter();
        match &self.notice {
            Some(msg) => {
                p.text(pos2(rect.right() - PAD, rect.center().y), Align2::RIGHT_CENTER, msg, mono(11.0), t.bad);
            }
            None => {
                // Right to left: the link to ArtCraft, then the version.
                let link = footer_link(ui, t, rect, rect.right() - PAD, "BUILT ON THE ARTCRAFT SUITE ↗", "https://getartcraft.com");
                ui.painter().text(pos2(link - 14.0, rect.center().y), Align2::RIGHT_CENTER, format!("v{} · OPEN SOURCE ·", env!("CARGO_PKG_VERSION")), mono(11.0), t.faint);
            }
        };
    }

    fn grid(&mut self, ui: &mut egui::Ui, t: &Tokens, group: &[usize]) {
        let width = ui.available_width();
        let cols = ((width / CARD_MIN_W).floor() as usize).max(1);
        // Neighbouring cards share one hairline, so each is one pixel wider than its slot.
        let card_w = (width + (cols as f32 - 1.0)) / cols as f32;
        let rows = group.len().div_ceil(cols);
        let height = rows as f32 * (CARD_H - 1.0) + 1.0;
        let (area, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        for (n, &i) in group.iter().enumerate() {
            let (row, col) = (n / cols, n % cols);
            let min = area.min + vec2(col as f32 * (card_w - 1.0), row as f32 * (CARD_H - 1.0));
            self.card(ui, t, i, Rect::from_min_size(min, vec2(card_w, CARD_H)));
        }
    }

    fn card(&mut self, ui: &mut egui::Ui, t: &Tokens, i: usize, rect: Rect) {
        let app = &APPS[i];
        let ready = self.found[i].is_some();
        let resp = ui.interact(rect, ui.scope_id().with(("card", app.id)), if ready { Sense::click() } else { Sense::hover() });
        let hovered = resp.hovered();
        if ready && hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let lift = ui.ctx().animate_bool_with_time(ui.scope_id().with(("lift", app.id)), hovered, 0.25);
        let color = rgb(app.color);
        // Apps that are not available are drawn in greys, and in colour while the pointer is over them.
        let accent = if ready { color } else { t.muted.lerp_to_gamma(color, lift) };

        let p = ui.painter_at(rect);
        p.rect(rect, CornerRadius::ZERO, t.bg.lerp_to_gamma(accent, 0.05 * lift).gamma_multiply(0.86), t.hairline(), StrokeKind::Inside);
        if t.dark {
            fx::radial(&p, self.torch.pos, 160.0, accent, 0.10 * lift * self.torch.power);
            self.torch.border(&p, rect, None, accent, 6.0, 0.0, 170.0);
        }
        if lift > 0.0 {
            let inside = rect.shrink(1.0);
            p.rect_filled(Rect::from_min_size(inside.min, vec2(inside.width() * lift, 2.0)), CornerRadius::ZERO, accent);
        }

        // Top strip: the engine, and the status.
        let strip = Rect::from_min_size(rect.min, vec2(rect.width(), STRIP_H));
        p.hline(strip.x_range(), strip.bottom() - 0.5, t.hairline());
        p.text(
            pos2(rect.left() + PAD, strip.center().y),
            Align2::LEFT_CENTER,
            format!("ENGINE · {}", app.engine.to_uppercase()),
            mono(11.0),
            if ready || hovered { t.muted } else { t.faint },
        );
        let (status, status_color) = if ready { ("READY", t.good) } else if app.exe.is_some() { ("NOT FOUND", t.bad) } else { ("PLANNED", t.faint) };
        p.text(pos2(rect.right() - PAD, strip.center().y), Align2::RIGHT_CENTER, status, mono(11.0), status_color);

        // Icon, name and tagline.
        let tile = Rect::from_min_size(pos2(rect.left() + PAD, strip.bottom() + 18.0 - 4.0 * lift), vec2(64.0, 64.0));
        let tint = if ready { Color32::WHITE } else { Color32::from_gray(150).lerp_to_gamma(Color32::WHITE, lift) };
        self.icons.paint(&p, i, tile, tint, t.line_strong);
        let text_x = tile.right() + 14.0;
        let text_w = rect.right() - PAD - text_x;
        p.text(pos2(text_x, tile.center().y - 11.0), Align2::LEFT_CENTER, app.name, FontId::proportional(20.0), t.ink_strong);
        let tagline = clipped(app.tagline, FontId::proportional(13.0), t.muted, text_w, 1);
        p.galley(pos2(text_x, tile.center().y + 6.0), p.layout_job(tagline), t.muted);

        // Description.
        let blurb = clipped(app.blurb, FontId::proportional(12.5), t.muted, rect.width() - 2.0 * PAD, 3);
        p.galley(pos2(rect.left() + PAD, tile.bottom() + 18.0), p.layout_job(blurb), t.muted);

        // Action.
        let button = Rect::from_min_size(pos2(rect.left() + PAD, rect.bottom() - PAD - 32.0), vec2(112.0, 32.0));
        if ready {
            let over = ui.rect_contains_pointer(button);
            let fill = if over { t.invert_bg.lerp_to_gamma(accent, 0.35) } else { t.invert_bg };
            p.rect_filled(button, CornerRadius::ZERO, fill);
            p.text(button.center(), Align2::CENTER_CENTER, "OPEN", mono(12.0), t.invert_fg);
        } else {
            p.rect(button, CornerRadius::ZERO, Color32::TRANSPARENT, egui::Stroke::new(1.0, t.line), StrokeKind::Inside);
            p.text(button.center(), Align2::CENTER_CENTER, "COMING SOON", mono(11.0), t.faint);
        }

        if ready && resp.clicked() {
            self.launch(i);
        }
    }

    fn list(&mut self, ui: &mut egui::Ui, t: &Tokens, group: &[usize]) {
        for &i in group {
            let app = &APPS[i];
            let ready = self.found[i].is_some();
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), if ready { Sense::click() } else { Sense::hover() });
            let hovered = resp.hovered();
            if ready && hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let lift = ui.ctx().animate_bool_with_time(ui.scope_id().with(("row", app.id)), hovered, 0.2);
            let color = rgb(app.color);
            let accent = if ready { color } else { t.muted.lerp_to_gamma(color, lift) };
            let p = ui.painter_at(rect);
            p.rect_filled(rect, CornerRadius::ZERO, t.bg.lerp_to_gamma(accent, 0.05 * lift));
            p.hline(rect.x_range(), rect.bottom() - 0.5, t.hairline());
            if lift > 0.0 {
                p.rect_filled(Rect::from_min_size(rect.min, vec2(2.0, rect.height() * lift)), CornerRadius::ZERO, accent);
            }

            let tile = Rect::from_center_size(pos2(rect.left() + PAD + 20.0, rect.center().y), vec2(44.0, 44.0));
            let tint = if ready { Color32::WHITE } else { Color32::from_gray(150).lerp_to_gamma(Color32::WHITE, lift) };
            self.icons.paint(&p, i, tile, tint, t.line_strong);
            let x = tile.right() + 14.0;
            p.text(pos2(x, rect.center().y - 9.0), Align2::LEFT_CENTER, app.name, FontId::proportional(16.0), t.ink_strong);
            p.text(pos2(x, rect.center().y + 10.0), Align2::LEFT_CENTER, app.tagline, FontId::proportional(12.5), t.muted);

            let (status, status_color) = if ready { ("READY", t.good) } else if app.exe.is_some() { ("NOT FOUND", t.bad) } else { ("PLANNED", t.faint) };
            if rect.width() > 560.0 {
                p.text(pos2(rect.right() - PAD - 260.0, rect.center().y), Align2::LEFT_CENTER, app.engine.to_uppercase(), mono(11.0), t.muted);
                p.text(pos2(rect.right() - PAD - 150.0, rect.center().y), Align2::LEFT_CENTER, status, mono(11.0), status_color);
            }
            let button = Rect::from_center_size(pos2(rect.right() - PAD - 40.0, rect.center().y), vec2(80.0, 28.0));
            if ready {
                let over = ui.rect_contains_pointer(button);
                p.rect_filled(button, CornerRadius::ZERO, if over { t.invert_bg.lerp_to_gamma(accent, 0.35) } else { t.invert_bg });
                p.text(button.center(), Align2::CENTER_CENTER, "OPEN", mono(11.0), t.invert_fg);
            } else {
                p.rect(button, CornerRadius::ZERO, Color32::TRANSPARENT, egui::Stroke::new(1.0, t.line), StrokeKind::Inside);
                p.text(button.center(), Align2::CENTER_CENTER, "SOON", mono(11.0), t.faint);
            }
            if ready && resp.clicked() {
                self.launch(i);
            }
        }
    }
}

/// Text wrapped to `width` and cut with an ellipsis after `rows` lines.
fn clipped(text: &str, font: FontId, color: Color32, width: f32, rows: usize) -> LayoutJob {
    let mut job = LayoutJob::simple(text.to_owned(), font, color, width.max(1.0));
    job.wrap.max_rows = rows;
    job
}

fn main() -> eframe::Result {
    // The notes the DWG conversion reports come from its own app catalog.
    i18n::init(&[SHELL_CATALOG, include_str!("../../crates/app-dwg-to-pdf/assets/strings.tsv")]);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1040.0, 820.0])
            .with_min_inner_size([640.0, 560.0])
            .with_title("voidflare")
            .with_drag_and_drop(true)
            .with_icon(egui::IconData { rgba: include_bytes!("../assets/app_icon.rgba").to_vec(), width: 256, height: 256 }),
        ..Default::default()
    };
    eframe::run_native(
        "voidflare",
        options,
        Box::new(|cc| {
            theme::install(&cc.egui_ctx);
            let mut hub = Hub::new(&cc.egui_ctx);
            // Files dragged onto the exe, or sent to it, wait in the drop panel.
            let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).filter(|p| p.exists()).collect();
            if !args.is_empty() {
                hub.set_files(&args);
            }
            Ok(Box::new(hub))
        }),
    )
}
