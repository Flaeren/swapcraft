//! The window shared by the single-purpose converter apps: file list, options, destination, progress
//! and log. An app is a [`Config`]; `run` opens the window.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use eframe::egui::{self, Align, Align2, Layout, RichText, TextureHandle, vec2};
use vf_core::engine::Engine;
use vf_core::i18n;
use vf_core::job::{self, Destination, Event, Outcome, Spec};
use vf_core::{t, tf};

use crate::theme::{self, Tokens};
use crate::widgets::{self, Kind, PAD, button, section};

pub const SHELL_CATALOG: &str = include_str!("../assets/shell.tsv");

/// A yes/no option that adds `arg` to the engine's command line when ticked.
pub struct OptionDef {
    /// Catalog key of the label.
    pub label_key: &'static str,
    pub arg: &'static str,
    pub default: bool,
}

/// Converts one file with the settings in the `Spec`, into a folder.
pub type Runner = fn(&Spec, &std::path::Path, &std::path::Path) -> Result<PathBuf, String>;

/// One output format an app offers when it converts to several. `args` go to the engine for this format only
/// (a bit depth, a compression...).
pub struct FormatDef {
    pub ext: &'static str,
    /// Shown in the list: a format name, so not translated.
    pub label: &'static str,
    pub args: &'static [&'static str],
}

/// A 1-100 quality slider, shown only while one of `exts` is the chosen format.
pub struct QualityDef {
    pub arg: &'static str,
    pub default: u32,
    pub exts: &'static [&'static str],
}

pub struct Config {
    /// Window title and header text. A product name, so it is not translated.
    pub title: &'static str,
    pub engine: Engine,
    /// Shown in the "engine missing" message, e.g. `WordCraft`.
    pub engine_name: &'static str,
    /// Lower-case extensions of the files the app takes.
    pub exts: &'static [&'static str],
    /// The one output type, when `formats` is empty.
    pub out_ext: &'static str,
    /// Output formats to choose from. Empty: the app always writes `out_ext`.
    pub formats: &'static [FormatDef],
    pub quality: Option<QualityDef>,
    /// How one file is converted, when the engine has no plain `convert <in> <out>` (FilmCraft: see `vf_core::film`).
    pub runner: Option<Runner>,
    pub options: &'static [OptionDef],
    /// If set, a password field is shown and its value is passed in this environment variable.
    pub password_env: Option<&'static str>,
    /// 256x256 RGBA: the window icon.
    pub icon_256: &'static [u8],
    /// 128x128 RGBA: the icon in the header.
    pub icon_128: &'static [u8],
    /// The app's own TSV catalog (keys `app.*` plus its option labels).
    pub catalog: &'static str,
}

struct ConverterApp {
    cfg: &'static Config,
    cli: Option<PathBuf>,
    files: Vec<PathBuf>,
    selected: BTreeSet<usize>,
    options: Vec<bool>,
    /// Index into `cfg.formats`.
    format: usize,
    quality: u32,
    password: String,
    recursive: bool,
    beside: bool,
    out_dir: String,
    open_when_done: bool,
    rx: Option<mpsc::Receiver<Event>>,
    cancel: Arc<AtomicBool>,
    running: bool,
    progress: (usize, usize),
    log: Vec<(String, Kind)>,
    notice: Option<(String, String)>,
    icon: TextureHandle,
}

impl ConverterApp {
    fn new(cc: &eframe::CreationContext<'_>, cfg: &'static Config, initial: Vec<PathBuf>) -> Self {
        theme::install(&cc.egui_ctx);
        let cli = cfg.engine.find();
        let notice = cli.is_none().then(|| {
            (t("ui.engine_missing_title"), tf("ui.engine_missing", &[("cli", cfg.engine.cli), ("engine", cfg.engine_name)]))
        });
        let mut app = Self {
            cfg,
            cli,
            files: Vec::new(),
            selected: BTreeSet::new(),
            options: cfg.options.iter().map(|o| o.default).collect(),
            format: 0,
            quality: cfg.quality.as_ref().map_or(0, |q| q.default),
            password: String::new(),
            recursive: true,
            beside: true,
            out_dir: documents_dir(),
            open_when_done: true,
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            running: false,
            progress: (0, 0),
            log: Vec::new(),
            notice,
            icon: widgets::load_icon(&cc.egui_ctx, cfg.icon_128),
        };
        app.add(&initial);
        app
    }

    fn add(&mut self, paths: &[PathBuf]) {
        let known: BTreeSet<String> = self.files.iter().map(|f| f.to_string_lossy().to_lowercase()).collect();
        for f in job::collect(paths, self.cfg.exts, self.recursive) {
            if !known.contains(&f.to_string_lossy().to_lowercase()) {
                self.files.push(f);
            }
        }
    }

    fn start(&mut self) {
        let Some(cli) = self.cli.clone() else { return };
        if self.files.is_empty() {
            self.notice = Some((t("ui.nothing_title"), t("app.nothing_text")));
            return;
        }
        if !self.beside && self.out_dir.trim().is_empty() {
            self.notice = Some((t("ui.dest_title"), t("ui.dest_text")));
            return;
        }
        let mut extra_args: Vec<String> = self.cfg.options.iter().zip(&self.options).filter(|(_, on)| **on).map(|(d, _)| d.arg.to_owned()).collect();
        let out_ext = self.cfg.formats.get(self.format).map_or(self.cfg.out_ext, |f| f.ext);
        if let Some(f) = self.cfg.formats.get(self.format) {
            extra_args.extend(f.args.iter().map(|a| (*a).to_owned()));
        }
        if let Some(q) = &self.cfg.quality
            && q.exts.contains(&out_ext)
        {
            extra_args.extend([q.arg.to_owned(), self.quality.to_string()]);
        }
        let mut env = Vec::new();
        if let Some(var) = self.cfg.password_env
            && !self.password.is_empty()
        {
            // Through the environment: a password on the command line is readable by other processes.
            env.push((var.to_owned(), self.password.clone()));
        }
        let spec = Spec { cli, out_ext: out_ext.to_owned(), extra_args, env };
        let dest = if self.beside { Destination::Beside } else { Destination::Folder(PathBuf::from(self.out_dir.trim())) };
        let (tx, rx) = mpsc::channel();
        self.cancel.store(false, Ordering::Relaxed);
        self.rx = Some(rx);
        self.running = true;
        self.progress = (0, self.files.len());
        self.log.clear();
        match self.cfg.runner {
            None => job::spawn_batch(spec, self.files.clone(), dest, tx, self.cancel.clone()),
            Some(run) => {
                let items = self
                    .files
                    .iter()
                    .map(|src| {
                        let label = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let (spec, out_dir, source) = (spec.clone(), job::out_dir_for(&dest, src), src.clone());
                        job::Item { label, source: src.clone(), run: Box::new(move |_| run(&spec, &source, &out_dir)) }
                    })
                    .collect();
                job::spawn_tasks(items, tx, self.cancel.clone());
            }
        }
    }

    fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        let finished = drain(rx, &mut self.log, &mut self.progress);
        if let Some(all) = finished {
            self.running = false;
            self.rx = None;
            finish(&all, &mut self.log, self.open_when_done);
        }
    }
}

/// Moves the worker's events into the log; returns the outcomes once it has finished.
pub fn drain(rx: &mpsc::Receiver<Event>, log: &mut Vec<(String, Kind)>, progress: &mut (usize, usize)) -> Option<Vec<Outcome>> {
    let mut finished = None;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            Event::Start { index, total, name } => log.push((format!("[{}/{}] {name} …", index + 1, total), Kind::Plain)),
            Event::Done { index, outcome } => {
                progress.0 = index + 1;
                match outcome.result {
                    Ok(out) => log.push((format!("    OK → {}", out.display()), Kind::Good)),
                    Err(msg) => log.push((tf("ui.line_error", &[("msg", &msg)]), Kind::Bad)),
                }
            }
            Event::Note(msg) => log.push((tf("ui.line_note", &[("msg", &msg)]), Kind::Plain)),
            Event::Cancelled => log.push((t("ui.cancelled"), Kind::Plain)),
            Event::Finished(all) => finished = Some(all),
        }
    }
    finished
}

/// The closing line of a run, and the output folder opened if asked.
pub fn finish(all: &[Outcome], log: &mut Vec<(String, Kind)>, open_folder: bool) {
    let ok: Vec<&Outcome> = all.iter().filter(|o| o.result.is_ok()).collect();
    let bad = all.len() - ok.len();
    let line = tf("ui.finished", &[("ok", &ok.len().to_string()), ("bad", &bad.to_string())]);
    log.push((line, if bad > 0 { Kind::Bad } else { Kind::Good }));
    if open_folder
        && let Some(Ok(path)) = ok.last().map(|o| &o.result)
        && let Some(dir) = if path.is_dir() { Some(path.as_path()) } else { path.parent() }
    {
        vf_core::platform::open_folder(dir);
    }
}

pub fn documents_dir() -> String {
    std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("Documents").to_string_lossy().into_owned()).unwrap_or_default()
}

/// A modal-looking notice window; returns false once dismissed.
pub fn notice_window(ctx: &egui::Context, notice: &mut Option<(String, String)>) {
    if let Some((title, text)) = notice.clone() {
        let mut open = true;
        let mut ok = false;
        egui::Window::new(title).collapsible(false).resizable(false).open(&mut open).anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0)).show(ctx, |ui| {
            ui.label(text);
            ui.add_space(8.0);
            ok = button(ui, "OK").clicked();
        });
        if !open || ok {
            *notice = None;
        }
    }
}

/// Files dropped on the window.
pub fn dropped_files(ctx: &egui::Context) -> Vec<PathBuf> {
    ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect())
}

/// While files are dragged over the window, veils it and says where to let go, so the drop is not a leap of faith.
pub fn drop_overlay(ctx: &egui::Context, tk: &Tokens) {
    if ctx.input(|i| i.raw.hovered_files.is_empty()) {
        return;
    }
    let rect = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("drop-veil")));
    painter.rect_filled(rect, 0.0, tk.bg.gamma_multiply(0.86));
    painter.rect_stroke(rect.shrink(10.0), 0.0, egui::Stroke::new(2.0, tk.accent), egui::StrokeKind::Inside);
    painter.text(rect.center(), Align2::CENTER_CENTER, t("ui.drop_here"), crate::theme::strong(16.0), tk.ink_strong);
}

/// After files were dropped: if the list did not grow, say so instead of leaving the window as it was.
pub fn drop_feedback(dropped: &[PathBuf], before: usize, after: usize, notice: &mut Option<(String, String)>) {
    if !dropped.is_empty() && after == before {
        *notice = Some((t("ui.drop_none_title"), t("ui.drop_none_text")));
    }
}

impl eframe::App for ConverterApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let tk = Tokens::get(&ctx);
        self.poll();
        if self.running {
            ctx.request_repaint_after(Duration::from_millis(60));
        }
        let dropped = dropped_files(&ctx);
        if !dropped.is_empty() {
            let before = self.files.len();
            self.add(&dropped);
            drop_feedback(&dropped, before, self.files.len(), &mut self.notice);
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(tk.bg)).show(ui, |ui| {
            widgets::header(ui, &tk, &self.icon, self.cfg.title);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                egui::Frame::new().inner_margin(egui::Margin::symmetric(PAD as i8, 0)).show(ui, |ui| self.body(ui, &tk));
            });
        });
        drop_overlay(&ctx, &tk);
        notice_window(&ctx, &mut self.notice);
    }
}

impl ConverterApp {
    fn body(&mut self, ui: &mut egui::Ui, tk: &Tokens) {
        section(ui, tk, &t("ui.files_title"));
        ui.label(RichText::new(t("app.list_label")).size(12.0).color(tk.muted));
        ui.add_space(6.0);
        widgets::file_list(ui, tk, &self.files, &mut self.selected);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if button(ui, &t("ui.add_files")).clicked() {
                let exts: Vec<&str> = self.cfg.exts.to_vec();
                if let Some(picked) = rfd::FileDialog::new().set_title(t("app.pick_files")).add_filter(t("app.filetype"), &exts).pick_files() {
                    self.add(&picked);
                }
            }
            if button(ui, &t("ui.add_folder")).clicked()
                && let Some(dir) = rfd::FileDialog::new().set_title(t("app.pick_folder")).pick_folder()
            {
                self.add(&[dir]);
            }
            if button(ui, &t("ui.remove")).clicked() {
                widgets::remove_selected(&mut self.files, &mut self.selected);
            }
            if button(ui, &t("ui.clear")).clicked() {
                self.files.clear();
                self.selected.clear();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let key = if self.files.len() == 1 { "ui.count_one" } else { "ui.count_many" };
                ui.label(RichText::new(tf(key, &[("n", &self.files.len().to_string())])).font(egui::FontId::proportional(12.5)).color(tk.muted));
            });
        });

        if !self.cfg.formats.is_empty() {
            section(ui, tk, &t("ui.convert_to"));
            egui::ComboBox::from_id_salt("out-format")
                .selected_text(self.cfg.formats[self.format].label)
                .width(260.0)
                .show_ui(ui, |ui| {
                    for (i, f) in self.cfg.formats.iter().enumerate() {
                        ui.selectable_value(&mut self.format, i, f.label);
                    }
                });
            if let Some(q) = &self.cfg.quality
                && q.exts.contains(&self.cfg.formats[self.format].ext)
            {
                ui.add_space(6.0);
                ui.add(egui::Slider::new(&mut self.quality, 1..=100).text(t("ui.quality")));
            }
        }
        section(ui, tk, &t("ui.options"));
        for (def, on) in self.cfg.options.iter().zip(self.options.iter_mut()) {
            ui.checkbox(on, t(def.label_key));
        }
        ui.checkbox(&mut self.recursive, t("ui.opt_recursive"));
        if self.cfg.password_env.is_some() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(t("app.password"));
                ui.add(egui::TextEdit::singleline(&mut self.password).password(true).desired_width(200.0));
            });
        }

        destination(ui, tk, &mut self.beside, &mut self.out_dir);

        ui.add_space(20.0);
        ui.horizontal(|ui| {
            if widgets::primary(ui, tk, &t("app.convert"), !self.running && self.cli.is_some()).clicked() {
                self.start();
            }
            if ui.add_enabled(self.running, egui::Button::new(RichText::new(t("ui.cancel")).font(egui::FontId::proportional(13.0)))).clicked() {
                self.cancel.store(true, Ordering::Relaxed);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.checkbox(&mut self.open_when_done, t("ui.open_when_done"));
            });
        });
        ui.add_space(14.0);
        widgets::progress(ui, self.progress.0, self.progress.1);
        ui.add_space(10.0);
        widgets::log_view(ui, tk, &self.log);
        ui.add_space(PAD);
    }
}

/// "Where to save": next to each file, or in a chosen folder.
pub fn destination(ui: &mut egui::Ui, tk: &Tokens, beside: &mut bool, out_dir: &mut String) {
    section(ui, tk, &t("ui.save_where"));
    ui.radio_value(beside, true, t("ui.save_beside"));
    ui.horizontal(|ui| {
        ui.radio_value(beside, false, t("ui.save_in"));
        ui.add_enabled_ui(!*beside, |ui| {
            let browse_w = 96.0;
            ui.add(egui::TextEdit::singleline(out_dir).desired_width((ui.available_width() - browse_w - 16.0).max(80.0)));
            if button(ui, &t("ui.browse")).clicked()
                && let Some(dir) = rfd::FileDialog::new().set_title(t("ui.dlg_dest")).pick_folder()
            {
                *out_dir = dir.to_string_lossy().into_owned();
            }
        });
    });
}

/// The native window options shared by the apps.
pub fn native_options(title: &str, icon_256: &[u8], size: [f32; 2]) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(size)
            .with_min_inner_size([640.0, 600.0])
            .with_title(title)
            .with_drag_and_drop(true)
            .with_icon(egui::IconData { rgba: icon_256.to_vec(), width: 256, height: 256 }),
        ..Default::default()
    }
}

/// Opens the app's window. Files named on the command line (dragged onto the exe) are loaded first.
pub fn run(cfg: &'static Config) -> eframe::Result {
    i18n::init(&[SHELL_CATALOG, cfg.catalog]);
    let initial: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).filter(|p| p.exists()).collect();
    eframe::run_native(
        cfg.title,
        native_options(cfg.title, cfg.icon_256, [820.0, 860.0]),
        Box::new(move |cc| Ok(Box::new(ConverterApp::new(cc, cfg, initial)))),
    )
}
