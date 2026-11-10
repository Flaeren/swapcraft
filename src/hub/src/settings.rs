//! The hub's settings window (the gear, top right) and the welcome dialog shown the first time it runs.
//!
//! Language: English is the base; the choice is saved in `%APPDATA%\voidflare\language.txt`, which every voidflare
//! app reads when it starts. (The hub's own window is English for now.)
//! Shortcuts: a Start menu entry can be made here. Windows does not let a program pin itself to the taskbar, so the
//! window says how to do it by hand from the Start menu entry.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, FontId, Id, Modal, Pos2, Rect, RichText, Sense, Stroke, vec2};
use vf_core::i18n::{self, Lang};
use vf_core::shortcuts::{self, Place, State};
use vf_core::update;
use vf_ui::theme::{Appearance, Tokens};

/// `%APPDATA%\voidflare` (Linux: `~/.config/voidflare`), where the hub keeps its few settings.
pub fn data_dir() -> Option<PathBuf> {
    vf_core::platform::data_dir()
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
    /// The desktop shortcut, read when the window opens.
    desktop: State,
    /// What the last action said, and whether it failed.
    msg: Option<(String, bool)>,
    /// When to look again at a shortcut that was just added.
    recheck_at: Option<std::time::Instant>,
    /// What adding or removing the right-click menu said.
    menu_msg: Option<(String, bool)>,
}

impl Settings {
    pub fn new() -> Self {
        let welcome = data_dir().is_some_and(|d| !welcome_done_in(&d));
        Self { open: false, welcome, lang: i18n::remembered().unwrap_or(Lang::En), appearance: Appearance::load(), shortcut: State::Missing, desktop: State::Missing, msg: None, recheck_at: None, menu_msg: None }
    }

    pub fn show_welcome(&self) -> bool {
        self.welcome
    }

    pub fn open(&mut self) {
        self.shortcut = shortcuts::state();
        self.desktop = shortcuts::state_at(Place::Desktop);
        self.msg = None;
        self.open = true;
    }

    fn add_shortcut(&mut self) {
        self.msg = Some(match shortcuts::create() {
            Ok(()) => {
                // security software or a policy can remove a new shortcut a moment later: look again in two seconds
                self.recheck_at = Some(std::time::Instant::now() + std::time::Duration::from_secs(2));
                let place = shortcuts::start_menu_dir().map(|d| d.display().to_string()).unwrap_or_default();
                (i18n::tf("hub.sc_added", &[("place", &place)]), false)
            }
            Err(e) => (i18n::tf("hub.sc_add_failed", &[("err", &e.to_string())]), true),
        });
        self.shortcut = shortcuts::state();
    }

    /// The second look after adding: if the shortcut is gone, say so instead of leaving "Added" on screen.
    fn recheck(&mut self) {
        self.recheck_at = None;
        self.shortcut = shortcuts::state();
        if self.shortcut == State::Missing {
            self.msg = Some((i18n::t("hub.sc_vanished"), true));
        }
    }

    fn add_desktop(&mut self) {
        self.msg = Some(match shortcuts::create_at(Place::Desktop) {
            Ok(()) => (i18n::t("hub.dt_added"), false),
            Err(e) => (i18n::tf("hub.sc_add_failed", &[("err", &e.to_string())]), true),
        });
        self.desktop = shortcuts::state_at(Place::Desktop);
    }

    fn remove_desktop(&mut self) {
        self.msg = Some(match shortcuts::remove_at(Place::Desktop) {
            Ok(()) => (i18n::t("hub.dt_removed"), false),
            Err(e) => (e, true),
        });
        self.desktop = shortcuts::state_at(Place::Desktop);
    }

    fn remove_shortcut(&mut self) {
        self.msg = Some(match shortcuts::remove() {
            Ok(()) => (i18n::t("hub.sc_removed"), false),
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
            ui.label(RichText::new(i18n::t("hub.welcome_title")).font(vf_ui::theme::strong(18.0)).color(t.ink_strong));
            ui.add_space(8.0);
            ui.label(RichText::new(i18n::t("hub.welcome_text")).color(t.muted));
            ui.add_space(6.0);
            ui.label(RichText::new(i18n::t("hub.welcome_note")).size(12.0).color(t.faint));
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if primary(ui, t, &i18n::t("hub.btn_add_start")).clicked() {
                    answer = Some(true);
                }
                if ui.add(egui::Button::new(RichText::new(i18n::t("hub.btn_not_now")).font(FontId::proportional(12.5)))).clicked() {
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
            ui.label(RichText::new(i18n::t("hub.set_title")).font(vf_ui::theme::strong(18.0)).color(t.ink_strong));
            // Taller than a small window: the sections scroll, the title and the Close row stay in view.
            let max_h = (ctx.content_rect().height() - 150.0).max(200.0);
            egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {

                section(ui, t, &i18n::t("hub.sec_language"));
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("language").selected_text(self.lang.name()).show_ui(ui, |ui| {
                        for l in Lang::ALL {
                            if ui.selectable_value(&mut self.lang, l, l.name()).clicked() {
                                i18n::set_language(l, true);
                            }
                        }
                    });
                });
                ui.label(RichText::new(i18n::t("hub.language_note")).size(12.0).color(t.faint));

                section(ui, t, &i18n::t("hub.sec_appearance"));
                let appearance_name = |a: Appearance| {
                    i18n::t(match a {
                        Appearance::Black => "hub.appearance_black",
                        Appearance::System => "hub.appearance_system",
                        Appearance::Dark => "hub.appearance_dark",
                        Appearance::Light => "hub.appearance_light",
                    })
                };
                egui::ComboBox::from_id_salt("appearance").selected_text(appearance_name(self.appearance)).show_ui(ui, |ui| {
                    for a in Appearance::ALL {
                        if ui.selectable_value(&mut self.appearance, a, appearance_name(a)).clicked() {
                            a.save();
                            a.apply(ctx);
                        }
                    }
                });
                ui.label(RichText::new(i18n::t("hub.appearance_note")).size(12.0).color(t.faint));

                section(ui, t, &i18n::t("hub.sec_updates"));
                if ui.checkbox(auto_on, i18n::t("hub.check_at_start")).changed() {
                    update::set_auto(*auto_on);
                }
                ui.label(RichText::new(i18n::t("hub.updates_note")).size(12.0).color(t.faint));

                section(ui, t, &i18n::t("hub.sec_shortcuts"));
                let (status, color) = match &self.shortcut {
                    State::Ok => (i18n::t("hub.sc_ok"), t.good),
                    State::Missing => (i18n::t("hub.sc_missing"), t.muted),
                    State::Stale(p) if p.as_os_str().is_empty() => (i18n::t("hub.sc_unreadable"), t.bad),
                    State::Stale(p) => (i18n::tf("hub.sc_stale", &[("path", &p.display().to_string())]), t.bad),
                };
                ui.label(RichText::new(status).color(color));
                ui.horizontal(|ui| match self.shortcut.clone() {
                    State::Ok => {
                        if ui.button(RichText::new(i18n::t("hub.btn_remove_start")).font(FontId::proportional(12.5))).clicked() {
                            self.remove_shortcut();
                        }
                        if ui.button(RichText::new(i18n::t("hub.btn_show")).font(FontId::proportional(12.5))).on_hover_text(i18n::t("hub.btn_show_hint")).clicked()
                            && let Some(dir) = shortcuts::start_menu_dir()
                        {
                            vf_core::platform::reveal(&shortcuts::link_path(&dir));
                        }
                    }
                    State::Missing => {
                        if primary(ui, t, &i18n::t("hub.btn_add_start")).clicked() {
                            self.add_shortcut();
                        }
                    }
                    State::Stale(_) => {
                        if primary(ui, t, &i18n::t("hub.btn_point")).clicked() {
                            self.add_shortcut();
                        }
                        if ui.button(RichText::new(i18n::t("hub.remove")).font(FontId::proportional(12.5))).clicked() {
                            self.remove_shortcut();
                        }
                    }
                });
                // The desktop shortcut: the same three states, its own buttons.
                ui.add_space(6.0);
                let (status, color) = match &self.desktop {
                    State::Ok => (i18n::t("hub.dt_ok"), t.good),
                    State::Missing => (i18n::t("hub.dt_missing"), t.muted),
                    State::Stale(p) if p.as_os_str().is_empty() => (i18n::t("hub.dt_unreadable"), t.bad),
                    State::Stale(p) => (i18n::tf("hub.dt_stale", &[("path", &p.display().to_string())]), t.bad),
                };
                ui.label(RichText::new(status).color(color));
                ui.horizontal(|ui| match self.desktop.clone() {
                    State::Ok => {
                        if ui.button(RichText::new(i18n::t("hub.btn_remove_desktop")).font(FontId::proportional(12.5))).clicked() {
                            self.remove_desktop();
                        }
                    }
                    State::Missing => {
                        if primary(ui, t, &i18n::t("hub.btn_add_desktop")).clicked() {
                            self.add_desktop();
                        }
                    }
                    State::Stale(_) => {
                        if primary(ui, t, &i18n::t("hub.btn_point")).clicked() {
                            self.add_desktop();
                        }
                        if ui.button(RichText::new(i18n::t("hub.remove")).font(FontId::proportional(12.5))).clicked() {
                            self.remove_desktop();
                        }
                    }
                });
                if let Some((msg, bad)) = &self.msg {
                    ui.label(RichText::new(msg).size(12.0).color(if *bad { t.bad } else { t.good }));
                }
                ui.label(RichText::new(i18n::t("hub.pin_note")).size(12.0).color(t.faint));

                // File Explorer's right-click menu: Windows only.
                if vf_core::menu::SUPPORTED {
                section(ui, t, &i18n::t("menu.sec"));
                let on = crate::menu::installed();
                ui.label(RichText::new(i18n::t(if on { "menu.on" } else { "menu.off" })).color(if on { t.good } else { t.muted }));
                ui.horizontal(|ui| {
                    if on {
                        if ui.button(RichText::new(i18n::t("menu.btn_remove")).font(FontId::proportional(12.5))).clicked() {
                            self.menu_msg = Some(match crate::menu::uninstall() {
                                Ok(()) => (i18n::t("menu.removed"), false),
                                Err(e) => (e, true),
                            });
                        }
                        if ui.button(RichText::new(i18n::t("menu.btn_rewrite")).font(FontId::proportional(12.5))).on_hover_text(i18n::t("menu.btn_rewrite_hint")).clicked() {
                            self.menu_msg = Some(match crate::menu::install() {
                                Ok(n) => (i18n::tf("menu.added", &[("n", &n.to_string())]), false),
                                Err(e) => (e, true),
                            });
                        }
                    } else if primary(ui, t, &i18n::t("menu.btn_add")).clicked() {
                        self.menu_msg = Some(match crate::menu::install() {
                            Ok(n) => (i18n::tf("menu.added", &[("n", &n.to_string())]), false),
                            Err(e) => (e, true),
                        });
                    }
                });
                if let Some((msg, bad)) = &self.menu_msg {
                    ui.label(RichText::new(msg).size(12.0).color(if *bad { t.bad } else { t.good }));
                }
                ui.label(RichText::new(i18n::t("menu.drag_note")).size(12.0).color(t.faint));
                ui.label(RichText::new(i18n::t("menu.note")).size(12.0).color(t.faint));
                }

                section(ui, t, &i18n::t("hub.sec_data"));
                if let Some(dir) = data_dir() {
                    ui.label(RichText::new(dir.display().to_string()).font(FontId::proportional(12.5)).color(t.muted));
                    if ui.button(RichText::new(i18n::t("hub.btn_open_data")).font(FontId::proportional(12.5))).clicked() {
                        let _ = std::fs::create_dir_all(&dir);
                        vf_core::platform::open_folder(&dir);
                    }
                }
                ui.label(RichText::new(i18n::tf("hub.data_note", &[("remove", &i18n::t("hub.remove"))])).size(12.0).color(t.faint));

            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(i18n::tf("hub.version_label", &[("v", env!("CARGO_PKG_VERSION"))])).font(FontId::proportional(12.5)).color(t.faint));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(RichText::new(i18n::t("hub.close")).font(FontId::proportional(12.5))).clicked() {
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
    ui.add_space(16.0);
    ui.label(RichText::new(label).font(vf_ui::theme::strong(13.0)).color(t.ink_strong));
    ui.add_space(4.0);
}

fn primary(ui: &mut egui::Ui, t: &Tokens, label: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(label).font(vf_ui::theme::strong(12.5)).color(t.invert_fg)).fill(t.invert_bg).min_size(vec2(0.0, 30.0)))
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
        ui.painter().rect_filled(rect, 16.0, t.chip);
    }
    paint_gear(ui.painter(), rect.center(), 9.0, color);
    resp.on_hover_text(i18n::t("hub.set_title")).clicked()
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
