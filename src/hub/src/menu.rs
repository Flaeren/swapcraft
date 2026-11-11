//! The File Explorer right-click menu on the hub's side: adding and removing it (from the settings), keeping its
//! recipe entries in step with the RECIPES tab, and the small window a menu click opens (`voidflare.exe --run`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use eframe::egui::{self, RichText, vec2};
use vf_core::job::{self, Event, Item, Task};
use vf_core::menu::{self, Action, Texts};
use vf_core::recipe::{self, Recipe};
use vf_core::routes;
use vf_ui::shell::{drain, finish};
use vf_ui::theme::{self, Tokens};
use vf_ui::widgets::{self, Kind};

use crate::catalog::{self, APPS};
use crate::{mono, tr, trf};

/// 0 unknown, 1 off, 2 on: whether the menu is added, read once and then kept up to date by install and remove.
static STATE: AtomicU8 = AtomicU8::new(0);

pub fn installed() -> bool {
    match STATE.load(Ordering::Relaxed) {
        0 => {
            let on = menu::installed();
            STATE.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
        s => s == 2,
    }
}

fn texts() -> Texts {
    Texts { title: tr("menu.title"), recipe: tr("menu.recipe"), open_in: tr("menu.open_in"), here: tr("menu.here") }
}

/// The name of an app that is installed (an "Open in" entry for one that is not would do nothing).
fn app_name(id: &str) -> Option<String> {
    APPS.iter().find(|a| a.id == id && catalog::locate(a).is_some()).map(|a| a.name.to_owned())
}

/// Adds the menu (or rewrites it) for this hub. Returns how many file types got it.
pub fn install() -> Result<usize, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let recipes = recipe::load().unwrap_or_default();
    let n = menu::install(&exe, &recipes, &texts(), &app_name)?;
    STATE.store(2, Ordering::Relaxed);
    Ok(n)
}

pub fn uninstall() -> Result<(), String> {
    menu::uninstall()?;
    STATE.store(1, Ordering::Relaxed);
    Ok(())
}

/// The menu was added by a hub without the drag-and-drop part (0.4.0), and this one has its DLL: add the drag-and-drop
/// part by writing the menu again, in the background. Nothing happens when the menu is not added.
pub fn upgrade_if_needed() {
    let Ok(exe) = std::env::current_exe() else { return };
    let has_dll = exe.with_file_name(menu::dragmenu::DLL_NAME).is_file();
    let has_file = menu::dragmenu::path().is_some_and(|p| p.is_file());
    // Also rewritten when the saved drag menu has an old title (the name changed from voidflare to SwapCraft in
    // 0.4.1), so the menu in File Explorer follows the program's name.
    let stale_title = has_file && menu::dragmenu::path().and_then(|p| std::fs::read_to_string(p).ok()).is_some_and(|text| !text.contains(&texts().here));
    if installed() && has_dll && (!has_file || stale_title) {
        refresh(&recipe::load().unwrap_or_default());
    }
}

/// After the recipes changed: rewrites the menu so its recipe entries match, in the background (it takes a moment).
pub fn refresh(recipes: &[Recipe]) {
    if !installed() {
        return;
    }
    let recipes = recipes.to_vec();
    std::thread::spawn(move || {
        if let Ok(exe) = std::env::current_exe() {
            let _ = menu::install(&exe, &recipes, &texts(), &app_name);
        }
    });
}

/// `voidflare.exe --run <action> <file>`: Some when this process was started by the menu. Gathers the files of the
/// same click (other processes hand theirs over and end here), then opens the progress window.
pub fn run_from_args() -> Option<eframe::Result> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `--add-menu` / `--remove-menu`: the same as the buttons in the settings, for scripts. The exit code says how it went.
    match args.first().map(String::as_str) {
        Some("--add-menu") => std::process::exit(if install().is_ok() { 0 } else { 1 }),
        Some("--remove-menu") => std::process::exit(if uninstall().is_ok() { 0 } else { 1 }),
        Some("--run") => {}
        // Nemo's action: `--choose <files...>`, all the selected files at once; the window asks what to make
        Some("--choose") => {
            let files: Vec<PathBuf> = args[1..].iter().map(PathBuf::from).filter(|p| p.is_file()).collect();
            if files.is_empty() {
                return Some(Ok(()));
            }
            return Some(run_window(Runner::choosing(files, None)));
        }
        // Nemo's "Convert here": `--here <folder>`, the files copied with Ctrl+C go there
        Some("--here") => {
            let Some(dir) = args.get(1).map(PathBuf::from).filter(|d| d.is_dir()) else { return Some(Ok(())) };
            let files = copied_files();
            return Some(run_window(Runner::choosing(files, Some(dir))));
        }
        _ => return None,
    }
    let Some(raw) = args.get(1) else { return Some(Ok(())) };
    // From the drag-and-drop menu: `--run <action> --to <folder> --list <file>`, all the files at once and the
    // results in the folder they were dropped on. From the right-click menu: `--run <action> <file>`, one process
    // per file, gathered here.
    let (files, to) = match args.get(2).map(String::as_str) {
        Some("--to") => {
            let (Some(to), Some("--list"), Some(list)) = (args.get(3), args.get(4).map(String::as_str), args.get(5)) else { return Some(Ok(())) };
            (read_list(&PathBuf::from(list)), Some(PathBuf::from(to)))
        }
        Some(file) => match menu::gather(raw, &PathBuf::from(file)) {
            Some(files) => (files, None),
            None => return Some(Ok(())),
        },
        None => return Some(Ok(())),
    };
    if files.is_empty() {
        return Some(Ok(()));
    }
    let action = Action::parse(raw);
    if let Some(Action::Open(app)) = &action {
        open_in(app, &files);
        return Some(Ok(()));
    }
    Some(run_window(Runner::start(action, files, to)))
}

/// The files copied (or cut) with Ctrl+C in the file manager: the clipboard's list of files, else its text when every
/// line is a file. Folders are left out (the conversions take files). Empty when there is none.
fn copied_files() -> Vec<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let Ok(mut clip) = arboard::Clipboard::new() else { return Vec::new() };
        let listed = clip.get().file_list().unwrap_or_default();
        let files: Vec<PathBuf> = if listed.is_empty() {
            let text = clip.get_text().unwrap_or_default();
            let lines: Vec<PathBuf> = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && *l != "copy" && *l != "cut")
                .map(|l| PathBuf::from(l.strip_prefix("file://").map_or_else(|| l.to_owned(), percent_decode)))
                .collect();
            if !lines.is_empty() && lines.iter().all(|p| p.is_absolute() && p.exists()) { lines } else { Vec::new() }
        } else {
            listed
        };
        files.into_iter().filter(|p| p.is_file()).collect()
    }
    #[cfg(not(target_os = "linux"))]
    Vec::new()
}

/// `%20` and the like in a `file://` address back to the characters (UTF-8).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && b[i + 1].is_ascii_hexdigit()
            && b[i + 2].is_ascii_hexdigit()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// "Open in": the app itself shows the files, no window of ours.
fn open_in(app: &str, files: &[PathBuf]) {
    if let Some(exe) = APPS.iter().find(|a| a.id == app).and_then(catalog::locate) {
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(files);
        if let Some(dir) = exe.parent() {
            cmd.current_dir(dir);
        }
        let _ = cmd.spawn();
    }
}

fn run_window(runner: Runner) -> eframe::Result {
    // a conversion opens straight as the card; the choice from Nemo first as a small normal window
    let card = runner.choosing.is_none();
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size(if card { CARD } else { [540.0, 340.0] })
        .with_min_inner_size(if card { CARD } else { [420.0, 260.0] })
        .with_decorations(!card);
    let viewport = if card { viewport.with_always_on_top() } else { viewport };
    // the choice has a title of its own, not the hub's, to tell the two apart in the window list
    let title = if card { "SwapCraft".to_owned() } else { tr("menu.title") };
    let options = eframe::NativeOptions {
        viewport: viewport
            .with_title(title)
            .with_icon(vf_ui::shell::window_icon(include_bytes!("../assets/app_icon.rgba"))),
        wgpu_options: vf_ui::shell::wgpu_config(),
        centered: !card,
        ..Default::default()
    };
    eframe::run_native(
        "voidflare-run",
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(runner))
        }),
    )
}

impl Runner {
    fn choose_ui(&mut self, ui: &mut egui::Ui, t: &Tokens, files: Vec<PathBuf>) {
        vf_ui::shell::fit_to_screen(ui.ctx());
        let entries = Self::choices(&files);
        let mut picked = None;
        let mut close = false;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg).inner_margin(18.0)).show(ui, |ui| {
            ui.label(RichText::new(&self.title).size(17.0).color(t.ink_strong));
            ui.add_space(4.0);
            let names: Vec<String> = files.iter().take(3).map(|f| f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect();
            let more = if files.len() > 3 { format!("  {}", trf("hub.more", &[("n", &(files.len() - 3).to_string())])) } else { String::new() };
            ui.label(RichText::new(format!("{}{more}", names.join("  ·  "))).font(mono(11.5)).color(t.faint));
            ui.add_space(12.0);
            if files.is_empty() {
                ui.label(RichText::new(tr("menu.here_empty")).color(t.bad));
                ui.add_space(12.0);
                if widgets::button(ui, &tr("hub.close")).clicked() {
                    close = true;
                }
            } else if entries.is_empty() {
                // each file may convert on its own: they only have no format in common
                let said = if files.len() > 1 { tr("menu.no_common") } else { tr("hub.nothing_convertible") };
                ui.label(RichText::new(said).color(t.bad));
                ui.add_space(12.0);
                if widgets::button(ui, &tr("hub.close")).clicked() {
                    close = true;
                }
            }
            egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    for e in &entries {
                        if e.separator {
                            ui.end_row();
                        }
                        if ui.add(egui::Button::new(RichText::new(&e.label).size(13.0)).min_size(vec2(0.0, 30.0))).clicked() {
                            picked = Some(e.action.clone());
                        }
                    }
                });
            });
        });
        if close {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        match picked {
            Some(Action::Open(app)) => {
                open_in(&app, &files);
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Some(action) => *self = Runner::start(Some(action), files, self.here.clone()),
            None => {}
        }
    }
}

/// The files of a drag-and-drop click: one path per line, written by the DLL in `%TEMP%\voidflare-menu`. The list
/// is deleted after reading, but only if it is one of ours (a path given by hand is never deleted).
fn read_list(list: &std::path::Path) -> Vec<PathBuf> {
    let files: Vec<PathBuf> = std::fs::read_to_string(list).unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()).map(PathBuf::from).collect();
    let ours = list.parent() == Some(std::env::temp_dir().join("voidflare-menu").as_path())
        && list.file_name().is_some_and(|n| n.to_string_lossy().starts_with("drop-"));
    if ours {
        let _ = std::fs::remove_file(list);
    }
    files
}

struct Runner {
    title: String,
    rx: Option<mpsc::Receiver<Event>>,
    log: Vec<(String, Kind)>,
    progress: (usize, usize),
    done: bool,
    last_dir: Option<PathBuf>,
    _cancel: Arc<AtomicBool>,
    /// A recipe: progress counts steps, not files.
    by_steps: bool,
    started: usize,
    /// From Nemo: the files waiting for a choice of format, recipe or app (nothing runs yet).
    choosing: Option<Vec<PathBuf>>,
    /// The format being made ("PDF"), for the sentence at the end; empty for a recipe.
    to_label: String,
    /// Files not converted.
    bad: usize,
    /// The log is open under the card.
    details: bool,
    /// When it ended: a card that went well closes by itself a little later.
    done_at: Option<std::time::Instant>,
    /// The window has become the card (no title bar, bottom right).
    placed: bool,
    icon: Option<egui::TextureHandle>,
    /// × was pressed while it ran: the engine is being stopped, and the window closes when it has.
    closing: bool,
    /// Nemo's "Convert here": the folder the results go to.
    here: Option<PathBuf>,
}

impl Runner {
    /// `to`: where the results go (the folder files were dropped on); None = next to each file, or the recipe's choice.
    fn start(action: Option<Action>, files: Vec<PathBuf>, to: Option<PathBuf>) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let n = files.len().to_string();
        let mut log = Vec::new();
        let by_steps = matches!(action, Some(Action::Recipe(_)));
        let mut to_label = String::new();
        let (title, total) = match action {
            Some(Action::Route(id)) => {
                let label = files.first().and_then(|f| routes::find_route(&routes::ext_of(f), &id)).map_or(id.to_uppercase(), |r| routes::shown_label(r.label));
                let items: Vec<Item> = files
                    .into_iter()
                    .map(|src| {
                        let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let id = id.clone();
                        let source = src.clone();
                        let to = to.clone();
                        let run: Task = Box::new(move |note| {
                            let ext = routes::ext_of(&source);
                            let route = routes::find_route(&ext, &id).ok_or_else(|| format!("a .{ext} file cannot become {id}"))?;
                            let out_dir = to.or_else(|| source.parent().map(PathBuf::from)).unwrap_or_default();
                            routes::run_route(route, &source, &out_dir, note)
                        });
                        Item { label: name, source: src, run }
                    })
                    .collect();
                let total = items.len();
                job::spawn_tasks(items, tx, cancel.clone());
                to_label = label.clone();
                let running = if total == 1 { trf("menu.toast_running_one", &[("to", &label)]) } else { trf("menu.toast_running", &[("n", &n), ("to", &label)]) };
                (running, total)
            }
            Some(Action::Recipe(name)) => match recipe::load().ok().and_then(|all| recipe::find(&all, &name).cloned()) {
                Some(mut r) => {
                    if let Some(dir) = to {
                        r.out = recipe::Out::Folder(dir);
                    }
                    let steps = r.steps.len();
                    recipe::spawn(r, files, tx, cancel.clone());
                    (trf("rec.running", &[("name", &name), ("n", &n)]), steps)
                }
                None => {
                    log.push((trf("menu.no_recipe", &[("name", &name)]), Kind::Bad));
                    (name, 0)
                }
            },
            _ => {
                log.push((tr("menu.bad_action"), Kind::Bad));
                (String::from("voidflare"), 0)
            }
        };
        let running = total > 0;
        Runner {
            title: if running { title } else { log.first().map(|l| l.0.clone()).unwrap_or(title) },
            rx: running.then_some(rx),
            log,
            progress: (0, total),
            done: !running,
            last_dir: None,
            _cancel: cancel,
            by_steps,
            started: 0,
            choosing: None,
            to_label,
            bad: 0,
            details: false,
            done_at: None,
            placed: false,
            icon: None,
            closing: false,
            here: None,
        }
    }

    /// `to`: the folder the results go to (Nemo's "Convert here"); None = beside each file.
    fn choosing(files: Vec<PathBuf>, to: Option<PathBuf>) -> Self {
        let mut r = Runner::start(None, Vec::new(), None);
        r.log.clear();
        r.done = false;
        r.title = tr(if to.is_some() { "menu.here" } else { "menu.title" });
        r.choosing = Some(files);
        r.here = to;
        r
    }

    /// What every one of `files` can become, the recipes that take them all, and the apps for them: the same
    /// entries as the Windows menu, kept to those that suit the whole selection.
    fn choices(files: &[PathBuf]) -> Vec<menu::Entry> {
        let recipes = recipe::load().unwrap_or_default();
        let mut exts: Vec<String> = files.iter().map(|f| routes::ext_of(f)).collect();
        exts.sort();
        exts.dedup();
        let all = |ext: &str| menu::entries(ext, &recipes, &texts(), &app_name);
        let mut entries = all(&exts[0]);
        for ext in &exts[1..] {
            let other = all(ext);
            entries.retain(|e| other.iter().any(|o| o.action == e.action));
        }
        entries
    }
}

/// The progress card's size, and its room when the details (the log) are open.
const CARD: [f32; 2] = [420.0, 132.0];
const CARD_DETAILS: [f32; 2] = [420.0, 340.0];
/// How long a card that went well stays before it closes by itself (while the pointer is not on it).
const LINGER: Duration = Duration::from_secs(4);

impl eframe::App for Runner {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let t = Tokens::get(ui.ctx());
        if let Some(files) = self.choosing.clone() {
            self.choose_ui(ui, &t, files);
            return;
        }
        let ctx = ui.ctx().clone();
        self.become_card(&ctx, frame);
        self.poll();
        if !self.done {
            ctx.request_repaint_after(Duration::from_millis(80));
        } else if self.closing {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let hovered = ctx.input(|i| i.pointer.hover_pos().is_some());
        if let Some(at) = self.done_at
            && !self.failed()
            && !self.details
        {
            if hovered {
                self.done_at = Some(std::time::Instant::now());
            } else if at.elapsed() >= LINGER {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        self.card_ui(ui, &t);
    }
}

impl Runner {
    /// Takes in the conversion's events; at the end, the sentence the card shows.
    fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        // A recipe reports a start per step and its results at the end: count the steps as progress.
        let before = self.log.len();
        let finished = drain(rx, &mut self.log, &mut self.progress);
        if self.by_steps {
            // a step's line appears when it starts; the bar shows the steps already finished
            let starts = self.log[before..].iter().filter(|(l, _)| l.starts_with('[')).count();
            self.started += starts;
            self.progress.0 = self.started.saturating_sub(1).min(self.progress.1);
        }
        let Some(all) = finished else { return };
        finish(&all, &mut self.log, false);
        self.last_dir = all.iter().rev().find_map(|o| o.result.as_ref().ok()).and_then(|p| if p.is_dir() { Some(p.clone()) } else { p.parent().map(PathBuf::from) });
        self.progress.0 = self.progress.1;
        self.done = true;
        self.rx = None;
        self.done_at = Some(std::time::Instant::now());
        let ok = all.iter().filter(|o| o.result.is_ok()).count();
        self.bad = all.len() - ok;
        let folder = self.last_dir.as_ref().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut line = if ok == 0 {
            String::new()
        } else if self.by_steps || self.to_label.is_empty() {
            trf("menu.toast_done_files", &[("n", &ok.to_string()), ("folder", &folder)])
        } else {
            trf("menu.toast_done", &[("n", &ok.to_string()), ("to", &self.to_label), ("folder", &folder)])
        };
        if self.bad > 0 {
            let failed = if self.bad == 1 { tr("menu.toast_failed_one") } else { trf("menu.toast_failed", &[("k", &self.bad.to_string())]) };
            line = if line.is_empty() { failed } else { format!("{line}, {failed}") };
        }
        self.title = line;
    }

    /// Something went wrong: a file not converted, or nothing could start (a recipe gone, an old menu entry).
    fn failed(&self) -> bool {
        self.bad > 0 || (self.progress.1 == 0 && !self.log.is_empty())
    }

    /// The first frames: no title bar, above the other windows, in the bottom right corner (above a taskbar or a
    /// panel), with rounded corners on Windows 11. Done once the screen's size is known.
    fn become_card(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if self.placed {
            return;
        }
        let Some(monitor) = ctx.input(|i| i.viewport().monitor_size) else { return };
        self.placed = true;
        let size = if self.details { CARD_DETAILS } else { CARD };
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(size[0], size[1])));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(monitor.x - size[0] - 24.0, monitor.y - size[1] - 72.0)));
        round_corners(frame);
    }

    fn card_ui(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let ctx = ui.ctx().clone();
        let icon = self.icon.get_or_insert_with(|| widgets::load_icon(&ctx, &half_icon())).clone();
        let card = egui::Frame::new().fill(t.card).stroke(egui::Stroke::new(1.0, t.line)).inner_margin(egui::Margin { left: 16, right: 12, top: 12, bottom: 12 });
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let resp = card.show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                // the card moves like a window: drag it anywhere but on its buttons
                let whole = ui.interact(ui.max_rect(), ui.scope_id().with("card-drag"), egui::Sense::drag());
                if whole.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                ui.horizontal(|ui| {
                    ui.add(egui::Image::new(&icon).fit_to_exact_size(vec2(22.0, 22.0)));
                    ui.add_space(4.0);
                    ui.label(RichText::new("SwapCraft").font(vf_ui::theme::strong(14.0)).color(t.ink_strong));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let close = ui.add(egui::Button::new(RichText::new("×").size(16.0).color(t.muted)).frame(false));
                        if close.on_hover_text(tr("hub.close")).clicked() {
                            if self.done {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            } else {
                                // closing at once would leave the engine working on with nobody waiting for it
                                self._cancel.store(true, Ordering::Relaxed);
                                self.closing = true;
                            }
                        }
                    });
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let color = if self.done && self.failed() { t.bad } else if self.done { t.good } else { t.muted };
                    let bar_w = 150.0;
                    let text_w = (ui.available_width() - bar_w - 16.0).max(80.0);
                    ui.allocate_ui_with_layout(vec2(text_w, 18.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(&self.title).size(12.5).color(color)).truncate());
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let total = self.progress.1.max(1) as f32;
                        pill_bar(ui, t, bar_w, self.progress.0 as f32 / total);
                    });
                });
                if self.done {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if let Some(dir) = self.last_dir.clone()
                            && widgets::primary(ui, t, &tr("hub.open_folder"), true).clicked()
                        {
                            vf_core::platform::open_folder(dir);
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if self.failed() {
                            let label = tr("menu.details");
                            if widgets::button(ui, &label).clicked() {
                                self.details = !self.details;
                                let size = if self.details { CARD_DETAILS } else { CARD };
                                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(size[0], size[1])));
                                if let Some(monitor) = ctx.input(|i| i.viewport().monitor_size) {
                                    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(monitor.x - size[0] - 24.0, monitor.y - size[1] - 72.0)));
                                }
                            }
                        }
                    });
                }
                if self.details {
                    ui.add_space(10.0);
                    egui::Frame::new().fill(t.sunken).stroke(t.hairline()).corner_radius(8).inner_margin(8.0).show(ui, |ui| {
                        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                            for (line, kind) in &self.log {
                                let c = match kind {
                                    Kind::Plain => t.ink,
                                    Kind::Good => t.good,
                                    Kind::Bad => t.bad,
                                };
                                ui.label(RichText::new(line).font(mono(11.0)).color(c));
                            }
                        });
                    });
                }
            });
            let _ = resp;
        });
    }
}

/// A thin pill: the track, and the filled part in the accent colour (as in the SwapCraft video).
fn pill_bar(ui: &mut egui::Ui, t: &Tokens, width: f32, fraction: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 5.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, egui::CornerRadius::same(3), t.sunken);
    let f = fraction.clamp(0.0, 1.0);
    if f > 0.0 {
        let fill = egui::Rect::from_min_size(rect.min, vec2((rect.width() * f).max(5.0), rect.height()));
        p.rect_filled(fill, egui::CornerRadius::same(3), t.accent);
    }
}

/// The 256x256 program icon halved to the 128x128 the icon widget takes.
fn half_icon() -> Vec<u8> {
    vf_ui::shell::window_icon_128(include_bytes!("../assets/app_icon.rgba"))
}

/// Windows 11 draws a window without a title bar with square corners unless asked; Windows 10 ignores the request.
fn round_corners(frame: &eframe::Frame) {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Dwm::{DWM_WINDOW_CORNER_PREFERENCE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
        if let Ok(handle) = frame.window_handle()
            && let RawWindowHandle::Win32(h) = handle.as_raw()
        {
            let pref: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND;
            // SAFETY: a live window of this process, and a value of the type the attribute takes
            unsafe {
                let _ = DwmSetWindowAttribute(HWND(h.hwnd.get() as *mut _), DWMWA_WINDOW_CORNER_PREFERENCE, std::ptr::from_ref(&pref).cast(), std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32);
            }
        }
    }
    #[cfg(not(windows))]
    let _ = frame;
}
