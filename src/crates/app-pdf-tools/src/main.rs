//! PDF Tools: convert (Word, PowerPoint, HTML, RTF, text, pictures), merge, split, extract pages,
//! OCR and shrink PDFs, offline, on the PdfCraft engine.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use eframe::egui::{self, Align, Layout, RichText, TextureHandle};
use vf_core::engine::PDFCRAFT;
use vf_core::i18n;
use vf_core::job::{self, Destination, Event, Item, out_dir_for};
use vf_core::pdf::{self, Export, Pass, SplitBy};
use vf_core::{t, tf};
use vf_ui::shell::{self, SHELL_CATALOG};
use vf_ui::theme::{self, Tokens};
use vf_ui::widgets::{self, Kind, PAD, button, section};

const CATALOG: &str = include_str!("../assets/strings.tsv");
const ICON_256: &[u8] = include_bytes!("../assets/icon_256.rgba");
const ICON_128: &[u8] = include_bytes!("../assets/icon_128.rgba");

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Convert,
    Merge,
    Split,
    Extract,
    Ocr,
    Shrink,
}

impl Mode {
    const ALL: [Mode; 6] = [Mode::Convert, Mode::Merge, Mode::Split, Mode::Extract, Mode::Ocr, Mode::Shrink];

    fn key(self) -> &'static str {
        match self {
            Mode::Convert => "convert",
            Mode::Merge => "merge",
            Mode::Split => "split",
            Mode::Extract => "extract",
            Mode::Ocr => "ocr",
            Mode::Shrink => "shrink",
        }
    }
}

/// The formats "convert" offers, by label.
const FORMATS: [(&str, bool); 7] =
    [("Word (.docx)", false), ("PowerPoint (.pptx)", false), ("HTML (.html)", false), ("RTF (.rtf)", false), ("Text (.txt)", false), ("PNG", true), ("JPEG", true)];

fn export_for(format: usize, dpi: u32) -> Export {
    match format {
        0 => Export::Docx,
        1 => Export::Pptx,
        2 => Export::Html,
        3 => Export::Rtf,
        4 => Export::Txt,
        5 => Export::Png { dpi },
        _ => Export::Jpeg { dpi },
    }
}

struct PdfTools {
    cli: Option<PathBuf>,
    mode: Mode,
    files: Vec<PathBuf>,
    selected: BTreeSet<usize>,
    format: usize,
    dpi: u32,
    merge_name: String,
    split_every: u32,
    extract_pages: String,
    ppi: u32,
    quality: u32,
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

impl PdfTools {
    fn new(cc: &eframe::CreationContext<'_>, initial: Vec<PathBuf>) -> Self {
        theme::install(&cc.egui_ctx);
        let cli = PDFCRAFT.find();
        let notice = cli.is_none().then(|| {
            (t("ui.engine_missing_title"), tf("ui.engine_missing", &[("cli", PDFCRAFT.cli), ("engine", "PdfCraft")]))
        });
        let mut app = Self {
            cli,
            mode: Mode::Convert,
            files: Vec::new(),
            selected: BTreeSet::new(),
            format: 0,
            dpi: 150,
            merge_name: "merged.pdf".to_owned(),
            split_every: 1,
            extract_pages: String::new(),
            ppi: 150,
            quality: 60,
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
        for f in job::collect(paths, &["pdf"], self.recursive) {
            if !known.contains(&f.to_string_lossy().to_lowercase()) {
                self.files.push(f);
            }
        }
    }

    fn move_selected(&mut self, down: bool) {
        let Some(&i) = self.selected.iter().next() else { return };
        let j = if down { i + 1 } else { i.wrapping_sub(1) };
        if j < self.files.len() {
            self.files.swap(i, j);
            self.selected.clear();
            self.selected.insert(j);
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
        if self.mode == Mode::Merge && self.files.len() < 2 {
            self.notice = Some((t("ui.nothing_title"), t("app.need_two")));
            return;
        }
        if self.mode == Mode::Extract && self.extract_pages.trim().is_empty() {
            self.notice = Some((t("ui.nothing_title"), t("app.need_pages")));
            return;
        }
        let dest = if self.beside { Destination::Beside } else { Destination::Folder(PathBuf::from(self.out_dir.trim())) };
        let items = self.items(&cli, &dest);
        let (tx, rx) = mpsc::channel();
        self.cancel.store(false, Ordering::Relaxed);
        self.rx = Some(rx);
        self.running = true;
        self.progress = (0, items.len());
        self.log.clear();
        job::spawn_tasks(items, tx, self.cancel.clone());
    }

    fn items(&self, cli: &PathBuf, dest: &Destination) -> Vec<Item> {
        if self.mode == Mode::Merge {
            let mut name = self.merge_name.trim().to_owned();
            if name.is_empty() {
                name = "merged.pdf".to_owned();
            }
            if !name.to_lowercase().ends_with(".pdf") {
                name.push_str(".pdf");
            }
            let files = self.files.clone();
            let out_dir = out_dir_for(dest, &files[0]);
            let (cli, source) = (cli.clone(), files[0].clone());
            let label = format!("{} → {name}", tf("ui.count_many", &[("n", &files.len().to_string())]));
            return vec![Item { label, source, run: Box::new(move |_| pdf::merge(&cli, &files, &out_dir, &name)) }];
        }
        self.files
            .iter()
            .map(|src| {
                let (cli, source, out_dir) = (cli.clone(), src.clone(), out_dir_for(dest, src));
                let label = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let run: job::Task = match self.mode {
                    Mode::Convert => {
                        let what = export_for(self.format, self.dpi);
                        Box::new(move |_| pdf::export(&cli, &source, what, &out_dir))
                    }
                    Mode::Split => {
                        let by = SplitBy::Every(self.split_every.max(1));
                        Box::new(move |_| pdf::split(&cli, &source, &by, &out_dir))
                    }
                    Mode::Extract => {
                        let pages = self.extract_pages.trim().to_owned();
                        Box::new(move |_| pdf::extract(&cli, &source, &pages, &out_dir))
                    }
                    Mode::Ocr => Box::new(move |_| pdf::batch(&cli, &source, Pass::Ocr, &out_dir)),
                    Mode::Shrink => {
                        let pass = Pass::Shrink { ppi: self.ppi, quality: self.quality };
                        Box::new(move |_| pdf::batch(&cli, &source, pass, &out_dir))
                    }
                    Mode::Merge => unreachable!("handled above"),
                };
                Item { label, source: src.clone(), run }
            })
            .collect()
    }

    fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        if let Some(all) = shell::drain(rx, &mut self.log, &mut self.progress) {
            self.running = false;
            self.rx = None;
            shell::finish(&all, &mut self.log, self.open_when_done);
        }
    }

    fn options(&mut self, ui: &mut egui::Ui, tk: &Tokens) {
        match self.mode {
            Mode::Convert => {
                ui.horizontal(|ui| {
                    ui.label(t("app.format"));
                    egui::ComboBox::from_id_salt("format").selected_text(FORMATS[self.format].0).show_ui(ui, |ui| {
                        for (i, (label, _)) in FORMATS.iter().enumerate() {
                            ui.selectable_value(&mut self.format, i, *label);
                        }
                    });
                    if FORMATS[self.format].1 {
                        ui.label(t("app.dpi"));
                        ui.add(egui::DragValue::new(&mut self.dpi).range(36..=600));
                    }
                });
                ui.add_space(4.0);
                ui.label(RichText::new(t("app.convert_note")).size(12.0).color(tk.muted));
            }
            Mode::Merge => {
                ui.label(RichText::new(t("app.merge_hint")).size(12.0).color(tk.muted));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(t("app.merge_name"));
                    ui.add(egui::TextEdit::singleline(&mut self.merge_name).desired_width(240.0));
                });
            }
            Mode::Split => {
                ui.horizontal(|ui| {
                    ui.label(t("app.split_every"));
                    ui.add(egui::DragValue::new(&mut self.split_every).range(1..=10_000));
                    ui.label(t("app.split_pages"));
                });
            }
            Mode::Extract => {
                ui.horizontal(|ui| {
                    ui.label(t("app.extract_pages"));
                    ui.add(egui::TextEdit::singleline(&mut self.extract_pages).desired_width(200.0));
                });
            }
            Mode::Ocr => {
                ui.label(RichText::new(t("app.ocr_note")).size(12.0).color(tk.muted));
            }
            Mode::Shrink => {
                ui.horizontal(|ui| {
                    ui.label(t("app.shrink_ppi"));
                    ui.add(egui::DragValue::new(&mut self.ppi).range(36..=600));
                    ui.add_space(12.0);
                    ui.label(t("app.shrink_quality"));
                    ui.add(egui::DragValue::new(&mut self.quality).range(1..=100));
                });
            }
        }
        ui.add_space(4.0);
        ui.checkbox(&mut self.recursive, t("ui.opt_recursive"));
    }

    fn body(&mut self, ui: &mut egui::Ui, tk: &Tokens) {
        ui.add_space(14.0);
        ui.horizontal_wrapped(|ui| {
            for mode in Mode::ALL {
                let label = RichText::new(t(&format!("app.mode.{}", mode.key()))).font(egui::FontId::proportional(13.0));
                if ui.selectable_label(self.mode == mode, label).clicked() {
                    self.mode = mode;
                }
            }
        });

        section(ui, tk, &t("ui.files_title"));
        ui.label(RichText::new(t("app.list_label")).size(12.0).color(tk.muted));
        ui.add_space(6.0);
        widgets::file_list(ui, tk, &self.files, &mut self.selected);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if button(ui, &t("ui.add_files")).clicked()
                && let Some(picked) = rfd::FileDialog::new().set_title(t("app.pick_files")).add_filter(t("app.filetype"), &["pdf"]).pick_files()
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
            if self.mode == Mode::Merge {
                if button(ui, &t("app.up")).clicked() {
                    self.move_selected(false);
                }
                if button(ui, &t("app.down")).clicked() {
                    self.move_selected(true);
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let key = if self.files.len() == 1 { "ui.count_one" } else { "ui.count_many" };
                ui.label(RichText::new(tf(key, &[("n", &self.files.len().to_string())])).font(egui::FontId::proportional(12.5)).color(tk.muted));
            });
        });

        section(ui, tk, &t("ui.options"));
        self.options(ui, tk);
        shell::destination(ui, tk, &mut self.beside, &mut self.out_dir);

        ui.add_space(20.0);
        ui.horizontal(|ui| {
            let go = t(&format!("app.go.{}", self.mode.key()));
            if widgets::primary(ui, tk, &go, !self.running && self.cli.is_some()).clicked() {
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

impl eframe::App for PdfTools {
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
            widgets::header(ui, &tk, &self.icon, "PDF Tools");
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
        "PDF Tools",
        shell::native_options("PDF Tools", ICON_256, [820.0, 900.0]),
        Box::new(move |cc| Ok(Box::new(PdfTools::new(cc, initial)))),
    )
}
