//! DWG to PDF: the drawing's sheets, or the whole drawing on one page, on the CADCraft engine, offline.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use eframe::egui::{self, Align, Layout, RichText, TextureHandle};
use vf_core::dwg::{self, SIZES, SheetOptions};
use vf_core::engine::CADCRAFT;
use vf_core::i18n;
use vf_core::job::{self, Destination, Event, Item, out_dir_for};
use vf_core::{t, tf};
use vf_ui::shell::{self, SHELL_CATALOG};
use vf_ui::theme::{self, Tokens};
use vf_ui::widgets::{self, Kind, PAD, button, mono, section};

const CATALOG: &str = include_str!("../assets/strings.tsv");
const ICON_256: &[u8] = include_bytes!("../assets/icon_256.rgba");
const ICON_128: &[u8] = include_bytes!("../assets/icon_128.rgba");

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Sheets,
    Whole,
}

struct DwgApp {
    cli: Option<PathBuf>,
    files: Vec<PathBuf>,
    selected: BTreeSet<usize>,
    mode: Mode,
    merge: bool,
    include_model: bool,
    paper: usize,
    crop: bool,
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

impl DwgApp {
    fn new(cc: &eframe::CreationContext<'_>, initial: Vec<PathBuf>) -> Self {
        theme::install(&cc.egui_ctx);
        let cli = CADCRAFT.find();
        let notice = cli.is_none().then(|| {
            (t("ui.engine_missing_title"), tf("ui.engine_missing", &[("cli", CADCRAFT.cli), ("engine", "CADCraft")]))
        });
        let mut app = Self {
            cli,
            files: Vec::new(),
            selected: BTreeSet::new(),
            mode: Mode::Sheets,
            merge: true,
            include_model: false,
            paper: 0,
            // off: it also drops isolated tables and legends that can matter
            crop: false,
            recursive: true,
            beside: true,
            out_dir: shell::documents_dir(),
            open_when_done: true,
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            running: false,
            progress: (0, 0),
            log: Vec::new(),
            notice,
            icon: widgets::load_icon(&cc.egui_ctx, ICON_128),
        };
        app.add(&initial);
        app
    }

    fn add(&mut self, paths: &[PathBuf]) {
        let known: BTreeSet<String> = self.files.iter().map(|f| f.to_string_lossy().to_lowercase()).collect();
        for f in job::collect(paths, &["dwg"], self.recursive) {
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
        let dest = if self.beside { Destination::Beside } else { Destination::Folder(PathBuf::from(self.out_dir.trim())) };
        let (mode, opts, crop) = (self.mode, SheetOptions { include_model: self.include_model, merge: self.merge }, self.crop);
        let (label, long_side) = SIZES[self.paper];
        let items: Vec<Item> = self
            .files
            .iter()
            .map(|src| {
                let (cli, source, out_dir) = (cli.clone(), src.clone(), out_dir_for(&dest, src));
                let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let run: job::Task = match mode {
                    Mode::Sheets => Box::new(move |notify| dwg::convert_sheets(&cli, &source, &out_dir, opts, notify)),
                    Mode::Whole => {
                        let paper = label.split(' ').next().unwrap_or(label).to_owned();
                        Box::new(move |notify| dwg::convert_whole(&cli, &source, &out_dir, long_side, crop, &paper, notify))
                    }
                };
                Item { label: name, source: src.clone(), run }
            })
            .collect();
        let (tx, rx) = mpsc::channel();
        self.cancel.store(false, Ordering::Relaxed);
        self.rx = Some(rx);
        self.running = true;
        self.progress = (0, items.len());
        self.log.clear();
        job::spawn_tasks(items, tx, self.cancel.clone());
    }

    fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        if let Some(all) = shell::drain(rx, &mut self.log, &mut self.progress) {
            self.running = false;
            self.rx = None;
            shell::finish(&all, &mut self.log, self.open_when_done);
        }
    }

    fn body(&mut self, ui: &mut egui::Ui, tk: &Tokens) {
        section(ui, tk, &t("ui.files_title"));
        ui.label(RichText::new(t("app.list_label")).size(12.0).color(tk.muted));
        ui.add_space(6.0);
        widgets::file_list(ui, tk, &self.files, &mut self.selected);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if button(ui, &t("ui.add_files")).clicked()
                && let Some(picked) = rfd::FileDialog::new().set_title(t("app.pick_files")).add_filter("DWG", &["dwg"]).pick_files()
            {
                self.add(&picked);
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
                ui.label(RichText::new(tf(key, &[("n", &self.files.len().to_string())]).to_uppercase()).font(mono(11.0)).color(tk.faint));
            });
        });

        section(ui, tk, &t("app.what_to_export"));
        ui.radio_value(&mut self.mode, Mode::Sheets, t("app.mode_sheets"));
        ui.add_enabled_ui(self.mode == Mode::Sheets, |ui| {
            ui.indent("sheets", |ui| {
                ui.checkbox(&mut self.merge, t("app.merge"));
                ui.checkbox(&mut self.include_model, t("app.include_model"));
            });
        });
        ui.add_space(6.0);
        ui.radio_value(&mut self.mode, Mode::Whole, t("app.mode_whole"));
        ui.add_enabled_ui(self.mode == Mode::Whole, |ui| {
            ui.indent("whole", |ui| {
                ui.horizontal(|ui| {
                    ui.label(t("app.paper"));
                    egui::ComboBox::from_id_salt("paper").selected_text(SIZES[self.paper].0).show_ui(ui, |ui| {
                        for (i, (label, _)) in SIZES.iter().enumerate() {
                            ui.selectable_value(&mut self.paper, i, *label);
                        }
                    });
                });
                ui.checkbox(&mut self.crop, t("app.crop"));
            });
        });
        ui.add_space(6.0);
        ui.checkbox(&mut self.recursive, t("ui.opt_recursive"));

        shell::destination(ui, tk, &mut self.beside, &mut self.out_dir);

        ui.add_space(20.0);
        ui.horizontal(|ui| {
            if widgets::primary(ui, tk, &t("app.convert"), !self.running && self.cli.is_some()).clicked() {
                self.start();
            }
            if ui.add_enabled(self.running, egui::Button::new(RichText::new(t("ui.cancel").to_uppercase()).font(mono(11.5)))).clicked() {
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

impl eframe::App for DwgApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let tk = Tokens::get(&ctx);
        self.poll();
        if self.running {
            ctx.request_repaint_after(Duration::from_millis(60));
        }
        let dropped = shell::dropped_files(&ctx);
        if !dropped.is_empty() {
            let before = self.files.len();
            self.add(&dropped);
            shell::drop_feedback(&dropped, before, self.files.len(), &mut self.notice);
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(tk.bg)).show(ui, |ui| {
            widgets::header(ui, &tk, &self.icon, "DWG to PDF");
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                egui::Frame::new().inner_margin(egui::Margin::symmetric(PAD as i8, 0)).show(ui, |ui| self.body(ui, &tk));
            });
        });
        shell::drop_overlay(&ctx, &tk);
        shell::notice_window(&ctx, &mut self.notice);
    }
}

fn main() -> eframe::Result {
    i18n::init(&[SHELL_CATALOG, CATALOG]);
    let initial: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).filter(|p| p.exists()).collect();
    eframe::run_native(
        "DWG to PDF",
        shell::native_options("DWG to PDF", ICON_256, [820.0, 880.0]),
        Box::new(move |cc| Ok(Box::new(DwgApp::new(cc, initial)))),
    )
}
