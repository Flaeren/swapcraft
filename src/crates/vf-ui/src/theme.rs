//! voidflare colours and widget look. Same family as the Craft launchers (hard edges, hairline
//! rules, near-black surfaces, mono labels); the accent is our own indigo, the colour of the flame mark.

use egui::{Color32, CornerRadius, Stroke, Visuals};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub dark: bool,
    /// Page and card surface.
    pub bg: Color32,
    /// Wells and hovers.
    pub sunken: Color32,
    pub ink: Color32,
    pub ink_strong: Color32,
    pub muted: Color32,
    pub faint: Color32,
    /// Hairlines.
    pub line: Color32,
    pub line_strong: Color32,
    /// Brand accent, for surfaces.
    pub accent: Color32,
    /// Brand accent, for text.
    pub accent_ink: Color32,
    /// Primary buttons.
    pub invert_bg: Color32,
    pub invert_fg: Color32,
    pub good: Color32,
    pub bad: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    let [_, r, g, b] = rgb.to_be_bytes();
    Color32::from_rgb(r, g, b)
}

const fn ink_alpha(rgb: u32, alpha: u8) -> Color32 {
    let [_, r, g, b] = rgb.to_be_bytes();
    Color32::from_rgba_unmultiplied_const(r, g, b, alpha)
}

impl Tokens {
    pub const DARK: Self = Self {
        dark: true,
        bg: hex(0x121316),
        sunken: hex(0x060607),
        ink: hex(0xf2f1ee),
        ink_strong: hex(0xffffff),
        muted: ink_alpha(0xf2f1ee, 153),
        faint: ink_alpha(0xf2f1ee, 97),
        line: ink_alpha(0xf2f1ee, 38),
        line_strong: ink_alpha(0xf2f1ee, 102),
        accent: hex(0x6366f1),
        accent_ink: hex(0xa5b4fc),
        invert_bg: hex(0xffffff),
        invert_fg: hex(0x0b0b0c),
        good: hex(0x3fd6b8),
        bad: hex(0xff6259),
    };

    pub const LIGHT: Self = Self {
        dark: false,
        bg: hex(0xf2f1ee),
        sunken: hex(0xe9e8e4),
        ink: hex(0x101014),
        ink_strong: hex(0x000000),
        muted: ink_alpha(0x101014, 158),
        faint: ink_alpha(0x101014, 107),
        line: ink_alpha(0x101014, 41),
        line_strong: ink_alpha(0x101014, 102),
        accent: hex(0x4f46e5),
        accent_ink: hex(0x4338ca),
        invert_bg: hex(0x101014),
        invert_fg: hex(0xf2f1ee),
        good: hex(0x0a7563),
        bad: hex(0xb3261e),
    };

    pub fn get(ctx: &egui::Context) -> Self {
        if ctx.theme() == egui::Theme::Dark { Self::DARK } else { Self::LIGHT }
    }

    pub fn hairline(&self) -> Stroke {
        Stroke::new(1.0, self.line)
    }
}

pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// The appearance the user picked in the hub's settings. Saved in `%APPDATA%\voidflare\theme.txt` (one word:
/// `system`, `dark` or `light`), shared by the hub and every app, like the language. Without the file, or with
/// anything else in it, voidflare follows Windows, as it did before the choice existed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    System,
    Dark,
    Light,
}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Dark, Self::Light];

    pub fn name(self) -> &'static str {
        match self {
            Self::System => "Same as Windows",
            Self::Dark => "Dark",
            Self::Light => "Light",
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    pub fn preference(self) -> egui::ThemePreference {
        match self {
            Self::System => egui::ThemePreference::System,
            Self::Dark => egui::ThemePreference::Dark,
            Self::Light => egui::ThemePreference::Light,
        }
    }

    fn file() -> Option<std::path::PathBuf> {
        std::env::var_os("APPDATA").map(|d| std::path::PathBuf::from(d).join("voidflare").join("theme.txt"))
    }

    fn parse(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "dark" => Self::Dark,
            "light" => Self::Light,
            _ => Self::System,
        }
    }

    /// The saved choice, `System` when there is none.
    pub fn load() -> Self {
        Self::file().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Self::parse(&t)).unwrap_or(Self::System)
    }

    /// Remembers the choice for the next start of the hub and of every app. A failed write is not an error: the
    /// choice still applies to the running window.
    pub fn save(self) {
        if let Some(path) = Self::file() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, self.word());
        }
    }
}

/// Installs both appearances and applies the saved choice (egui follows the system when it is `System`).
pub fn install(ctx: &egui::Context) {
    ctx.set_visuals_of(egui::Theme::Dark, visuals(&Tokens::DARK));
    ctx.set_visuals_of(egui::Theme::Light, visuals(&Tokens::LIGHT));
    ctx.set_theme(Appearance::load().preference());
}

#[cfg(test)]
mod tests {
    use super::Appearance;

    #[test]
    fn appearance_words_round_trip_and_unknown_means_system() {
        for a in Appearance::ALL {
            assert_eq!(Appearance::parse(a.word()), a);
        }
        assert_eq!(Appearance::parse(" Dark\r\n"), Appearance::Dark);
        assert_eq!(Appearance::parse(""), Appearance::System);
        assert_eq!(Appearance::parse("purple"), Appearance::System);
    }
}

fn visuals(t: &Tokens) -> Visuals {
    let mut v = if t.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = t.bg;
    v.window_fill = t.bg;
    v.extreme_bg_color = t.sunken;
    v.text_edit_bg_color = Some(t.sunken);
    v.faint_bg_color = t.sunken;
    v.window_stroke = Stroke::new(1.0, t.line_strong);
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    v.override_text_color = Some(t.ink);
    v.hyperlink_color = t.accent_ink;
    v.selection.bg_fill = t.accent;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    for (w, fill, stroke, fg) in [
        (&mut v.widgets.noninteractive, t.bg, t.line, t.muted),
        (&mut v.widgets.inactive, t.bg, t.line, t.ink),
        (&mut v.widgets.hovered, t.sunken, t.line_strong, t.ink_strong),
        (&mut v.widgets.active, t.sunken, t.line_strong, t.ink_strong),
        (&mut v.widgets.open, t.sunken, t.line_strong, t.ink_strong),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::new(1.0, stroke);
        w.corner_radius = CornerRadius::ZERO;
        w.fg_stroke = Stroke::new(1.0, fg);
        w.expansion = 0.0;
    }
    v
}
