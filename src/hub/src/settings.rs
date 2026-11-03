//! The hub's settings window (the gear, top right) and the welcome dialog shown the first time it runs.
//!
//! Language: English is the base; the choice is saved in `%APPDATA%\voidflare\language.txt`, which every voidflare
//! app reads when it starts. (The hub's own window is English for now.)
//! Shortcuts: a Start menu entry can be made here. Windows does not let a program pin itself to the taskbar, so the
//! window says how to do it by hand from the Start menu entry.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, FontId, Id, Modal, Pos2, Rect, RichText, Sense, Stroke, vec2};
use vf_core::i18n::{self, Lang};
use vf_core::shortcuts::{self, State};
use vf_core::update;
use vf_ui::theme::{Appearance, Tokens};

fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// `%APPDATA%\voidflare`, where the hub keeps its few settings.
pub fn data_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("voidflare"))
}

const WELCOME_MARK: &str = "welcome.txt";

/// Has the welcome dialog already been answered (either way) in `dir`?
pub fn welcome_done_in(dir: &Path) -> bool {
    dir.join(WELCOME_MARK).is_file()
}

pub fn mark_welcome_done_in(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(WELCOME_MARK), "done\n");
}

pub struct Settings {
    /// The settings window is open.
    pub open: bool,
    /// The welcome dialog is showing.
    welcome: bool,
    lang: Lang,
    appearance: Appearance,
    shortcut: State,
    /// What the last action said, and whether it failed.
    msg: Option<(String, bool)>,
    /// When to look again at a shortcut that was just added.
    recheck_at: Option<std::time::Instant>,
}

impl Settings {
    pub fn new() -> Self {
        let welcome = data_dir().is_some_and(|d| !welcome_done_in(&d));
        Self { open: false, welcome, lang: i18n::remembered().unwrap_or(Lang::En), appearance: Appearance::load(), shortcut: State::Missing, msg: None, recheck_at: None }
    }

    pub fn show_welcome(&self) -> bool {
        self.welcome
    }

    pub fn open(&mut self) {
        self.shortcut = shortcuts::state();
        self.msg = None;
        self.open = true;
    }

    fn add_shortcut(&mut self) {
        self.msg = Some(match shortcuts::create() {
            Ok(()) => {
                // security software or a policy can remove a new shortcut a moment later: look again in two seconds
                self.recheck_at = Some(std::time::Instant::now() + std::time::Duration::from_secs(2));
                let place = shortcuts::start_menu_dir().map(|d| d.display().to_string()).unwrap_or_default();
                (format!("Added to the app list of your Start menu ({place}). To pin it there: open Start > All apps, right-click voidflare and choose Pin to Start."), false)
            }
            Err(e) => (format!("Could not add it: {e}"), true),
        });
        self.shortcut = shortcuts::state();
    }

    /// The second look after adding: if the shortcut is gone, say so instead of leaving "Added" on screen.
    fn recheck(&mut self) {
        self.recheck_at = None;
        self.shortcut = shortcuts::state();
        if self.shortcut == State::Missing {
            self.msg = Some(("It was created, but it is gone again. Your security software or a policy on this PC removes new Start menu shortcuts. You can pin voidflare.exe to the taskbar by hand instead.".to_owned(), true));
        }
    }

    fn remove_shortcut(&mut self) {
        self.msg = Some(match shortcuts::remove() {
            Ok(()) => ("Removed from the Start menu.".to_owned(), false),
            Err(e) => (e, true),
        });
        self.shortcut = shortcuts::state();
    }

    /// Draws whichever of the two windows is open. `auto_on` is the hub's update-check switch.
    pub fn ui(&mut self, ctx: &egui::Context, t: &Tokens, auto_on: &mut bool) {
        if let Some(at) = self.recheck_at {
            if std::time::Instant::now() >= at {
                self.recheck();
                // the dialog is gone by now: a shortcut that vanished is shown in the settings window
                if self.msg.as_ref().is_some_and(|(_, bad)| *bad) {
                    self.open = true;
                }
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            }
        }
        if self.welcome {
            self.welcome_ui(ctx, t);
        } else if self.open {
            self.settings_ui(ctx, t, auto_on);
        }
    }

    fn welcome_ui(&mut self, ctx: &egui::Context, t: &Tokens) {
        let mut answer: Option<bool> = None;
        Modal::new(Id::unique("welcome")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.add_space(4.0);
            ui.label(RichText::new("Welcome to voidflare").size(18.0).color(t.ink_strong));
            ui.add_space(8.0);
            ui.label(RichText::new("Want voidflare in your Start menu? It is added to the list of all apps. To pin it to the Start menu, right-click it there and choose Pin to Start; for the taskbar, choose Pin to taskbar.").color(t.muted));
            ui.add_space(6.0);
            ui.label(RichText::new("Windows does not let a program pin itself, so that last step is yours. You can change your mind any time in the settings (the gear).").size(12.0).color(t.faint));
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if primary(ui, t, "ADD TO START MENU").clicked() {
                    answer = Some(true);
                }
                if ui.add(egui::Button::new(RichText::new("NOT NOW").font(mono(11.0)))).clicked() {
                    answer = Some(false);
                }
            });
        });
        if let Some(add) = answer {
            if add {
                self.add_shortcut();
            }
            if let Some(dir) = data_dir() {
                mark_welcome_done_in(&dir);
            }
            self.welcome = false;
            // after "add", show the result in the settings window so a failure is not silent
            if add && self.msg.as_ref().is_some_and(|(_, bad)| *bad) {
                self.open();
            }
        }
    }

    fn settings_ui(&mut self, ctx: &egui::Context, t: &Tokens, auto_on: &mut bool) {
        let resp = Modal::new(Id::unique("settings")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.add_space(4.0);
            ui.label(RichText::new("Settings").size(18.0).color(t.ink_strong));

            section(ui, t, "LANGUAGE");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("language").selected_text(self.lang.name()).show_ui(ui, |ui| {
                    for l in Lang::ALL {
                        if ui.selectable_value(&mut self.lang, l, l.name()).clicked() {
                            i18n::set_language(l, true);
                        }
                    }
                });
            });
            ui.label(RichText::new("English is the default. The choice applies to the converter apps the next time they start; this window stays in English for now.").size(12.0).color(t.faint));

            section(ui, t, "APPEARANCE");
            egui::ComboBox::from_id_salt("appearance").selected_text(self.appearance.name()).show_ui(ui, |ui| {
                for a in Appearance::ALL {
                    if ui.selectable_value(&mut self.appearance, a, a.name()).clicked() {
                        a.save();
                        ctx.set_theme(a.preference());
                    }
                }
            });
            ui.label(RichText::new("The hub changes at once; the converter apps use the same choice the next time they start.").size(12.0).color(t.faint));

            section(ui, t, "UPDATES");
            if ui.checkbox(auto_on, "Check for a newer version when voidflare starts").changed() {
                update::set_auto(*auto_on);
            }
            ui.label(RichText::new("Sends nothing about you and downloads nothing by itself.").size(12.0).color(t.faint));

            section(ui, t, "SHORTCUTS");
            let (status, color) = match &self.shortcut {
                State::Ok => ("In the Start menu.".to_owned(), t.good),
                State::Missing => ("Not in the Start menu.".to_owned(), t.muted),
                State::Stale(p) if p.as_os_str().is_empty() => ("The Start menu entry is unreadable.".to_owned(), t.bad),
                State::Stale(p) => (format!("The Start menu entry points to {}, not here.", p.display()), t.bad),
            };
            ui.label(RichText::new(status).color(color));
            ui.horizontal(|ui| match self.shortcut.clone() {
                State::Ok => {
                    if ui.button(RichText::new("REMOVE FROM START MENU").font(mono(11.0))).clicked() {
                        self.remove_shortcut();
                    }
                    if ui.button(RichText::new("SHOW IT").font(mono(11.0))).on_hover_text("Opens the folder with the shortcut selected").clicked()
                        && let Some(dir) = shortcuts::start_menu_dir()
                    {
                        let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{}", shortcuts::link_path(&dir).display())).spawn();
                    }
                }
                State::Missing => {
                    if primary(ui, t, "ADD TO START MENU").clicked() {
                        self.add_shortcut();
                    }
                }
                State::Stale(_) => {
                    if primary(ui, t, "POINT IT HERE").clicked() {
                        self.add_shortcut();
                    }
                    if ui.button(RichText::new("REMOVE").font(mono(11.0))).clicked() {
                        self.remove_shortcut();
                    }
                }
            });
            if let Some((msg, bad)) = &self.msg {
                ui.label(RichText::new(msg).size(12.0).color(if *bad { t.bad } else { t.good }));
            }
            ui.label(RichText::new("Pinning: Windows does not let a program pin itself. Open Start > All apps, right-click voidflare and choose Pin to Start; or open it, right-click its icon in the taskbar and choose Pin to taskbar.").size(12.0).color(t.faint));

            section(ui, t, "YOUR DATA");
            if let Some(dir) = data_dir() {
                ui.label(RichText::new(dir.display().to_string()).font(mono(11.0)).color(t.muted));
                if ui.button(RichText::new("OPEN THAT FOLDER").font(mono(11.0))).clicked() {
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::process::Command::new("explorer.exe").arg(&dir).spawn();
                }
            }
            ui.label(RichText::new("Only your language and update choices live there. Apps are removed from their card in the hub (REMOVE).").size(12.0).color(t.faint));

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("voidflare v{}", env!("CARGO_PKG_VERSION"))).font(mono(11.0)).color(t.faint));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(RichText::new("CLOSE").font(mono(11.0))).clicked() {
                        ui.close();
                    }
                });
            });
        });
        if resp.should_close() {
            self.open = false;
        }
    }
}

fn section(ui: &mut egui::Ui, t: &Tokens, label: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(label).font(mono(11.0)).color(t.faint));
    ui.add_space(4.0);
}

fn primary(ui: &mut egui::Ui, t: &Tokens, label: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(label).font(mono(11.0)).color(t.invert_fg)).fill(t.invert_bg))
}

/// A gear drawn with lines (the window's fonts have no gear glyph): a ring and eight teeth.
pub fn paint_gear(p: &egui::Painter, c: Pos2, r: f32, color: Color32) {
    p.circle_stroke(c, r * 0.52, Stroke::new(1.6, color));
    p.circle_stroke(c, r * 0.18, Stroke::new(1.2, color));
    for k in 0..8 {
        let a = k as f32 * std::f32::consts::TAU / 8.0;
        let (s, co) = a.sin_cos();
        p.line_segment([c + vec2(co, s) * (r * 0.66), c + vec2(co, s) * r], Stroke::new(2.6, color));
    }
}

/// The gear button at `rect`; true when clicked.
pub fn gear_button(ui: &mut egui::Ui, t: &Tokens, rect: Rect) -> bool {
    let resp = ui.interact(rect, ui.scope_id().with("gear"), Sense::click());
    let color = if resp.hovered() { t.ink_strong } else { t.faint };
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        ui.painter().rect_filled(rect, 6.0, t.sunken);
    }
    paint_gear(ui.painter(), rect.center(), 9.0, color);
    resp.on_hover_text("Settings").clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_welcome_dialog_is_asked_once() {
        let dir = std::env::temp_dir().join(format!("vf-welcome-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!welcome_done_in(&dir));
        mark_welcome_done_in(&dir);
        assert!(welcome_done_in(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
