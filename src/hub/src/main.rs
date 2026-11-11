//! The SwapCraft hub (SwapCraft.exe; voidflare.exe up to 0.4): one window that lists the converters and starts them.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod catalog;
mod icons;
mod menu;
mod recipes;
mod settings;
mod single;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use catalog::APPS;
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Rect, RichText, Sense, StrokeKind, UiBuilder,
    text::LayoutJob, pos2, vec2,
};
use vf_core::i18n;

/// The hub's own texts: five languages, English first (the others fall back to it).
const HUB_CATALOG: &str = include_str!("../assets/hub.tsv");

/// The text for `key` in the chosen language.
fn tr(key: &str) -> String {
    i18n::t(key)
}

/// [`tr`] with each `{name}` replaced.
/// A message kept as its catalog key and arguments, so it follows a change of language while it is on screen.
/// `key` empty: `args[0]` is already text (an engine's or the system's own words).
#[derive(Clone, Default)]
struct Said {
    key: &'static str,
    args: Vec<(&'static str, String)>,
}

impl Said {
    fn key(key: &'static str) -> Self {
        Said { key, args: vec![] }
    }
    fn with(key: &'static str, args: &[(&'static str, &str)]) -> Self {
        Said { key, args: args.iter().map(|(k, v)| (*k, (*v).to_owned())).collect() }
    }
    fn raw(text: String) -> Self {
        Said { key: "", args: vec![("", text)] }
    }
    fn text(&self) -> String {
        if self.key.is_empty() {
            return self.args.first().map(|a| a.1.clone()).unwrap_or_default();
        }
        let args: Vec<(&str, &str)> = self.args.iter().map(|(k, v)| (*k, v.as_str())).collect();
        trf(self.key, &args)
    }
}

fn trf(key: &str, args: &[(&str, &str)]) -> String {
    i18n::tf(key, args)
}
use vf_core::job::{self, Event};
use vf_core::packs::{self, EngineSource, Health, Status};
use vf_core::routes;
use vf_core::update;
use vf_ui::shell::{SHELL_CATALOG, drain, dropped_files, finish};
use vf_ui::theme::{self, Tokens, rgb, strong};
use vf_ui::widgets::{self, Kind};

const CARD_MIN_W: f32 = 300.0;
const CARD_H: f32 = 196.0;
/// The space between two cards, and between two rows of the list.
const GAP: f32 = 10.0;
const PAD: f32 = 24.0;
const ROW_H: f32 = 64.0;
const BAR_H: f32 = 56.0;
const FOOTER_H: f32 = 40.0;

#[derive(Clone, Copy, PartialEq)]
enum View {
    Grid,
    List,
}

/// The cards' view and order are remembered between runs, in `view.txt` beside the other choices ("list za").
fn load_view() -> (View, Sort) {
    let text = vf_core::platform::data_dir().and_then(|d| std::fs::read_to_string(d.join("view.txt")).ok()).unwrap_or_default();
    let view = if text.split_whitespace().any(|w| w == "list") { View::List } else { View::Grid };
    let sort = if text.split_whitespace().any(|w| w == "za") { Sort::NameDesc } else { Sort::NameAsc };
    (view, sort)
}

fn save_view(view: View, sort: Sort) {
    if let Some(dir) = vf_core::platform::data_dir() {
        let words = format!("{} {}\n", if view == View::List { "list" } else { "grid" }, if sort == Sort::NameDesc { "za" } else { "az" });
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("view.txt"), words);
    }
}

/// The two tabs under the header.
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Apps,
    Recipes,
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
    /// The new hub is in place; this one still runs until it is restarted.
    Restart(PathBuf),
    Failed(String),
}

enum UpdMsg {
    Checked(Result<update::Release, String>),
    Progress(u64, u64),
    Done(Result<PathBuf, String>),
}

/// What the release's pack list says, read together with the update check.
struct Loaded {
    release: update::Release,
    sums: String,
    manifest: packs::Manifest,
    /// What installing each pack would do, by pack id (read once, with the engines found on this PC).
    plans: HashMap<String, packs::Plan>,
    /// The version of each engine installed on this PC, by engine name.
    system: HashMap<String, Option<String>>,
}

enum Packs {
    /// Not looked at yet (the update check is off or has not run).
    Unknown,
    Loading,
    Ready(Box<Loaded>),
    Failed(String),
}

/// An install, update or removal in progress, one at a time.
struct Job {
    app: usize,
    removing: bool,
    rx: mpsc::Receiver<JobMsg>,
    cancel: Arc<AtomicBool>,
    stage: String,
    done: u64,
    total: u64,
}

enum JobMsg {
    Progress(String, u64, u64),
    Installed(Result<packs::Installed, String>),
    Removed(Result<(), String>),
}

struct Hub {
    /// `<hub folder>\apps`: where packs are installed and where `catalog::locate` looks first.
    apps_dir: PathBuf,
    packs: Packs,
    packs_rx: Option<mpsc::Receiver<Result<Box<Loaded>, String>>>,
    job: Option<Job>,
    /// An INSTALL clicked before the pack list was loaded: runs as soon as it arrives.
    pending_install: Option<usize>,
    /// The card whose REMOVE was clicked once and now asks to be confirmed.
    confirm_remove: Option<usize>,
    /// UPDATE ALL: the apps still waiting for their turn (the one running is in `job`).
    queue: Vec<usize>,
    /// What each app's `pack.json` says (None: not installed by this installer), same order as `APPS`.
    installed: Vec<Option<packs::Installed>>,
    status: Vec<Option<Status>>,
    health: Vec<Health>,
    upd: Upd,
    upd_rx: Option<mpsc::Receiver<UpdMsg>>,
    /// Check for a newer release at start (on unless the user turned it off). A check started this way says
    /// nothing when it fails: an offline PC is normal for these apps.
    auto_on: bool,
    quiet: bool,
    query: String,
    view: View,
    sort: Sort,
    /// Where each app's portable build was found, same order as `APPS`.
    found: Vec<Option<PathBuf>>,
    notice: Option<Said>,
    /// Files from later starts of the program (see `single`).
    later_starts: Option<mpsc::Receiver<Vec<PathBuf>>>,
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
    icons: icons::AppIcons,
    mark: icons::Mark,
    /// The settings window and the first-run welcome dialog.
    settings: settings::Settings,
    tab: Tab,
    recipes: recipes::RecipesTab,
    /// "voidflare is now SwapCraft": shown once, right after the hub renamed itself (see `main`).
    renamed_notice: bool,
}

/// Left in the data folder by the hub that renamed itself, for the SwapCraft.exe it starts.
const RENAMED_MARK: &str = "renamed.txt";

impl Hub {
    fn new(ctx: &egui::Context) -> Self {
        // A hub that was just replaced leaves its old self parked next to it; it may still be closing, so try twice.
        std::thread::spawn(|| {
            update::cleanup_after_update();
            std::thread::sleep(std::time::Duration::from_secs(3));
            update::cleanup_after_update();
        });
        // The right-click menu added by an older hub has no drag-and-drop part yet: write it once.
        menu::upgrade_if_needed();
        // 0.5: this hub was voidflare.exe a moment ago (see `main`). The Start menu entry and the right-click menus
        // are pointed at SwapCraft.exe without asking; a notice says what changed.
        let renamed = settings::data_dir().is_some_and(|d| std::fs::remove_file(d.join(RENAMED_MARK)).is_ok());
        // an old voidflare.lnk is replaced by SwapCraft.lnk (a pin to Start is lost: Windows ties it to the file name)
        let _ = vf_core::shortcuts::replace_legacy();
        if renamed && menu::installed() {
            let _ = menu::install();
        }
        let mut hub = Self::build(ctx);
        // test hook, like VOIDFLARE_HUB_TAB=settings: VOIDFLARE_HUB_TAB=renamed shows the notice
        hub.renamed_notice = renamed || std::env::var("VOIDFLARE_HUB_TAB").is_ok_and(|v| v == "renamed");
        // Reading the pack list is part of the update check: only when that check is on, or when the user clicks.
        if hub.auto_on {
            hub.check_updates(ctx, true);
        }
        hub
    }

    fn build(ctx: &egui::Context) -> Self {
        let apps_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join("apps"))).unwrap_or_else(|| PathBuf::from("apps"));
        let mut hub = Self::fresh(ctx, apps_dir);
        hub.refresh_packs();
        hub
    }

    fn fresh(ctx: &egui::Context, apps_dir: PathBuf) -> Self {
        Self {
            apps_dir,
            packs: Packs::Unknown,
            packs_rx: None,
            job: None,
            pending_install: None,
            confirm_remove: None,
            queue: Vec::new(),
            installed: vec![None; APPS.len()],
            status: vec![None; APPS.len()],
            health: vec![Health::NotInstalled; APPS.len()],
            upd: Upd::Idle,
            upd_rx: None,
            auto_on: update::auto_enabled(),
            quiet: false,
            query: String::new(),
            view: load_view().0,
            sort: load_view().1,
            found: APPS.iter().map(catalog::locate).collect(),
            notice: None,
            later_starts: None,
            files: Vec::new(),
            bytes: 0,
            running: false,
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            log: Vec::new(),
            progress: (0, 0),
            last_dir: None,
            icons: icons::AppIcons::new(ctx, &APPS.iter().map(|a| a.id).collect::<Vec<_>>()),
            mark: icons::Mark::new(ctx),
            // test hook, like VOIDFLARE_RELEASES_URL: VOIDFLARE_HUB_TAB=settings starts with the settings open
            settings: {
                let mut s = settings::Settings::new();
                if std::env::var("VOIDFLARE_HUB_TAB").is_ok_and(|v| v == "settings") {
                    s.open();
                }
                s
            },
            // test hook, like VOIDFLARE_RELEASES_URL: start on the RECIPES tab
            tab: if std::env::var("VOIDFLARE_HUB_TAB").is_ok_and(|v| v == "recipes") { Tab::Recipes } else { Tab::Apps },
            recipes: recipes::RecipesTab::default(),
            renamed_notice: false,
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
            Err(e) => Some(Said::with("hub.err_start", &[("name", APPS[i].name), ("err", &e.to_string())])),
        };
    }

    /// Indices of the apps matching the search, in three groups: installed, downloadable, and planned; by name.
    fn visible(&self) -> [Vec<usize>; 3] {
        let q = self.query.trim().to_lowercase();
        let mut idx: Vec<usize> = (0..APPS.len())
            .filter(|&i| {
                let a = &APPS[i];
                q.is_empty() || [a.name.to_owned(), a.tagline(), a.engine.to_owned()].iter().any(|s| s.to_lowercase().contains(&q))
            })
            .collect();
        idx.sort_by(|&a, &b| {
            let o = APPS[a].name.to_lowercase().cmp(&APPS[b].name.to_lowercase());
            if self.sort == Sort::NameDesc { o.reverse() } else { o }
        });
        let mut groups = [Vec::new(), Vec::new(), Vec::new()];
        for i in idx {
            let g = if self.found[i].is_some() { 0 } else if APPS[i].pack { 1 } else { 2 };
            groups[g].push(i);
        }
        groups
    }
}

impl eframe::App for Hub {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        vf_ui::shell::fit_to_screen(ui.ctx());
        let t = Tokens::get(ui.ctx());
        self.poll();
        self.poll_update();
        self.poll_packs(ui.ctx());
        let ctx = ui.ctx().clone();
        self.poll_job(&ctx);
        let handed: Vec<Vec<PathBuf>> = self.later_starts.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        if let Some(files) = handed.into_iter().rev().find(|f| !f.is_empty())
            && !self.running
        {
            self.set_files(&files);
        }
        if self.running {
            ui.ctx().request_repaint_after(Duration::from_millis(60));
        }
        let dropped = dropped_files(ui.ctx());
        if !dropped.is_empty() && !self.running {
            self.set_files(&dropped);
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg)).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            self.header(ui, &t);
            if self.tab == Tab::Recipes {
                self.recipes_tab(ui, &t);
                self.footer(ui, &t);
                return;
            }
            self.drop_panel(ui, &t);
            self.toolbar(ui, &t);
            let [ready, available, planned] = self.visible();
            let body_h = (ui.available_height() - FOOTER_H).max(0.0);
            egui::ScrollArea::vertical().max_height(body_h).auto_shrink([false, false]).show(ui, |ui| {
                if ready.is_empty() && available.is_empty() && planned.is_empty() {
                    ui.add_space(48.0);
                    ui.vertical_centered(|ui| ui.label(RichText::new(tr("hub.no_match")).color(t.muted)));
                    return;
                }
                for (label, group) in [(tr("hub.group_installed"), &ready), (tr("hub.group_available"), &available), (tr("hub.group_soon"), &planned)] {
                    if group.is_empty() {
                        continue;
                    }
                    section_label(ui, &t, &label, group.len());
                    match self.view {
                        View::Grid => self.grid(ui, &t, group),
                        View::List => self.list(ui, &t, group),
                    }
                }
            });
            self.footer(ui, &t);
        });
        let ctx = ui.ctx().clone();
        self.settings.ui(&ctx, &t, &mut self.auto_on);
        if self.renamed_notice && !self.settings.show_welcome() {
            let mut close = false;
            let resp = egui::Modal::new(egui::Id::unique("renamed")).show(&ctx, |ui| {
                ui.set_width(400.0);
                ui.add_space(4.0);
                ui.label(RichText::new(tr("hub.renamed_title")).font(strong(18.0)).color(t.ink_strong));
                ui.add_space(8.0);
                ui.label(RichText::new(tr("hub.renamed_text")).font(FontId::proportional(14.0)).color(t.ink));
                ui.add_space(6.0);
                ui.label(RichText::new(tr("hub.renamed_pin")).font(FontId::proportional(12.5)).color(t.muted));
                ui.add_space(16.0);
                if widgets::primary(ui, &t, &tr("hub.renamed_ok"), true).clicked() {
                    close = true;
                }
            });
            if close || resp.should_close() {
                self.renamed_notice = false;
            }
        }
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
            let unnamed_file = paths.first().is_some_and(|p| p.is_file() && p.extension().is_none());
            Some(if unnamed_file {
                Said::key("hub.no_extension")
            } else if ext.is_empty() {
                Said::key("hub.nothing_convertible")
            } else {
                Said::with("hub.no_converter", &[("ext", &ext)])
            })
        } else {
            None
        };
    }

    /// Look at the project's latest GitHub release: one HTTPS request, nothing sent about the user. Done when the
    /// user clicks, and at start unless they turned that off (`quiet`: no error message if it fails).
    fn check_updates(&mut self, ctx: &egui::Context, quiet: bool) {
        let (tx, rx) = mpsc::channel();
        let (ptx, prx) = mpsc::channel();
        self.quiet = quiet;
        self.upd = Upd::Checking;
        self.upd_rx = Some(rx);
        self.packs = Packs::Loading;
        self.packs_rx = Some(prx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // One request to GitHub answers both questions: is there a newer voidflare, and which packs does it carry.
            let started = std::time::Instant::now();
            let latest = update::latest();
            let loaded = latest.as_ref().map_err(Clone::clone).and_then(load_packs);
            // GitHub often answers in a fraction of a second: after a click, "Checking…" stays long enough to be
            // seen, or the click looks like it did nothing
            let shown = std::time::Duration::from_millis(800);
            if !quiet && started.elapsed() < shown {
                std::thread::sleep(shown - started.elapsed());
            }
            let _ = tx.send(UpdMsg::Checked(latest));
            let _ = ptx.send(loaded);
            ctx.request_repaint();
        });
    }

    /// Re-reads what is installed on disk and works out each card's status and engine health.
    fn refresh_packs(&mut self) {
        self.found = APPS.iter().map(catalog::locate).collect();
        for (i, app) in APPS.iter().enumerate() {
            self.installed[i] = packs::installed(&self.apps_dir, app.id);
            self.status[i] = None;
            self.health[i] = Health::NotInstalled;
            let Packs::Ready(l) = &self.packs else { continue };
            let Some(pack) = l.manifest.packs.iter().find(|p| p.id == app.id) else { continue };
            if self.installed[i].is_some() {
                self.status[i] = Some(packs::status(&self.apps_dir, pack, &l.sums));
                let sys = pack.engine.as_ref().and_then(|e| l.system.get(&e.name)).cloned().flatten();
                self.health[i] = packs::health(&self.apps_dir, pack, sys.as_deref());
            }
        }
    }

    fn poll_packs(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.packs_rx else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.packs_rx = None;
        match result {
            Ok(loaded) => {
                self.packs = Packs::Ready(loaded);
                self.refresh_packs();
                if let Some(i) = self.pending_install.take() {
                    self.start_install(ctx, i);
                }
            }
            Err(e) => {
                self.packs = Packs::Failed(e.clone());
                if self.pending_install.take().is_some() {
                    self.notice = Some(Said::with("hub.cannot_install", &[("err", &e)]));
                }
            }
        }
    }

    /// INSTALL or UPDATE on a card. Needs the pack list: asks for it first if it has not been read yet.
    fn request_install(&mut self, ctx: &egui::Context, i: usize) {
        self.notice = None;
        match &self.packs {
            Packs::Ready(_) => self.start_install(ctx, i),
            Packs::Loading => self.pending_install = Some(i),
            Packs::Unknown | Packs::Failed(_) => {
                self.pending_install = Some(i);
                self.check_updates(ctx, true);
            }
        }
    }

    fn start_install(&mut self, ctx: &egui::Context, i: usize) {
        let Packs::Ready(l) = &self.packs else { return };
        if self.job.is_some() {
            return;
        }
        let Some(pack) = l.manifest.packs.iter().find(|p| p.id == APPS[i].id).cloned() else {
            self.notice = Some(Said::with("hub.not_in_release", &[("app", APPS[i].name)]));
            return;
        };
        let (release, sums, apps_dir) = (l.release.clone(), l.sums.clone(), self.apps_dir.clone());
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = Job { app: i, removing: false, rx, cancel: cancel.clone(), stage: "app".into(), done: 0, total: 0 };
        self.job = Some(job);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // The PC's engine is looked at now, not when the list was read: it may have changed since.
            let system = pack.engine.as_ref().and_then(packs::system_engine_version);
            let engine = pack.engine.as_ref().map(|spec| packs::choose_engine(spec, system.as_deref()));
            let progress = |stage: &str, done: u64, total: u64| {
                let _ = tx.send(JobMsg::Progress(stage.to_owned(), done, total));
                ctx.request_repaint();
            };
            let result = packs::install(&apps_dir, &release, &sums, &pack, engine.as_ref(), &progress, &cancel);
            let _ = tx.send(JobMsg::Installed(result));
            ctx.request_repaint();
        });
    }

    fn start_remove(&mut self, ctx: &egui::Context, i: usize) {
        if self.job.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.job = Some(Job { app: i, removing: true, rx, cancel: Arc::new(AtomicBool::new(false)), stage: "remove".into(), done: 0, total: 0 });
        let (apps_dir, id, ctx) = (self.apps_dir.clone(), APPS[i].id, ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(JobMsg::Removed(packs::remove(&apps_dir, id)));
            ctx.request_repaint();
        });
    }

    /// The installed apps that have a newer pack (or a tested engine that moved), same order as `APPS`.
    fn updatable(&self) -> Vec<usize> {
        (0..APPS.len()).filter(|&i| matches!(self.status[i], Some(Status::UpdateAvailable))).collect()
    }

    /// UPDATE ALL: updates every app that can be updated, one after the other.
    fn update_all(&mut self, ctx: &egui::Context) {
        if self.job.is_some() {
            return;
        }
        self.notice = None;
        self.queue = self.updatable();
        self.next_queued(ctx);
    }

    fn next_queued(&mut self, ctx: &egui::Context) {
        if !self.queue.is_empty() {
            let i = self.queue.remove(0);
            self.start_install(ctx, i);
        }
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.job else { return };
        let mut finished = None;
        while let Ok(msg) = job.rx.try_recv() {
            match msg {
                JobMsg::Progress(stage, done, total) => (job.stage, job.done, job.total) = (stage, done, total),
                JobMsg::Installed(r) => finished = Some((job.app, r.map(|_| ()), false)),
                JobMsg::Removed(r) => finished = Some((job.app, r, true)),
            }
        }
        if let Some((app, result, removed)) = finished {
            self.job = None;
            let result_was_cancel = matches!(&result, Err(e) if e.contains("ancel"));
            self.notice = match result {
                Ok(()) => None,
                Err(e) if e.contains("ancel") => None,
                Err(e) => Some(Said::raw(format!("{}: {e}", APPS[app].name))),
            };
            let _ = removed;
            self.refresh_packs();
            // a failed or cancelled update stops the rest of UPDATE ALL; a good one hands over to the next app
            if self.notice.is_some() || result_was_cancel {
                self.queue.clear();
            } else {
                self.next_queued(ctx);
            }
        }
    }

    /// The user asked: download the new hub, verify its published checksum and swap it in place of this one.
    fn download_update(&mut self, ctx: &egui::Context, release: update::Release) {
        let (tx, rx) = mpsc::channel();
        self.upd = Upd::Downloading(0, release.update_zip().map_or(0, |z| z.size));
        self.upd_rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let progress = |done, total| {
                let _ = tx.send(UpdMsg::Progress(done, total));
                ctx.request_repaint();
            };
            let result = update::install_hub(&release, &progress, &AtomicBool::new(false));
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
                    self.upd = if self.quiet { Upd::Idle } else { Upd::Failed(e) };
                    self.quiet = false;
                    finished = true;
                }
                UpdMsg::Progress(done, total) => self.upd = Upd::Downloading(done, total),
                UpdMsg::Done(Ok(path)) => {
                    // Nothing is running that the restart would cut short: start the new hub right away.
                    if self.job.is_none() && !self.running {
                        restart_into(&path);
                    }
                    self.upd = Upd::Restart(path);
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
            self.log.push((trf("hub.skipped", &[("n", &other.len().to_string())]), Kind::Plain));
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
        let labels: Vec<String> = self.files.first().map_or(vec![], |f| routes::targets(&routes::ext_of(f)).iter().map(|&i| routes::shown_label(routes::ROUTES[i].label)).collect());
        // the chips wrap inside the card less its 18 px inner margin on each side
        let rows = chip_rows(ui, &labels, ui.available_width() - 2.0 * PAD - 36.0);
        let recipe_rows = match self.files.first() {
            Some(f) if !self.recipes.accepting(&routes::ext_of(f)).is_empty() => 66.0,
            _ => 0.0,
        };
        let height = if has_files { 176.0 + 36.0 * rows as f32 + recipe_rows } else { 176.0 };
        let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), height + 8.0 + 16.0), Sense::hover());
        let rect = Rect::from_min_max(outer.min + vec2(PAD, 8.0), outer.max - vec2(PAD, 16.0));
        let zone = ui.interact(rect, ui.scope_id().with("drop-zone"), if has_files || self.running { Sense::hover() } else { Sense::click() });
        let lit = ui.ctx().animate_bool_with_time(ui.scope_id().with("drop-lit"), hovering || (zone.hovered() && !has_files), 0.2);
        // A card like the app cards; it takes the indigo of the flame while a file is over it.
        ui.painter().rect(
            rect,
            CornerRadius::same(12),
            t.card.lerp_to_gamma(t.accent, 0.08 * lit),
            egui::Stroke::new(1.0, t.line.lerp_to_gamma(t.accent, 0.8 * lit)),
            StrokeKind::Inside,
        );
        if zone.hovered() && !has_files {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if zone.clicked()
            && let Some(picked) = rfd::FileDialog::new().set_title(tr("hub.pick_title")).add_filter(tr("hub.pick_filter"), &routes::known_exts()).pick_files()
        {
            self.set_files(&picked);
        }

        if !has_files {
            let p = ui.painter();
            let c = rect.center();
            p.text(pos2(c.x, c.y - 26.0), Align2::CENTER_CENTER, tr("hub.drop_title"), strong(21.0), t.ink_strong);
            p.text(pos2(c.x, c.y + 6.0), Align2::CENTER_CENTER, tr("hub.drop_sub"), FontId::proportional(14.0), t.muted);
            p.text(pos2(c.x, c.y + 44.0), Align2::CENTER_CENTER, "DOCX · XLSX · PPTX · PDF · DWG · PNG · MP3 · MP4 · EPUB", mono(11.0), t.faint);
            if let Some(msg) = self.notice.as_ref().map(Said::text) {
                let galley = one_line(ui, msg, FontId::proportional(12.5), t.bad, rect.width() - 32.0);
                let at = pos2(c.x - galley.size().x / 2.0, rect.bottom() - 18.0 - galley.size().y / 2.0);
                ui.painter().galley(at, galley, t.bad);
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
            ui.label(RichText::new(tr("hub.ready")).font(FontId::proportional(12.5)).color(t.muted));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::button(ui, &tr("hub.clear")).clicked() && !self.running {
                    self.files.clear();
                    self.log.clear();
                    self.last_dir = None;
                }
                if let Some(dir) = self.last_dir.clone()
                    && widgets::button(ui, &tr("hub.open_folder")).clicked()
                {
                    vf_core::platform::open_folder(dir);
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
                    // one line, cut with "..." when it is too long (a very long name used to wrap over four lines and push
                    // the format buttons down); the whole name is in the tooltip
                    let full = name(&self.files[0]);
                    ui.add(egui::Label::new(RichText::new(&full).font(strong(17.0)).color(t.ink_strong)).truncate()).on_hover_text(full);
                    let dir = self.files[0].parent().map(|p| p.display().to_string()).unwrap_or_default();
                    ui.label(RichText::new(format!("{} · {}", human_size(self.bytes), dir.trim_start_matches(r"\\?\"))).font(mono(11.0)).color(t.faint));
                } else {
                    ui.label(RichText::new(trf("hub.n_files", &[("n", &n.to_string()), ("ext", &ext.to_uppercase())])).font(strong(17.0)).color(t.ink_strong));
                    let shown: Vec<String> = self.files.iter().take(3).map(name).collect();
                    let more = if n > 3 { format!("  {}", trf("hub.more", &[("n", &(n - 3).to_string())])) } else { String::new() };
                    ui.label(RichText::new(format!("{}{more}", shown.join("  ·  "))).size(13.0).color(t.muted));
                    ui.label(RichText::new(trf("hub.total", &[("size", &human_size(self.bytes))])).font(mono(11.0)).color(t.faint));
                }
            });
        });
        ui.add_space(12.0);
        let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().hline(line.x_range(), line.center().y, t.hairline());
        ui.add_space(10.0);
        ui.label(RichText::new(tr("hub.convert_to")).font(FontId::proportional(12.5)).color(t.muted));
        ui.add_space(6.0);
        let mut chosen = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for i in routes::targets(&ext) {
                let route = &routes::ROUTES[i];
                let more = APPS.iter().find(|a| a.id == route.app).map_or(route.app, |a| a.name);
                let button = chip(ui, t, &routes::shown_label(route.label), !self.running).on_hover_text(trf("hub.default_hint", &[("app", more)]));
                if button.clicked() {
                    chosen = Some(i);
                }
            }
        });
        if let Some(i) = chosen {
            self.convert(i);
        }
        // The recipes that take this kind of file.
        let recipes = self.recipes.accepting(&ext);
        if !recipes.is_empty() {
            ui.add_space(12.0);
            ui.label(RichText::new(tr("hub.or_recipe")).font(FontId::proportional(12.5)).color(t.muted));
            ui.add_space(6.0);
            let mut run = None;
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                for r in &recipes {
                    let label = RichText::new(format!("▶ {}", r.name)).font(FontId::proportional(13.0)).color(t.engine_ink);
                    if ui.add_enabled(!self.running, egui::Button::new(label)).on_hover_text(r.steps.iter().map(|s| s.summary()).collect::<Vec<_>>().join(" → ")).clicked() {
                        run = Some(r.clone());
                    }
                }
            });
            if let Some(r) = run {
                let files: Vec<PathBuf> = self.files.iter().filter(|f| r.accepts(&routes::ext_of(f))).cloned().collect();
                self.start_recipe(r, files);
            }
        }
    }
}

/// `text` on one line no wider than `max_w`, ending in "…" when it had to be cut (`elided` says so).
fn one_line(ui: &egui::Ui, text: String, font: FontId, color: Color32, max_w: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text, font, color);
    job.wrap = egui::text::TextWrapping { max_width: max_w.max(10.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.fonts_mut(|f| f.layout_job(job))
}

fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// How many rows the format chips take in `width`: each chip is its text (13 px) plus the button padding, 8 px
/// apart, filled left to right as `horizontal_wrapped` does. Measured with the real font, so a long label in any
/// language or a narrow window still gets the room it needs.
fn chip_rows(ui: &egui::Ui, labels: &[String], width: f32) -> usize {
    let pad = 2.0 * ui.spacing().button_padding.x;
    let (mut rows, mut x) = (1, 0.0);
    for label in labels {
        let w = ui.fonts_mut(|f| f.layout_no_wrap(label.clone(), FontId::proportional(13.0), Color32::WHITE).size().x) + pad;
        if x > 0.0 && x + w > width {
            rows += 1;
            x = 0.0;
        }
        x += w + 8.0;
    }
    rows
}

/// A format chip in the drop panel: a grey pill that turns warm white under the pointer.
fn chip(ui: &mut egui::Ui, t: &Tokens, label: &str, enabled: bool) -> egui::Response {
    let id = ui.next_auto_id();
    let over = ui.ctx().read_response(id).is_some_and(|r| r.hovered()) && enabled;
    let (fill, ink) = if over { (t.invert_bg, t.invert_fg) } else { (t.chip, t.ink) };
    let b = egui::Button::new(RichText::new(label).font(FontId::proportional(13.0)).color(ink)).fill(fill).stroke(egui::Stroke::NONE).min_size(vec2(0.0, 32.0));
    ui.add_enabled(enabled, b)
}

/// A pill-shaped button drawn at `rect`. `solid`: warm white with dark text (or `accent` under the pointer when it
/// is given); otherwise an outline in `ink`.
fn pill(ui: &mut egui::Ui, p: &egui::Painter, t: &Tokens, rect: Rect, id: egui::Id, label: &str, solid: bool, ink: Color32, enabled: bool) -> egui::Response {
    let r = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    let over = r.hovered() && enabled;
    if over {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let round = CornerRadius::same((rect.height() / 2.0) as u8);
    if !enabled {
        p.rect(rect, round, t.chip, egui::Stroke::NONE, StrokeKind::Inside);
        p.text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(12.5), t.faint);
    } else if solid || over {
        p.rect_filled(rect, round, t.invert_bg);
        p.text(rect.center(), Align2::CENTER_CENTER, label, strong(12.5), t.invert_fg);
    } else {
        p.rect(rect, round, Color32::TRANSPARENT, egui::Stroke::new(1.0, t.line_strong), StrokeKind::Inside);
        p.text(rect.center(), Align2::CENTER_CENTER, label, strong(12.5), ink);
    }
    r
}

/// The width a pill needs for `label`.
fn pill_w(ui: &egui::Ui, label: &str) -> f32 {
    ui.painter().layout_no_wrap(label.to_owned(), strong(12.5), Color32::WHITE).size().x + 30.0
}

/// A text link in the footer whose right edge is at `right`; opens `url` in the browser. Returns its left edge.
fn footer_link(ui: &mut egui::Ui, t: &Tokens, bar: Rect, right: f32, label: &str, url: &str) -> f32 {
    let galley = ui.painter().layout_no_wrap(label.to_owned(), FOOT(), t.faint);
    let rect = Rect::from_min_size(pos2(right - galley.size().x, bar.top()), vec2(galley.size().x, bar.height()));
    let resp = ui.interact(rect, ui.scope_id().with(("footer-link", url)), Sense::click());
    let color = if resp.hovered() { t.ink_strong } else { t.engine_ink };
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().text(pos2(rect.left(), bar.center().y), Align2::LEFT_CENTER, label, FOOT(), color);
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

/// The file's icon: a rounded tile with the extension in its colour, with a second tile behind it for several files.
fn file_tile(ui: &mut egui::Ui, t: &Tokens, ext: &str, count: usize) {
    let (rect, _) = ui.allocate_exact_size(vec2(60.0, 66.0), Sense::hover());
    let tile = Rect::from_min_size(rect.min + vec2(4.0, 8.0), vec2(48.0, 56.0));
    let color = kind_color(ext);
    let p = ui.painter();
    if count > 1 {
        p.rect_filled(tile.translate(vec2(6.0, -6.0)), CornerRadius::same(9), t.chip.gamma_multiply(0.6));
    }
    p.rect_filled(tile, CornerRadius::same(9), t.chip);
    p.text(tile.center(), Align2::CENTER_CENTER, ext.to_uppercase(), mono(if ext.len() > 4 { 10.0 } else { 12.0 }), color);
}

/// «SwapCraft by voidflare»: «Swap» in the text colour, «Craft» shading through the voidflare indigos letter by
/// letter, and «by voidflare» small underneath. `left` is the left edge, centred on `left.y`. Returns the area used.
fn wordmark(p: &egui::Painter, t: &Tokens, left: egui::Pos2) -> Rect {
    let big = strong(26.0);
    let swap = p.layout_no_wrap("Swap".to_owned(), big.clone(), t.ink_strong);
    let by = p.layout_no_wrap("by voidflare".to_owned(), FontId::proportional(12.0), t.muted);
    let size = swap.size();
    let gap = 1.0;
    let top = left.y - (size.y + gap + by.size().y) / 2.0;
    p.galley(pos2(left.x, top), swap, t.ink_strong);
    let (from, to) = if t.dark { (Color32::from_rgb(0x81, 0x8c, 0xf8), Color32::from_rgb(0xc7, 0xd2, 0xfe)) } else { (Color32::from_rgb(0x43, 0x38, 0xca), Color32::from_rgb(0x63, 0x66, 0xf1)) };
    let mut x = left.x + size.x;
    let craft = ["C", "r", "a", "f", "t"];
    for (i, ch) in craft.iter().enumerate() {
        let color = from.lerp_to_gamma(to, i as f32 / (craft.len() - 1) as f32);
        let g = p.layout_no_wrap((*ch).to_owned(), big.clone(), color);
        let w = g.size().x;
        p.galley(pos2(x, top), g, color);
        x += w;
    }
    let by_size = by.size();
    p.galley(pos2(left.x + 1.0, top + size.y + gap), by, t.muted);
    Rect::from_min_max(pos2(left.x, top), pos2(x.max(left.x + by_size.x), top + size.y + gap + by_size.y))
}

/// The footer's type.
#[allow(non_snake_case)]
fn FOOT() -> FontId {
    FontId::proportional(12.0)
}

fn section_label(ui: &mut egui::Ui, t: &Tokens, label: &str, n: usize) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    ui.painter().text(pos2(rect.left() + PAD, rect.bottom() - 12.0), Align2::LEFT_BOTTOM, format!("{label} · {n}"), FontId::proportional(13.0), t.muted);
}

impl Hub {
    fn header(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), BAR_H + 20.0), Sense::hover());
        let c = pos2(rect.left() + PAD, rect.center().y + 4.0);
        // the gear sits in the top right corner; the tagline gives way to it
        let gear = Rect::from_center_size(pos2(rect.right() - PAD - 14.0, c.y), vec2(32.0, 32.0));
        if !self.settings.show_welcome() && settings::gear_button(ui, t, gear) {
            self.settings.open();
        }
        let w = self.mark.paint(ui.painter(), c.x, c.y, 44.0);
        let word = wordmark(ui.painter(), t, pos2(c.x + w + 12.0, c.y));
        // Apps | Recipes: pills right of the wordmark.
        let mut x = word.right() + 32.0;
        for (tab, label) in [(Tab::Apps, tr("hub.tab_apps")), (Tab::Recipes, tr("hub.tab_recipes"))] {
            let galley = ui.painter().layout_no_wrap(label.clone(), FontId::proportional(13.5), t.ink);
            let r = Rect::from_min_size(pos2(x, c.y - 15.0), vec2(galley.size().x + 28.0, 30.0));
            let resp = ui.interact(r, ui.scope_id().with(("tab", label.as_str())), Sense::click());
            let on = self.tab == tab;
            let p = ui.painter();
            if on {
                p.rect_filled(r, CornerRadius::same(15), t.chip);
            }
            let color = if on || resp.hovered() { t.ink_strong } else { t.muted };
            p.text(r.center(), Align2::CENTER_CENTER, &label, if on { strong(13.5) } else { FontId::proportional(13.5) }, color);
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.clicked() {
                self.tab = tab;
            }
            x = r.right() + 4.0;
        }
        let p = ui.painter();
        // The tagline gives way to the tabs in a narrow window.
        let tagline = "Craft your conversion, always offline.";
        let tag_w = p.layout_no_wrap(tagline.to_owned(), FontId::proportional(13.0), t.muted).size().x;
        if gear.left() - 16.0 - tag_w > x + 24.0 {
            p.text(pos2(gear.left() - 16.0, c.y), Align2::RIGHT_CENTER, tagline, FontId::proportional(13.0), t.muted);
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), BAR_H), Sense::hover());
        let mut bar = ui.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(PAD, 0.0))).layout(Layout::left_to_right(Align::Center)));
        bar.spacing_mut().item_spacing.x = 12.0;
        bar.add(egui::TextEdit::singleline(&mut self.query).hint_text(tr("hub.search")).font(FontId::proportional(13.5)).desired_width(260.0).margin(vec2(14.0, 8.0)));
        // UPDATE ALL shows only when two or more installed apps have an update (with one, its card has UPDATE)
        let n_update = self.updatable().len();
        let queued = !self.queue.is_empty() || self.job.as_ref().is_some_and(|j| !j.removing && self.status[j.app] == Some(Status::UpdateAvailable));
        let mut go_update_all = false;
        let before = (self.view, self.sort);
        bar.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.selectable_value(&mut self.view, View::List, RichText::new(tr("hub.list")).font(FontId::proportional(13.0)));
            ui.selectable_value(&mut self.view, View::Grid, RichText::new(tr("hub.grid")).font(FontId::proportional(13.0)));
            ui.add_space(8.0);
            let (asc, desc) = (tr("hub.sort_az"), tr("hub.sort_za"));
            let label = if self.sort == Sort::NameAsc { asc.clone() } else { desc.clone() };
            egui::ComboBox::from_id_salt("sort").selected_text(RichText::new(label).font(FontId::proportional(13.0))).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.sort, Sort::NameAsc, asc);
                ui.selectable_value(&mut self.sort, Sort::NameDesc, desc);
            });
            if n_update >= 2 && !queued && self.job.is_none() {
                ui.add_space(8.0);
                let label = trf("hub.update_all", &[("n", &n_update.to_string())]);
                let b = egui::Button::new(RichText::new(label).font(strong(12.5)).color(t.invert_fg)).fill(t.invert_bg);
                if ui.add(b).on_hover_text(tr("hub.update_all_hint")).clicked() {
                    go_update_all = true;
                }
            }
        });
        if (self.view, self.sort) != before {
            save_view(self.view, self.sort);
        }
        if go_update_all {
            let ctx = ui.ctx().clone();
            self.update_all(&ctx);
        }
    }

    fn footer(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), FOOTER_H), Sense::hover());
        ui.painter().hline(rect.x_range(), rect.top() + 0.5, egui::Stroke::new(1.0, t.line));
        // Our site, a link like the ArtCraft one on the right.
        const SITE: &str = "swapcraft.voidflare.app";
        let site_w = ui.painter().layout_no_wrap(SITE.to_owned(), FOOT(), t.faint).size().x;
        let site_left = rect.left() + PAD;
        footer_link(ui, t, rect, site_left + site_w, SITE, "https://swapcraft.voidflare.app/");
        let site = Rect::from_min_size(pos2(site_left, rect.top()), vec2(site_w, rect.height()));

        // Updates: only on a click, and the label says what the click will do.
        let (label, color, hint): (String, Color32, String) = match &self.upd {
            Upd::Idle => (tr("hub.upd_check"), t.muted, tr("hub.upd_check_hint")),
            Upd::Checking => (tr("hub.upd_checking"), t.faint, String::new()),
            Upd::UpToDate => (tr("hub.upd_uptodate"), t.good, String::new()),
            Upd::Available(r) => match r.update_zip() {
                Some(z) if r.sums.is_some() => (trf("hub.upd_now", &[("v", &r.version), ("size", &human_size(z.size))]), t.accent_ink, tr("hub.upd_now_hint")),
                _ => (trf("hub.upd_link", &[("v", &r.version)]), t.accent_ink, tr("hub.upd_link_hint")),
            },
            Upd::Downloading(done, total) => (trf("hub.upd_progress", &[("pct", &(if *total > 0 { done * 100 / total } else { 0 }).to_string())]), t.faint, String::new()),
            Upd::Restart(_) => (tr("hub.upd_restart"), t.good, tr("hub.upd_restart_hint")),
            Upd::Failed(msg) => (trf("hub.upd_failed", &[("msg", &msg.chars().take(70).collect::<String>())]), t.bad, String::new()),
        };
        let clickable = !matches!(self.upd, Upd::Checking | Upd::Downloading(..));
        // Right after the site's name, with the same gap at any length of it.
        let x = site.right() + 24.0;
        let galley = ui.painter().layout_no_wrap(label.clone(), FOOT(), color);
        let action = Rect::from_min_size(pos2(x, rect.top()), vec2(galley.size().x, rect.height()));
        let resp = ui.interact(action, ui.scope_id().with("update-action"), if clickable { Sense::click() } else { Sense::hover() });
        let shown = if resp.hovered() && clickable { t.ink_strong } else { color };
        ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, &label, FOOT(), shown);
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
                Upd::Idle | Upd::UpToDate => self.check_updates(&ctx, false),
                Upd::Available(r) if r.update_zip().is_some() && r.sums.is_some() => self.download_update(&ctx, r),
                Upd::Available(r) => {
                    ctx.open_url(egui::OpenUrl::new_tab(&r.page));
                    self.upd = Upd::Available(r);
                }
                Upd::Restart(path) => restart_into(&path),
                Upd::Failed(_) => {}
                other => self.upd = other,
            }
        }

        // The switch for the check at start, right after the update label.
        let auto_label = if self.auto_on { tr("hub.auto_on") } else { tr("hub.auto_off") };
        let auto_x = x + galley.size().x + 24.0;
        let auto_w = ui.painter().layout_no_wrap(auto_label.clone(), FOOT(), t.faint).size().x;
        let auto_rect = Rect::from_min_size(pos2(auto_x, rect.top()), vec2(auto_w, rect.height()));
        let auto_resp = ui.interact(auto_rect, ui.scope_id().with("auto-check"), Sense::click());
        let auto_color = if auto_resp.hovered() { t.ink_strong } else { t.muted };
        ui.painter().text(pos2(auto_x, rect.center().y), Align2::LEFT_CENTER, &auto_label, FOOT(), auto_color);
        if auto_resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        auto_resp.clone().on_hover_text(tr("hub.auto_hint"));
        if auto_resp.clicked() {
            self.auto_on = !self.auto_on;
            update::set_auto(self.auto_on);
        }

        match self.notice.as_ref().map(Said::text) {
            Some(msg) => {
                // in the room left of the update controls: cut with "…" when the window is narrow, whole on hover
                let room = rect.right() - PAD - (auto_x + auto_w) - 24.0;
                if room > 40.0 {
                    let galley = one_line(ui, msg.clone(), FOOT(), t.bad, room);
                    let at = pos2(rect.right() - PAD - galley.size().x, rect.center().y - galley.size().y / 2.0);
                    let cut = galley.elided;
                    let area = Rect::from_min_size(at, galley.size());
                    ui.painter().galley(at, galley, t.bad);
                    if cut {
                        ui.interact(area, ui.scope_id().with("footer-notice"), Sense::hover()).on_hover_text(msg);
                    }
                }
            }
            None => {
                // Right to left: the link to ArtCraft, then the version. In a narrow window they give way to the update
                // controls on the left instead of being drawn over them.
                let room = rect.right() - PAD - (auto_x + auto_w) - 24.0;
                // widths are measured, because the text is longer in some languages
                let (built, version) = (tr("hub.built_on"), trf("hub.version_line", &[("v", env!("CARGO_PKG_VERSION"))]));
                let built_w = ui.painter().layout_no_wrap(built.clone(), FOOT(), t.faint).size().x;
                let version_w = ui.painter().layout_no_wrap(version.clone(), FOOT(), t.faint).size().x;
                if room > built_w + 10.0 {
                    let link = footer_link(ui, t, rect, rect.right() - PAD, &built, "https://getartcraft.com");
                    if room > built_w + version_w + 30.0 {
                        ui.painter().text(pos2(link - 14.0, rect.center().y), Align2::RIGHT_CENTER, version, FOOT(), t.faint);
                    }
                }
            }
        };
    }

    fn grid(&mut self, ui: &mut egui::Ui, t: &Tokens, group: &[usize]) {
        let width = ui.available_width() - 2.0 * PAD;
        let cols = (((width + GAP) / (CARD_MIN_W + GAP)).floor() as usize).max(1);
        let card_w = (width - GAP * (cols as f32 - 1.0)) / cols as f32;
        let rows = group.len().div_ceil(cols);
        let height = rows as f32 * (CARD_H + GAP) - GAP;
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        for (n, &i) in group.iter().enumerate() {
            let (row, col) = (n / cols, n % cols);
            let min = area.min + vec2(PAD + col as f32 * (card_w + GAP), row as f32 * (CARD_H + GAP));
            self.card(ui, t, i, Rect::from_min_size(min, vec2(card_w, CARD_H)));
        }
    }

    fn card(&mut self, ui: &mut egui::Ui, t: &Tokens, i: usize, rect: Rect) {
        let app = &APPS[i];
        let ready = self.found[i].is_some();
        let resp = ui.interact(rect, ui.scope_id().with(("card", app.id)), if ready { Sense::click() } else { Sense::hover() });
        // test hook, like VOIDFLARE_HUB_TAB: VOIDFLARE_HUB_HOVER=<app id> draws that card as if the pointer were on it
        let forced = std::env::var("VOIDFLARE_HUB_HOVER").is_ok_and(|v| v == app.id);
        let hovered = resp.hovered() || ui.rect_contains_pointer(rect) || forced;
        if ready && resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let lift = ui.ctx().animate_bool_with_time(ui.scope_id().with(("lift", app.id)), hovered, 0.2);
        let color = rgb(app.color);
        // Apps that are not available are drawn in greys, and in colour while the pointer is over them.
        let accent = if ready { color } else { t.muted.lerp_to_gamma(color, lift) };

        // The card lights up in the app's own colour: a tint, the border, and a bar along the top.
        let p = ui.painter_at(rect.expand(1.0));
        p.rect(
            rect,
            CornerRadius::same(12),
            t.card.lerp_to_gamma(accent, 0.07 * lift),
            egui::Stroke::new(1.0, t.line.lerp_to_gamma(accent, 0.55 * lift)),
            StrokeKind::Inside,
        );
        if lift > 0.0 {
            let w = (rect.width() - 24.0) * lift;
            p.rect_filled(Rect::from_min_size(pos2(rect.center().x - w / 2.0, rect.top()), vec2(w, 2.0)), CornerRadius::same(1), accent);
        }
        let inner = rect.shrink(18.0);

        // Icon, name and tagline.
        let tile = Rect::from_min_size(inner.min, vec2(48.0, 48.0));
        let tint = if ready { Color32::WHITE } else { Color32::from_gray(150).lerp_to_gamma(Color32::WHITE, lift) };
        self.icons.paint(&p, i, tile, tint, t.line);
        let text_x = tile.right() + 14.0;
        let text_w = inner.right() - text_x;
        p.text(pos2(text_x, tile.center().y - 10.0), Align2::LEFT_CENTER, app.name, strong(16.5), t.ink_strong);
        let tagline = clipped(&app.tagline(), FontId::proportional(13.0), t.muted, text_w, 1);
        p.galley(pos2(text_x, tile.center().y + 2.0), p.layout_job(tagline), t.muted);

        // Description, two lines at most.
        let blurb = clipped(&app.blurb(), FontId::proportional(12.5), t.muted, inner.width(), 2);
        p.galley(pos2(inner.left(), tile.bottom() + 12.0), p.layout_job(blurb), t.muted);

        // The foot: the engine and its status on the left, the actions on the right.
        let foot_y = inner.bottom() - 15.0;
        let button_h = 30.0;
        let ctx = ui.ctx().clone();
        let working = self.job.as_ref().is_some_and(|j| j.app == i);
        let busy_elsewhere = self.job.is_some() && !working;
        // Where the engine comes from, in small print above the foot.
        let line = if working { self.job_line(i) } else { self.source_line(i) };
        if let Some((text, warn)) = line {
            let color = if warn { t.bad } else if working { t.engine_ink } else { t.faint };
            p.text(pos2(inner.left(), foot_y - 26.0), Align2::LEFT_CENTER, text, mono(10.5), color);
        }

        let scope = ui.scope_id();
        let id = |what: &str| scope.with((what, app.id));
        let mut handled = false;
        let (mut act_install, mut act_remove, mut act_cancel) = (false, false, false);
        let small_link = |ui: &mut egui::Ui, p: &egui::Painter, rect: Rect, salt: &str, label: &str, base: Color32| -> bool {
            let r = ui.interact(rect, id(salt), Sense::click());
            let color = if r.hovered() { t.ink_strong } else { base };
            if r.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            p.text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(12.5), color);
            r.clicked()
        };
        // the right edge of the room left for the engine text in the foot
        let mut actions_left: f32;
        if working {
            let job = self.job.as_ref().expect("working implies a job");
            let cancel_w = if job.removing { 0.0 } else { 70.0 };
            let bar = Rect::from_min_size(pos2(inner.left(), foot_y - 2.0), vec2(inner.width() - cancel_w, 4.0));
            p.rect_filled(bar, CornerRadius::same(2), t.chip);
            let frac = if job.total > 0 { (job.done as f32 / job.total as f32).clamp(0.0, 1.0) } else { 0.0 };
            p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * frac, bar.height())), CornerRadius::same(2), accent);
            if !job.removing {
                let cancel_rect = Rect::from_min_size(pos2(inner.right() - cancel_w + 8.0, foot_y - button_h / 2.0), vec2(cancel_w - 8.0, button_h));
                act_cancel = small_link(ui, &p, cancel_rect, "cancel", &tr("hub.cancel"), t.muted);
            }
            actions_left = inner.left();
            handled = true;
        } else if ready {
            let managed = self.installed[i].is_some();
            let open = tr("hub.open");
            let ow = pill_w(ui, &open).max(72.0);
            let open_rect = Rect::from_min_size(pos2(inner.right() - ow, foot_y - button_h / 2.0), vec2(ow, button_h));
            // the open pill fills in when the card is under the pointer; a click anywhere on the card opens the app
            let r = pill(ui, &p, t, open_rect, id("open"), &open, hovered, t.ink_strong, true);
            if r.clicked() {
                self.launch(i);
                handled = true;
            }
            actions_left = open_rect.left() - 8.0;
            if managed {
                if let Some(label) = self.fix_label(i).filter(|_| !busy_elsewhere) {
                    let uw = pill_w(ui, &label);
                    let upd = Rect::from_min_size(pos2(actions_left - uw, open_rect.top()), vec2(uw, button_h));
                    if pill(ui, &p, t, upd, id("update"), &label, false, t.engine_ink, true).clicked() {
                        act_install = true;
                        handled = true;
                    }
                    actions_left = upd.left() - 8.0;
                }
                // Remove sits in the top right corner and shows while the card is under the pointer.
                let corner_y = rect.top() + 18.0;
                if self.confirm_remove == Some(i) {
                    let no = Rect::from_min_size(pos2(inner.right() - 32.0, corner_y - 10.0), vec2(32.0, 20.0));
                    let yes = Rect::from_min_size(pos2(no.left() - 36.0, corner_y - 10.0), vec2(36.0, 20.0));
                    p.rect_filled(Rect::from_min_max(pos2(yes.left() - 74.0, yes.top() - 2.0), pos2(no.right() + 2.0, no.bottom() + 2.0)), CornerRadius::same(6), t.card);
                    p.text(pos2(yes.left() - 4.0, corner_y), Align2::RIGHT_CENTER, tr("hub.remove_q"), FontId::proportional(12.5), t.bad);
                    if small_link(ui, &p, yes, "yes", &tr("hub.yes"), t.bad) {
                        act_remove = true;
                    }
                    if small_link(ui, &p, no, "no", &tr("hub.no"), t.muted) {
                        self.confirm_remove = None;
                    }
                    handled = true;
                } else if !busy_elsewhere && lift > 0.5 {
                    let label = tr("hub.remove");
                    let w = ui.painter().layout_no_wrap(label.clone(), FontId::proportional(12.5), t.faint).size().x + 8.0;
                    let rm = Rect::from_min_size(pos2(inner.right() - w, corner_y - 10.0), vec2(w, 20.0));
                    if small_link(ui, &p, rm, "remove", &label, t.faint) {
                        self.confirm_remove = Some(i);
                        handled = true;
                    }
                }
            }
        } else if app.pack {
            let size = match &self.packs {
                Packs::Ready(l) => l.plans.get(app.id).map(|pl| trf("hub.install_size", &[("size", &mb(pl.download_bytes))])),
                _ => None,
            };
            let label = size.unwrap_or_else(|| if matches!(self.packs, Packs::Loading) { tr("hub.loading") } else { tr("hub.install") });
            let w = pill_w(ui, &label);
            let wide = Rect::from_min_size(pos2(inner.right() - w, foot_y - button_h / 2.0), vec2(w, button_h));
            let r = pill(ui, &p, t, wide, id("install"), &label, true, t.invert_fg, !busy_elsewhere);
            let r = match &self.packs {
                _ if busy_elsewhere => r.on_hover_text(tr("hub.busy")),
                Packs::Failed(e) => r.on_hover_text(trf("hub.list_failed", &[("err", e)])),
                _ => r,
            };
            if !busy_elsewhere && r.clicked() {
                act_install = true;
            }
            actions_left = wide.left() - 8.0;
            handled = true;
        } else {
            let label = tr("hub.coming_soon");
            let w = pill_w(ui, &label);
            let soon = Rect::from_min_size(pos2(inner.right() - w, foot_y - button_h / 2.0), vec2(w, button_h));
            pill(ui, &p, t, soon, id("soon"), &label, false, t.faint, false);
            actions_left = soon.left() - 8.0;
        }

        // The engine in violet mono, then its status.
        if actions_left > inner.left() + 40.0 {
            let (status, status_color) = self.status_text(t, i);
            let room = actions_left - inner.left();
            let engine = app.engine.to_lowercase();
            let eng = ui.painter().layout_no_wrap(engine.clone(), mono(12.0), t.engine_ink);
            let ew = eng.size().x.min(room);
            p.text(pos2(inner.left(), foot_y), Align2::LEFT_CENTER, fit(ui, &engine, mono(12.0), room), mono(12.0), t.engine_ink);
            if room - ew > 50.0 {
                let st = fit(ui, &format!("· {status}"), FontId::proportional(12.5), room - ew - 8.0);
                p.text(pos2(inner.left() + ew + 8.0, foot_y), Align2::LEFT_CENTER, st, FontId::proportional(12.5), status_color);
            }
        }

        if act_cancel && let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
        if act_remove {
            self.confirm_remove = None;
            self.start_remove(&ctx, i);
        } else if act_install {
            self.request_install(&ctx, i);
        } else if ready && !handled && resp.clicked() {
            self.launch(i);
        }
    }

    /// The repair or update the card offers next to OPEN, if any.
    fn fix_label(&self, i: usize) -> Option<String> {
        match (&self.health[i], &self.status[i]) {
            (Health::EngineMissing, _) => Some(tr("hub.fix_reinstall")),
            (Health::EngineUntested { .. }, _) => Some(tr("hub.fix_bundle")),
            (_, Some(Status::UpdateAvailable)) => Some(tr("hub.fix_update")),
            _ => None,
        }
    }

    /// The word and colour in the card's top-right corner.
    fn status_text(&self, t: &Tokens, i: usize) -> (String, Color32) {
        if let Some(job) = self.job.as_ref().filter(|j| j.app == i) {
            return (tr(if job.removing { "hub.st_removing" } else { "hub.st_installing" }), t.accent_ink);
        }
        if self.found[i].is_some() {
            return match (&self.health[i], &self.status[i]) {
                (Health::EngineMissing, _) => (tr("hub.st_engine_missing"), t.bad),
                (Health::EngineUntested { .. }, _) => (tr("hub.st_engine_untested"), t.accent_ink),
                (_, Some(Status::UpdateAvailable)) => (tr("hub.st_update"), t.accent_ink),
                _ => (tr("hub.st_ready"), t.good),
            };
        }
        if APPS[i].pack { (tr("hub.st_not_installed"), t.faint) } else { (tr("hub.st_planned"), t.faint) }
    }

    /// What a running install is doing, for the card's small print.
    fn job_line(&self, i: usize) -> Option<(String, bool)> {
        let job = self.job.as_ref().filter(|j| j.app == i)?;
        if job.removing {
            return Some((tr("hub.job_removing"), false));
        }
        let pct = if job.total > 0 { format!(" {}%", job.done * 100 / job.total) } else { String::new() };
        Some((
            if job.stage == "engine" {
                trf("hub.job_dl_engine", &[("engine", APPS[i].engine), ("pct", &pct)])
            } else {
                trf("hub.job_dl_app", &[("pct", &pct)])
            },
            false,
        ))
    }

    /// Small print about the engine: where it comes from now, or where it would come from. The bool marks a warning.
    fn source_line(&self, i: usize) -> Option<(String, bool)> {
        let app = &APPS[i];
        let engine = app.engine.to_owned();
        if let Some(have) = &self.installed[i] {
            return Some(match (&self.health[i], &have.engine) {
                (Health::EngineMissing, _) => (trf("hub.src_not_found", &[("engine", &engine)]), true),
                (Health::EngineUntested { found }, _) => (trf("hub.src_untested", &[("engine", &engine), ("found", &found.to_string())]), true),
                (_, Some((EngineSource::Bundled, v))) => (trf("hub.src_bundled_v", &[("engine", &engine), ("v", &v.to_string())]), false),
                (_, Some((EngineSource::System { .. }, v))) => (trf("hub.src_uses_v", &[("engine", &engine), ("v", &v.to_string())]), false),
                _ => return None,
            });
        }
        if self.found[i].is_some() {
            return Some((trf("hub.src_bundled", &[("engine", &engine)]), false)); // from the full download
        }
        let Packs::Ready(l) = &self.packs else { return None };
        let pack = l.manifest.packs.iter().find(|p| p.id == app.id)?;
        let plan = l.plans.get(&pack.id)?;
        Some(match (&plan.engine, &pack.engine) {
            (Some(EngineSource::System { version }), _) => (trf("hub.src_will_use", &[("engine", &engine), ("version", &version.to_string())]), false),
            (Some(EngineSource::Bundled), Some(spec)) => (trf("hub.src_includes", &[("engine", &engine), ("version", &spec.version.to_string())]), false),
            _ => return None,
        })
    }

    fn list(&mut self, ui: &mut egui::Ui, t: &Tokens, group: &[usize]) {
        for &i in group {
            let app = &APPS[i];
            let ready = self.found[i].is_some();
            let (slot, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H + 6.0), Sense::hover());
            let rect = Rect::from_min_size(slot.min + vec2(PAD, 0.0), vec2(slot.width() - 2.0 * PAD, ROW_H));
            let resp = ui.interact(rect, ui.scope_id().with(("row-hit", app.id)), if ready { Sense::click() } else { Sense::hover() });
            let hovered = resp.hovered();
            if ready && hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let lift = ui.ctx().animate_bool_with_time(ui.scope_id().with(("row", app.id)), hovered, 0.2);
            let color = rgb(app.color);
            let accent = if ready { color } else { t.muted.lerp_to_gamma(color, lift) };
            let p = ui.painter_at(rect.expand(1.0));
            p.rect(
                rect,
                CornerRadius::same(12),
                t.card.lerp_to_gamma(accent, 0.07 * lift),
                egui::Stroke::new(1.0, t.line.lerp_to_gamma(accent, 0.55 * lift)),
                StrokeKind::Inside,
            );
            if lift > 0.0 {
                let h = (rect.height() - 20.0) * lift;
                p.rect_filled(Rect::from_min_size(pos2(rect.left(), rect.center().y - h / 2.0), vec2(2.0, h)), CornerRadius::same(1), accent);
            }

            let tile = Rect::from_center_size(pos2(rect.left() + PAD + 20.0, rect.center().y), vec2(44.0, 44.0));
            let tint = if ready { Color32::WHITE } else { Color32::from_gray(150).lerp_to_gamma(Color32::WHITE, lift) };
            self.icons.paint(&p, i, tile, tint, t.line);
            let x = tile.right() + 14.0;
            p.text(pos2(x, rect.center().y - 9.0), Align2::LEFT_CENTER, app.name, strong(15.0), t.ink_strong);
            p.text(pos2(x, rect.center().y + 10.0), Align2::LEFT_CENTER, app.tagline(), FontId::proportional(12.5), t.muted);

            let (status, status_color) = self.status_text(t, i);
            let working = self.job.as_ref().is_some_and(|j| j.app == i);
            let busy_elsewhere = self.job.is_some() && !working;
            let button = Rect::from_center_size(pos2(rect.right() - 18.0 - 40.0, rect.center().y), vec2(80.0, 30.0));
            // INSTALL is wider than OPEN and the REMOVE / UPDATE links sit left of OPEN: the columns of text (status,
            // engine) end where the actions begin, so they never run under them.
            let links = ready && !working && self.installed[i].is_some() && !busy_elsewhere && rect.width() > 700.0;
            let links_w = if !links {
                0.0
            } else if self.confirm_remove == Some(i) {
                14.0 + 100.0 + 6.0 + 36.0 + 6.0
            } else {
                14.0 + 70.0 + 6.0 + self.fix_label(i).map_or(0.0, |_| 110.0 + 6.0)
            };
            let actions_left = if working {
                rect.right() - PAD - 230.0
            } else if !ready && app.pack {
                button.left() - 60.0
            } else {
                button.left() - links_w
            };
            let status_x = actions_left - 120.0;
            let engine_x = status_x - 100.0;
            let text_end = rect.left() + 440.0;
            if engine_x > text_end {
                p.text(pos2(engine_x, rect.center().y), Align2::LEFT_CENTER, app.engine.to_lowercase(), mono(12.0), t.engine_ink);
            }
            if status_x > text_end {
                p.text(pos2(status_x, rect.center().y), Align2::LEFT_CENTER, status, FontId::proportional(12.5), status_color);
            }
            let ctx = ui.ctx().clone();
            let (mut act_install, mut act_remove, mut handled) = (false, false, false);
            if working {
                let line = self.job_line(i).map(|(s, _)| s).unwrap_or_default();
                p.text(pos2(button.right(), rect.center().y), Align2::RIGHT_CENTER, line, mono(10.5), t.engine_ink);
                let r = ui.interact(button, ui.scope_id().with(("cancel-row", app.id)), Sense::click());
                if !self.job.as_ref().is_some_and(|j| j.removing) && r.clicked() && let Some(job) = &self.job {
                    job.cancel.store(true, Ordering::Relaxed);
                }
                handled = true;
            } else if ready {
                let row_over = ui.rect_contains_pointer(rect);
                let r = pill(ui, &p, t, button, ui.scope_id().with(("row-open", app.id)), &tr("hub.open"), row_over, t.ink_strong, true);
                if r.clicked() {
                    self.launch(i);
                    handled = true;
                }
                if links {
                    let fix = self.fix_label(i);
                    let mut x = button.left() - 14.0;
                    let mut link = |ui: &mut egui::Ui, salt: &str, label: &str, w: f32, color: Color32| -> bool {
                        let r = Rect::from_min_size(pos2(x - w, rect.center().y - 14.0), vec2(w, 28.0));
                        x -= w + 6.0;
                        let resp = ui.interact(r, ui.scope_id().with((salt, app.id)), Sense::click());
                        let c = if resp.hovered() { t.ink_strong } else { color };
                        if resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        p.text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(12.5), c);
                        resp.clicked()
                    };
                    if self.confirm_remove == Some(i) {
                        if link(ui, "row-yes", &tr("hub.yes_remove"), 100.0, t.bad) {
                            act_remove = true;
                        }
                        if link(ui, "row-no", &tr("hub.no"), 36.0, t.muted) {
                            self.confirm_remove = None;
                        }
                    } else {
                        if link(ui, "row-remove", &tr("hub.remove"), 70.0, t.faint) {
                            self.confirm_remove = Some(i);
                        }
                        if let Some(label) = fix
                            && link(ui, "row-update", &label, 110.0, t.accent_ink)
                        {
                            act_install = true;
                        }
                    }
                    handled = true;
                }
            } else if app.pack {
                let label = match &self.packs {
                    Packs::Ready(l) => l.plans.get(app.id).map(|pl| trf("hub.install_size", &[("size", &mb(pl.download_bytes))])),
                    _ => None,
                }
                .unwrap_or_else(|| tr("hub.install"));
                let wide = Rect::from_min_max(pos2(rect.right() - PAD - 160.0, button.top()), pos2(rect.right() - PAD, button.bottom()));
                let r = ui.interact(wide, ui.scope_id().with(("install-row", app.id)), if busy_elsewhere { Sense::hover() } else { Sense::click() });
                let _ = r;
                let r = pill(ui, &p, t, wide, ui.scope_id().with(("install-pill", app.id)), &label, true, t.invert_fg, !busy_elsewhere);
                act_install = !busy_elsewhere && r.clicked();
                handled = true;
            } else {
                pill(ui, &p, t, button, ui.scope_id().with(("row-soon", app.id)), &tr("hub.soon"), false, t.faint, false);
            }
            if act_remove {
                self.confirm_remove = None;
                self.start_remove(&ctx, i);
            } else if act_install {
                self.request_install(&ctx, i);
            } else if ready && !handled && resp.clicked() {
                self.launch(i);
            }
        }
    }
}

/// Reads a release's pack list (checked against its checksums) and what each pack would do on this PC.
/// Starts the hub at `exe` (the new one, same path as this one) and ends this process.
fn restart_into(exe: &std::path::Path) {
    let mut cmd = Command::new(exe);
    // this hub is still listening for later starts until it exits: the new one must not hand itself over to it
    cmd.env("VOIDFLARE_MULTI", "1");
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    if cmd.spawn().is_ok() {
        std::process::exit(0);
    }
}

fn load_packs(release: &update::Release) -> Result<Box<Loaded>, String> {
    let manifest = packs::fetch_manifest(release)?;
    let sums = update::sums_text(release)?;
    let mut system = HashMap::new();
    let mut plans = HashMap::new();
    for pack in &manifest.packs {
        let version = pack.engine.as_ref().map(|spec| system.entry(spec.name.clone()).or_insert_with(|| packs::system_engine_version(spec)).clone()).flatten();
        plans.insert(pack.id.clone(), packs::plan(release, pack, version.as_deref()));
    }
    Ok(Box::new(Loaded { release: release.clone(), sums, manifest, plans, system }))
}

/// "50 MB" for a download, in the card's small print.
fn mb(bytes: u64) -> String {
    format!("{} MB", (bytes as f64 / 1_048_576.0).round().max(1.0) as u64)
}

/// `text` cut with an ellipsis so it fits in `width` on one line.
fn fit(ui: &egui::Ui, text: &str, font: FontId, width: f32) -> String {
    let w = |s: &str| ui.painter().layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x;
    if w(text) <= width {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 1 && w(&format!("{}…", chars[..n].iter().collect::<String>())) > width {
        n -= 1;
    }
    format!("{}…", chars[..n].iter().collect::<String>().trim_end())
}

/// Text wrapped to `width` and cut with an ellipsis after `rows` lines.
fn clipped(text: &str, font: FontId, color: Color32, width: f32, rows: usize) -> LayoutJob {
    let mut job = LayoutJob::simple(text.to_owned(), font, color, width.max(1.0));
    job.wrap.max_rows = rows;
    job
}

fn main() -> eframe::Result {
    // The notes the DWG conversion reports come from its own app catalog.
    i18n::init(&[SHELL_CATALOG, include_str!("../../crates/app-dwg-to-pdf/assets/strings.tsv"), HUB_CATALOG]);
    // 0.5: a hub that starts as voidflare.exe renames itself SwapCraft.exe (and the command line next to it),
    // starts again under the new name and ends; the new process rewires the shortcut and the menus. Not when the
    // right-click menu started it: Explorer starts one process per file, and they would race for the rename.
    if !std::env::args().any(|a| a == "--run")
        && let Ok(exe) = std::env::current_exe()
        && let Ok(Some(new)) = update::rename_legacy(&exe)
    {
        if let Some(dir) = settings::data_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join(RENAMED_MARK), "voidflare.exe -> SwapCraft.exe\n");
        }
        let mut cmd = Command::new(&new);
        cmd.args(std::env::args_os().skip(1));
        if let Some(dir) = new.parent() {
            cmd.current_dir(dir);
        }
        if cmd.spawn().is_ok() {
            return Ok(());
        }
    }
    // Started by the right-click menu: a small progress window instead of the hub.
    if let Some(result) = menu::run_from_args() {
        return result;
    }
    // Files dragged onto the program, or sent to it with "Open with".
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).filter(|p| p.exists()).collect();
    // A hub is already open: it takes the files and comes to the front, and this start ends here.
    if std::env::var_os("VOIDFLARE_MULTI").is_none() && single::hand_over(&args) {
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1040.0, 820.0])
            .with_min_inner_size([640.0, 560.0])
            .with_title("SwapCraft")
            .with_drag_and_drop(true)
            .with_icon(vf_ui::shell::window_icon(include_bytes!("../assets/app_icon.rgba"))),
        wgpu_options: vf_ui::shell::wgpu_config(),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "voidflare",
        options,
        Box::new(|cc| {
            theme::install(&cc.egui_ctx);
            let mut hub = Hub::new(&cc.egui_ctx);
            let ctx = cc.egui_ctx.clone();
            hub.later_starts = single::listen(move || {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                ctx.request_repaint();
            });
            // Files dragged onto the exe, or sent to it, wait in the drop panel.
            if !args.is_empty() {
                hub.set_files(&args);
            }
            Ok(Box::new(hub))
        }),
    )
}
