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
        // "Open in": the app itself shows the files, no window of ours.
        if let Some(exe) = APPS.iter().find(|a| a.id == app).and_then(catalog::locate) {
            let mut cmd = std::process::Command::new(&exe);
            cmd.args(&files);
            if let Some(dir) = exe.parent() {
                cmd.current_dir(dir);
            }
            let _ = cmd.spawn();
        }
        return Some(Ok(()));
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([540.0, 340.0])
            .with_min_inner_size([420.0, 260.0])
            .with_title("SwapCraft")
            .with_icon(egui::IconData { rgba: include_bytes!("../assets/app_icon.rgba").to_vec(), width: 256, height: 256 }),
        ..Default::default()
    };
    Some(eframe::run_native(
        "voidflare-run",
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(Runner::start(action, files, to)))
        }),
    ))
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
}

impl Runner {
    /// `to`: where the results go (the folder files were dropped on); None = next to each file, or the recipe's choice.
    fn start(action: Option<Action>, files: Vec<PathBuf>, to: Option<PathBuf>) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let n = files.len().to_string();
        let mut log = Vec::new();
        let by_steps = matches!(action, Some(Action::Recipe(_)));
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
                (trf("menu.converting", &[("n", &n), ("to", &label)]), total)
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
        Runner { title, rx: running.then_some(rx), log, progress: (0, total), done: !running, last_dir: None, _cancel: cancel, by_steps, started: 0 }
    }
}

impl eframe::App for Runner {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let t = Tokens::get(ui.ctx());
        if let Some(rx) = &self.rx {
            // A recipe reports a start per step and its results at the end: count the steps as progress.
            let before = self.log.len();
            let finished = drain(rx, &mut self.log, &mut self.progress);
            if self.by_steps {
                // a step's line appears when it starts; the bar shows the steps already finished
                let starts = self.log[before..].iter().filter(|(l, _)| l.starts_with('[')).count();
                self.started += starts;
                self.progress.0 = self.started.saturating_sub(1).min(self.progress.1);
            }
            if let Some(all) = finished {
                finish(&all, &mut self.log, false);
                self.last_dir = all.iter().rev().find_map(|o| o.result.as_ref().ok()).and_then(|p| if p.is_dir() { Some(p.clone()) } else { p.parent().map(PathBuf::from) });
                self.progress.0 = self.progress.1;
                self.done = true;
                self.rx = None;
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(80));
            }
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg).inner_margin(18.0)).show(ui, |ui| {
            ui.label(RichText::new(tr("menu.from_menu")).font(mono(11.0)).color(t.faint));
            ui.add_space(4.0);
            ui.label(RichText::new(&self.title).size(17.0).color(t.ink_strong));
            ui.add_space(10.0);
            if !self.done {
                widgets::progress(ui, self.progress.0, self.progress.1.max(1));
                ui.add_space(8.0);
            }
            let log_h = (ui.available_height() - 44.0).max(60.0);
            egui::Frame::new().fill(t.sunken).stroke(t.hairline()).inner_margin(8.0).show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(log_h).stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                    for (line, kind) in &self.log {
                        let c = match kind {
                            Kind::Plain => t.ink,
                            Kind::Good => t.good,
                            Kind::Bad => t.bad,
                        };
                        ui.label(RichText::new(line).font(mono(11.5)).color(c));
                    }
                });
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(10.0, 0.0);
                if let Some(dir) = self.last_dir.clone()
                    && widgets::primary(ui, &t, &tr("hub.open_folder"), true).clicked()
                {
                    vf_core::platform::open_folder(dir);
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if self.done && widgets::button(ui, &tr("hub.close")).clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }
}
