//! Pieces every voidflare app window is built from: header, section heading, buttons, file list, log.

use std::collections::BTreeSet;
use std::path::PathBuf;

use eframe::egui::{
    self, Align, Align2, Color32, ColorImage, FontId, Layout, RichText, Sense, TextureHandle, TextureOptions, pos2, vec2,
};
use vf_core::i18n::{self, Lang};
use vf_core::t;

use crate::theme::Tokens;

/// Side padding of every window.
pub const PAD: f32 = 20.0;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Plain,
    Good,
    Bad,
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// The app's own icon (128x128 RGBA) as a texture.
pub fn load_icon(ctx: &egui::Context, rgba_128: &[u8]) -> TextureHandle {
    ctx.load_texture("app-icon", ColorImage::from_rgba_unmultiplied([128, 128], rgba_128), TextureOptions::LINEAR)
}

/// A mono upper-case label over a hairline, the section heading of every voidflare app.
pub fn section(ui: &mut egui::Ui, tk: &Tokens, text: &str) {
    ui.add_space(18.0);
    ui.label(RichText::new(text.to_uppercase()).font(mono(11.0)).color(tk.faint));
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 7.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.top() + 0.5, tk.hairline());
    ui.add_space(4.0);
}

pub fn button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text.to_uppercase()).font(mono(11.5))))
}

/// The primary (inverted) button.
pub fn primary(ui: &mut egui::Ui, tk: &Tokens, text: &str, enabled: bool) -> egui::Response {
    let b = egui::Button::new(RichText::new(text.to_uppercase()).font(mono(12.0)).color(tk.invert_fg)).fill(tk.invert_bg);
    ui.add_enabled(enabled, b)
}

/// Icon, title and the language menu, over a hairline.
pub fn header(ui: &mut egui::Ui, tk: &Tokens, icon: &TextureHandle, title: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
    let icon_rect = egui::Rect::from_center_size(pos2(rect.left() + PAD + 20.0, rect.center().y), vec2(40.0, 40.0));
    ui.painter().image(icon.id(), icon_rect, egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    ui.painter().text(pos2(icon_rect.right() + 12.0, rect.center().y), Align2::LEFT_CENTER, title, FontId::proportional(20.0), tk.ink_strong);
    ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, tk.hairline());

    let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(PAD, 0.0))).layout(Layout::right_to_left(Align::Center)));
    let current = i18n::language();
    egui::ComboBox::from_id_salt("lang").selected_text(RichText::new(current.name()).font(mono(11.5))).show_ui(&mut bar, |ui| {
        for lang in Lang::ALL {
            if ui.selectable_label(lang == current, lang.name()).clicked() {
                i18n::set_language(lang, true);
            }
        }
    });
    bar.label(RichText::new(t("ui.language").trim_end_matches([':', ' ', '\u{a0}']).to_uppercase()).font(mono(11.0)).color(tk.faint));
}

/// The list of files, with click to select and ctrl+click to add to the selection.
pub fn file_list(ui: &mut egui::Ui, tk: &Tokens, files: &[PathBuf], selected: &mut BTreeSet<usize>) {
    egui::Frame::new().fill(tk.sunken).stroke(tk.hairline()).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("files").min_scrolled_height(150.0).max_height(190.0).auto_shrink([false, false]).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let ctrl = ui.input(|i| i.modifiers.command);
            for (i, file) in files.iter().enumerate() {
                let on = selected.contains(&i);
                if ui.selectable_label(on, file.to_string_lossy()).clicked() {
                    if ctrl {
                        if !selected.remove(&i) {
                            selected.insert(i);
                        }
                    } else {
                        selected.clear();
                        selected.insert(i);
                    }
                }
            }
        });
    });
}

/// The coloured log under the progress bar.
pub fn log_view(ui: &mut egui::Ui, tk: &Tokens, log: &[(String, Kind)]) {
    egui::Frame::new().fill(tk.sunken).stroke(tk.hairline()).inner_margin(8.0).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("log").max_height(150.0).min_scrolled_height(110.0).stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            for (line, kind) in log {
                let color = match kind {
                    Kind::Plain => tk.ink,
                    Kind::Good => tk.good,
                    Kind::Bad => tk.bad,
                };
                ui.label(RichText::new(line).font(mono(12.0)).color(color));
            }
        });
    });
}

pub fn progress(ui: &mut egui::Ui, done: usize, total: usize) {
    ui.add(egui::ProgressBar::new(if total == 0 { 0.0 } else { done as f32 / total as f32 }).desired_height(4.0));
}

/// Removes the entries at `selected` from `files`.
pub fn remove_selected(files: &mut Vec<PathBuf>, selected: &mut BTreeSet<usize>) {
    let keep: Vec<PathBuf> = files.iter().enumerate().filter(|(i, _)| !selected.contains(i)).map(|(_, f)| f.clone()).collect();
    *files = keep;
    selected.clear();
}
