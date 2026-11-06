//! The RECIPES tab: the saved recipes on the left, the one chosen on the right with its steps in order. Every
//! change is saved at once to `%APPDATA%\voidflare\recipes.json` (the command line, the MCP server and the
//! right-click menu read the same file). Running a recipe uses the hub's log, like a conversion from the drop panel.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};

use eframe::egui::{self, Align, Align2, Layout, Rect, RichText, Sense, UiBuilder, pos2, vec2};
use vf_core::recipe::{self, Level, Out, Recipe, Step};
use vf_core::routes::{self, ROUTES};
use vf_ui::theme::Tokens;
use vf_ui::widgets::{self, Kind};

use crate::{FOOTER_H, Hub, mono, tr, trf};

const LIST_W: f32 = 280.0;
const ROW_H: f32 = 58.0;

#[derive(Default)]
pub struct RecipesTab {
    list: Vec<Recipe>,
    selected: usize,
    loaded: bool,
    /// Why the file could not be read or written; while it could not be read nothing is saved over it.
    error: Option<String>,
    read_only: bool,
    confirm_delete: bool,
}

impl RecipesTab {
    /// Reads the saved recipes the first time the tab is shown. On a PC with none yet, the examples are saved, so the
    /// tab, the command line and the right-click menu start from the same three.
    fn ensure_loaded(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        match recipe::load() {
            Ok(list) if list.is_empty() => {
                // the examples get their names in the user's language; after that they are the user's own
                self.list = recipe::examples();
                for (r, key) in self.list.iter_mut().zip(["rec.ex_drawings", "rec.ex_scans", "rec.ex_report"]) {
                    r.name = tr(key);
                }
                self.list[1].file_name = tr("rec.ex_scans_file");
                self.list[2].file_name = tr("rec.ex_report_file");
                self.error = recipe::save(&self.list).err();
            }
            Ok(list) => self.list = list,
            Err(e) => {
                self.error = Some(e);
                self.read_only = true;
            }
        }
    }

    /// The recipes whose chain takes `ext`, for the drop panel.
    pub fn accepting(&mut self, ext: &str) -> Vec<Recipe> {
        self.ensure_loaded();
        self.list.iter().filter(|r| r.accepts(ext)).cloned().collect()
    }

    /// Why the list cannot be saved as it is, if it cannot.
    fn problem(&self) -> Option<String> {
        if self.list.iter().any(|r| r.name.trim().is_empty()) {
            return Some(tr("rec.need_name"));
        }
        // The name goes into the right-click menu's command line between quotes.
        if self.list.iter().any(|r| r.name.contains('\u{22}')) {
            return Some(tr("rec.no_quotes"));
        }
        if self.list.iter().flat_map(|r| &r.steps).any(|s| matches!(s, Step::Extract(p) if p.trim().is_empty())) {
            return Some(tr("rec.need_pages"));
        }
        let mut names: Vec<String> = self.list.iter().map(|r| r.name.trim().to_lowercase()).collect();
        names.sort();
        let n = names.len();
        names.dedup();
        (names.len() != n).then(|| tr("rec.same_name"))
    }

    fn save(&mut self) {
        if self.read_only || self.problem().is_some() {
            return;
        }
        self.error = recipe::save(&self.list).err();
        crate::menu::refresh(&self.list);
    }
}

/// The targets a convert step can name: one entry per route id, with the label of the first route that has it.
fn convert_targets() -> Vec<(String, &'static str)> {
    let mut seen: Vec<(String, &'static str)> = Vec::new();
    for r in ROUTES {
        let id = routes::route_id(r);
        if !seen.iter().any(|(s, _)| *s == id) {
            seen.push((id, r.label));
        }
    }
    seen
}

fn step_name(kind: &str) -> String {
    tr(&format!("rec.step_{kind}"))
}

fn level_name(l: Level) -> String {
    tr(&format!("rec.level_{}", l.word()))
}

/// `DWG → PDF → MERGE → SHRINK`: the short line under a recipe's name.
fn outline(r: &Recipe) -> String {
    let mut parts: Vec<String> = Vec::new();
    match r.takes().as_slice() {
        [] => parts.push("?".into()),
        [one] => parts.push(one.to_uppercase()),
        [one, two] => parts.push(format!("{}/{}", one.to_uppercase(), two.to_uppercase())),
        many => parts.push(format!("{} {}", many.len(), tr("rec.types"))),
    }
    for s in &r.steps {
        parts.push(match s {
            Step::Convert(to) => to.to_uppercase(),
            other => step_name(other.kind()).to_uppercase(),
        });
    }
    parts.join(" → ")
}

impl Hub {
    /// Runs a recipe on files with the hub's log, from the tab or from the drop panel.
    pub(crate) fn start_recipe(&mut self, r: Recipe, files: Vec<PathBuf>) {
        self.log.clear();
        self.last_dir = None;
        self.log.push((trf("rec.running", &[("name", &r.name), ("n", &files.len().to_string())]), Kind::Plain));
        let (tx, rx) = mpsc::channel();
        self.cancel = Arc::new(AtomicBool::new(false));
        self.progress = (0, 0);
        recipe::spawn(r, files, tx, self.cancel.clone());
        self.running = true;
        self.rx = Some(rx);
    }

    pub(crate) fn recipes_tab(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        self.recipes.ensure_loaded();
        let before = self.recipes.list.clone();
        let height = (ui.available_height() - FOOTER_H).max(0.0);
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let list_rect = Rect::from_min_size(area.min, vec2(LIST_W.min(area.width() * 0.4), area.height()));
        let edit_rect = Rect::from_min_max(pos2(list_rect.right(), area.top()), area.max);
        {
            let p = ui.painter();
            p.rect_filled(list_rect, 0.0, t.bg.gamma_multiply(0.86));
            p.rect_filled(edit_rect, 0.0, t.bg.gamma_multiply(0.86));
            p.vline(list_rect.right() - 0.5, list_rect.y_range(), t.hairline());
        }
        let mut left = ui.new_child(UiBuilder::new().max_rect(list_rect).layout(Layout::top_down(Align::Min)));
        self.recipe_list(&mut left, t);
        let mut right = ui.new_child(UiBuilder::new().max_rect(edit_rect).layout(Layout::top_down(Align::Min)));
        egui::ScrollArea::vertical().id_salt("recipe-editor").auto_shrink([false, false]).show(&mut right, |ui| {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(22, 18)).show(ui, |ui| self.recipe_editor(ui, t));
        });
        if self.recipes.list != before {
            self.recipes.save();
        }
    }

    fn recipe_list(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
        ui.painter().text(pos2(head.left() + 16.0, head.bottom() - 12.0), Align2::LEFT_BOTTOM, trf("rec.yours", &[("n", &self.recipes.list.len().to_string())]), mono(11.0), t.faint);
        ui.painter().hline(head.x_range(), head.bottom() - 0.5, t.hairline());
        egui::ScrollArea::vertical().id_salt("recipe-list").auto_shrink([false, false]).max_height(ui.available_height()).show(ui, |ui| {
            let mut chosen = None;
            for (i, r) in self.recipes.list.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                let on = i == self.recipes.selected;
                let p = ui.painter();
                if on {
                    p.rect_filled(rect, 0.0, t.sunken);
                    p.rect_filled(Rect::from_min_size(rect.min, vec2(2.0, rect.height())), 0.0, t.accent);
                } else if resp.hovered() {
                    p.rect_filled(rect, 0.0, t.sunken.gamma_multiply(0.6));
                }
                let name = if r.name.trim().is_empty() { tr("rec.untitled") } else { r.name.clone() };
                let w = rect.width() - 32.0;
                p.text(pos2(rect.left() + 16.0, rect.top() + 19.0), Align2::LEFT_CENTER, crate::fit(ui, &name, egui::FontId::proportional(14.0), w), egui::FontId::proportional(14.0), if on { t.ink_strong } else { t.ink });
                p.text(pos2(rect.left() + 16.0, rect.top() + 39.0), Align2::LEFT_CENTER, crate::fit(ui, &outline(r), mono(10.5), w), mono(10.5), t.faint);
                p.hline(rect.x_range(), rect.bottom() - 0.5, t.hairline());
                if resp.clicked() {
                    chosen = Some(i);
                }
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
            if let Some(i) = chosen {
                self.recipes.selected = i;
                self.recipes.confirm_delete = false;
            }
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
            let color = if resp.hovered() { t.ink_strong } else { t.accent_ink };
            ui.painter().text(pos2(rect.left() + 16.0, rect.center().y), Align2::LEFT_CENTER, format!("+ {}", tr("rec.new")), mono(11.5), color);
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.clicked() && !self.recipes.read_only {
                let mut n = self.recipes.list.len() + 1;
                let mut name = trf("rec.new_name", &[("n", &n.to_string())]);
                while recipe::find(&self.recipes.list, &name).is_some() {
                    n += 1;
                    name = trf("rec.new_name", &[("n", &n.to_string())]);
                }
                self.recipes.list.push(Recipe::new(&name));
                self.recipes.selected = self.recipes.list.len() - 1;
            }
        });
    }

    fn recipe_editor(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        if let Some(e) = &self.recipes.error {
            ui.label(RichText::new(trf("rec.file_error", &[("err", e)])).font(mono(11.5)).color(t.bad));
            ui.add_space(10.0);
        }
        let n = self.recipes.list.len();
        if n == 0 {
            ui.add_space(30.0);
            ui.label(RichText::new(tr("rec.empty")).color(t.muted));
            return;
        }
        self.recipes.selected = self.recipes.selected.min(n - 1);
        let sel = self.recipes.selected;
        let enabled = !self.recipes.read_only && !self.running;
        let mut run_on: Option<Vec<PathBuf>> = None;
        let mut delete = false;
        let mut duplicate = false;

        // Below this width the step settings go on a line of their own and the two boxes stack.
        let wide = ui.available_width() >= 600.0;
        ui.add_enabled_ui(enabled, |ui| {
            let r = &mut self.recipes.list[sel];
            // Name, and what the recipe takes.
            let takes_line = |ui: &mut egui::Ui, r: &Recipe| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let takes = r.takes();
                let shown: Vec<String> = takes.iter().take(6).map(|e| format!(".{}", e.to_uppercase())).collect();
                let more = if takes.len() > 6 { format!(" +{}", takes.len() - 6) } else { String::new() };
                ui.label(RichText::new(format!("{}{more}", shown.join(" "))).font(mono(11.0)).color(if takes.is_empty() { t.bad } else { t.ink }));
                ui.label(RichText::new(tr("rec.takes")).font(mono(11.0)).color(t.faint));
            };
            if wide {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(tr("rec.recipe")).font(mono(11.0)).color(t.faint));
                        ui.add(egui::TextEdit::singleline(&mut r.name).font(egui::FontId::proportional(20.0)).desired_width((ui.available_width() - 200.0).max(160.0)).frame(egui::Frame::NONE));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Max), |ui| takes_line(ui, r));
                });
            } else {
                ui.label(RichText::new(tr("rec.recipe")).font(mono(11.0)).color(t.faint));
                ui.add(egui::TextEdit::singleline(&mut r.name).font(egui::FontId::proportional(20.0)).desired_width(ui.available_width()).frame(egui::Frame::NONE));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let takes = r.takes();
                    ui.label(RichText::new(tr("rec.takes")).font(mono(11.0)).color(t.faint));
                    let shown: Vec<String> = takes.iter().take(4).map(|e| format!(".{}", e.to_uppercase())).collect();
                    let more = if takes.len() > 4 { format!(" +{}", takes.len() - 4) } else { String::new() };
                    ui.label(RichText::new(format!("{}{more}", shown.join(" "))).font(mono(11.0)).color(if takes.is_empty() { t.bad } else { t.ink }));
                });
            }
            hairline(ui, t);
            ui.add_space(14.0);

            // The steps, one box each.
            let targets = convert_targets();
            let mut action: Option<(usize, i32)> = None; // (step, -1 up / +1 down / 0 delete)
            let count = r.steps.len();
            for (i, step) in r.steps.iter_mut().enumerate() {
                egui::Frame::new().fill(t.bg).stroke(t.hairline()).inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        ui.label(RichText::new(format!("{:02}", i + 1)).font(mono(18.0)).color(t.faint));
                        ui.add_sized(vec2(96.0, 20.0), egui::Label::new(RichText::new(step_name(step.kind()).to_uppercase()).font(mono(11.5)).color(t.accent_ink)));
                        if wide {
                            step_options(ui, t, step, &targets, i);
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            // (the window's fonts have no ✕: a plain X)
                            if small(ui, "X").on_hover_text(tr("rec.remove_step")).clicked() {
                                action = Some((i, 0));
                            }
                            if ui.add_enabled(i + 1 < count, egui::Button::new(RichText::new("↓").font(mono(12.0)))).on_hover_text(tr("rec.down")).clicked() {
                                action = Some((i, 1));
                            }
                            if ui.add_enabled(i > 0, egui::Button::new(RichText::new("↑").font(mono(12.0)))).on_hover_text(tr("rec.up")).clicked() {
                                action = Some((i, -1));
                            }
                        });
                    });
                    if !wide {
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            step_options(ui, t, step, &targets, i);
                        });
                    }
                });
                ui.add_space(8.0);
            }
            match action {
                Some((i, 0)) => {
                    r.steps.remove(i);
                }
                Some((i, -1)) => r.steps.swap(i, i - 1),
                Some((i, _)) => r.steps.swap(i, i + 1),
                None => {}
            }
            // Add a step.
            egui::Frame::new().stroke(egui::Stroke::new(1.0, t.line_strong)).inner_margin(egui::Margin::symmetric(14, 8)).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                    ui.label(RichText::new(format!("+ {}", tr("rec.add_step"))).font(mono(11.0)).color(t.muted));
                    for kind in Step::KINDS {
                        if widgets::button(ui, &step_name(kind)).clicked()
                            && let Some(s) = Step::new_of(kind)
                        {
                            r.steps.push(s);
                        }
                    }
                });
            });
            if r.takes().is_empty() {
                // The reason in detail (in English, from the core) on hover: which step cannot follow which.
                let why = ["pdf", "docx", "dwg", "png", "mp4", "wav", "epub"].iter().filter_map(|e| r.chain(e).err()).next().unwrap_or_default();
                ui.add_space(8.0);
                ui.label(RichText::new(tr("rec.takes_nothing")).font(mono(11.5)).color(t.bad)).on_hover_text(why);
            }

            // Where the results go, and the right-click menu.
            ui.add_space(16.0);
            let save_box = |ui: &mut egui::Ui, r: &mut Recipe| {
                egui::Frame::new().fill(t.bg).stroke(t.hairline()).inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(RichText::new(tr("rec.save_to")).font(mono(11.0)).color(t.faint));
                    let mut beside = r.out == Out::Beside;
                    if ui.radio_value(&mut beside, true, tr("rec.beside")).clicked() {
                        r.out = Out::Beside;
                    }
                    let folder_label = match &r.out {
                        Out::Folder(f) => f.display().to_string(),
                        Out::Beside => tr("rec.folder"),
                    };
                    if ui.radio(!beside, folder_label).clicked()
                        && let Some(f) = rfd::FileDialog::new().set_title(tr("rec.folder_title")).pick_folder()
                    {
                        r.out = Out::Folder(f);
                    }
                    ui.add_space(6.0);
                    ui.label(RichText::new(tr("rec.file_name")).font(mono(11.0)).color(t.faint));
                    ui.add(egui::TextEdit::singleline(&mut r.file_name).hint_text("{name}").font(mono(12.0)).desired_width(f32::INFINITY))
                        .on_hover_text(tr("rec.file_name_hint"));
                });
            };
            let run_box = |ui: &mut egui::Ui, r: &mut Recipe| {
                egui::Frame::new().fill(t.bg).stroke(t.hairline()).inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(RichText::new(tr("rec.run_from")).font(mono(11.0)).color(t.faint));
                    ui.checkbox(&mut r.in_menu, tr("rec.in_menu")).on_hover_text(tr("rec.in_menu_hint"));
                    if !crate::menu::installed() {
                        ui.label(RichText::new(tr("rec.menu_off")).font(mono(10.5)).color(t.faint));
                    }
                });
            };
            if wide {
                ui.columns(2, |cols| {
                    save_box(&mut cols[0], r);
                    run_box(&mut cols[1], r);
                });
            } else {
                save_box(ui, r);
                ui.add_space(8.0);
                run_box(ui, r);
            }

            // Actions.
            ui.add_space(16.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(10.0, 8.0);
                let takes = r.takes();
                if widgets::primary(ui, t, &tr("rec.run"), !takes.is_empty()).clicked()
                    && let Some(files) = rfd::FileDialog::new().set_title(tr("rec.run_title")).add_filter(r.name.clone(), &takes).pick_files()
                {
                    run_on = Some(files);
                }
                if widgets::button(ui, &tr("rec.duplicate")).clicked() {
                    duplicate = true;
                }
                if self.recipes.confirm_delete {
                    let b = egui::Button::new(RichText::new(tr("rec.delete_sure").to_uppercase()).font(mono(11.5)).color(t.bad));
                    if ui.add(b).clicked() {
                        delete = true;
                    }
                } else if widgets::button(ui, &tr("rec.delete")).clicked() {
                    self.recipes.confirm_delete = true;
                }
            });
            ui.add_space(8.0);
            let cli = format!("voidflare-cli recipe \"{}\" <files>", self.recipes.list[sel].name);
            ui.label(RichText::new(trf("rec.cli", &[("cmd", &cli)])).font(mono(10.5)).color(t.faint));
        });

        if let Some(p) = self.recipes.problem() {
            ui.add_space(8.0);
            ui.label(RichText::new(p).font(mono(11.5)).color(t.bad));
        }
        if !self.log.is_empty() {
            ui.add_space(14.0);
            if self.running {
                widgets::progress(ui, 0, 0);
                ui.add_space(6.0);
            }
            widgets::log_view(ui, t, &self.log);
            if let Some(dir) = self.last_dir.clone()
                && widgets::button(ui, &tr("hub.open_folder")).clicked()
            {
                let _ = std::process::Command::new("explorer").arg(dir).spawn();
            }
        }

        if duplicate {
            let mut copy = self.recipes.list[sel].clone();
            let mut n = 2;
            while recipe::find(&self.recipes.list, &format!("{} ({n})", copy.name)).is_some() {
                n += 1;
            }
            copy.name = format!("{} ({n})", copy.name);
            self.recipes.list.insert(sel + 1, copy);
            self.recipes.selected = sel + 1;
        }
        if delete {
            self.recipes.list.remove(sel);
            self.recipes.selected = sel.saturating_sub(1);
            self.recipes.confirm_delete = false;
        }
        if let Some(files) = run_on {
            let r = self.recipes.list[sel].clone();
            self.start_recipe(r, files);
        }
    }
}

fn hairline(ui: &mut egui::Ui, t: &Tokens) {
    ui.add_space(10.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, t.hairline());
}

fn small(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).font(mono(12.0))))
}

/// A step's one-line explanation, cut with an ellipsis (full text on hover) so it never runs under the step's buttons.
fn hint(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    let w = (ui.available_width() - 110.0).max(80.0);
    ui.allocate_ui(vec2(w, 20.0), |ui| {
        ui.add(egui::Label::new(RichText::new(text).size(13.0).color(t.muted)).truncate());
    });
}

/// The settings of one step, inline in its box.
fn step_options(ui: &mut egui::Ui, t: &Tokens, step: &mut Step, targets: &[(String, &'static str)], i: usize) {
    match step {
        Step::Convert(to) => {
            let shown = targets.iter().find(|(id, _)| id == to).map_or(to.to_uppercase(), |(_, l)| (*l).to_owned());
            egui::ComboBox::from_id_salt(("convert-to", i)).selected_text(RichText::new(shown).font(mono(12.0))).width(220f32.min(ui.available_width() - 120.0).max(120.0)).show_ui(ui, |ui| {
                for (id, label) in targets {
                    ui.selectable_value(to, id.clone(), format!("{label}  ·  {id}"));
                }
            });
        }
        Step::Merge => {
            hint(ui, t, &tr("rec.merge_hint"));
        }
        Step::Shrink(level) => {
            egui::ComboBox::from_id_salt(("shrink", i)).selected_text(RichText::new(level_name(*level)).font(mono(12.0))).show_ui(ui, |ui| {
                for l in Level::ALL {
                    ui.selectable_value(level, l, level_name(l));
                }
            });
        }
        Step::Ocr => {
            hint(ui, t, &tr("rec.ocr_hint"));
        }
        Step::Split(n) => {
            ui.label(RichText::new(tr("rec.split_every")).size(13.0).color(t.muted));
            ui.add(egui::DragValue::new(n).range(1..=10_000));
            ui.label(RichText::new(tr("rec.pages")).size(13.0).color(t.muted));
        }
        Step::Extract(pages) => {
            ui.label(RichText::new(tr("rec.pages")).size(13.0).color(t.muted));
            ui.add(egui::TextEdit::singleline(pages).hint_text("1,3,5-8").font(mono(12.0)).desired_width(140.0));
        }
    }
}
