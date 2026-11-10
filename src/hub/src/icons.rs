//! The voidflare flame and the app icons, loaded once as textures. Both are made by scripts in `tools/`
//! (`make_flame.py`, `engrave_icons.py`) and stored as raw RGBA, so the hub needs no image decoder.

use egui::{Color32, ColorImage, Context, CornerRadius, Painter, Rect, Stroke, StrokeKind, TextureHandle, TextureOptions, vec2};

/// Size in pixels of the flame textures made by `tools/make_flame.py`.
#[allow(dead_code)]
const FLAME_W: usize = 52;
#[allow(dead_code)]
const FLAME_H: usize = 84;

/// The voidflare mark (anti-aliased textures, one per appearance; the tkinter apps embed the same shape). Out of the
/// header since 0.5, until SwapCraft has a mark of its own.
#[allow(dead_code)]
pub struct Flame {
    dark: TextureHandle,
    light: TextureHandle,
}

#[allow(dead_code)]
impl Flame {
    pub fn new(ctx: &Context) -> Self {
        let load = |name: &str, bytes: &[u8]| {
            ctx.load_texture(name, ColorImage::from_rgba_unmultiplied([FLAME_W, FLAME_H], bytes), TextureOptions::LINEAR)
        };
        Self {
            dark: load("flame-dark", include_bytes!("../assets/flame_dark.rgba")),
            light: load("flame-light", include_bytes!("../assets/flame_light.rgba")),
        }
    }

    /// Draws the mark `height` points tall with its left edge at `left` and centred on `y`.
    pub fn paint(&self, p: &Painter, left: f32, y: f32, height: f32, dark: bool) -> f32 {
        let width = height * FLAME_W as f32 / FLAME_H as f32;
        let rect = Rect::from_min_size(egui::pos2(left, y - height / 2.0), vec2(width, height));
        let texture = if dark { &self.dark } else { &self.light };
        p.image(texture.id(), rect, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        width
    }
}

/// The SwapCraft icon (the butterfly), made by `tools/make_swapcraft_icon.py`: the same picture as the exe's icon.
pub struct Mark(TextureHandle);

impl Mark {
    pub fn new(ctx: &Context) -> Self {
        Self(ctx.load_texture("swapcraft-mark", ColorImage::from_rgba_unmultiplied([256, 256], include_bytes!("../assets/app_icon.rgba")), TextureOptions::LINEAR))
    }

    /// Draws the icon `size` points square with its left edge at `left`, centred on `y`. Returns its width.
    pub fn paint(&self, p: &Painter, left: f32, y: f32, size: f32) -> f32 {
        let rect = Rect::from_min_size(egui::pos2(left, y - size / 2.0), vec2(size, size));
        p.image(self.0.id(), rect, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        size
    }
}

/// The engraved app icons made by `tools/engrave_icons.py`, as 128 px textures, in `APPS` order.
pub struct AppIcons {
    textures: Vec<TextureHandle>,
}

macro_rules! icon_bytes {
    ($($id:literal),* $(,)?) => {
        [$(($id, include_bytes!(concat!("../assets/icons/", $id, ".rgba")) as &[u8])),*]
    };
}

const ICON_FILES: [(&str, &[u8]); 9] = icon_bytes!(
    "dwg-to-pdf", "word-to-pdf", "excel-to-pdf", "slides-to-pdf", "pdf-tools", "image-converter", "audio-converter",
    "video-converter", "ebook-converter",
);

impl AppIcons {
    pub fn new(ctx: &Context, ids: &[&str]) -> Self {
        let textures = ids
            .iter()
            .map(|id| {
                let bytes = ICON_FILES.iter().find(|(name, _)| name == id).map(|(_, b)| *b).unwrap_or(&[]);
                let image = if bytes.len() == 128 * 128 * 4 {
                    ColorImage::from_rgba_unmultiplied([128, 128], bytes)
                } else {
                    ColorImage::filled([128, 128], Color32::TRANSPARENT)
                };
                ctx.load_texture(format!("icon-{id}"), image, TextureOptions::LINEAR)
            })
            .collect();
        Self { textures }
    }

    /// Draws app `index` filling `tile`, with a hairline `edge` so the dark tile shows against the page;
    /// `tint` dims it (white leaves it as drawn).
    pub fn paint(&self, p: &Painter, index: usize, tile: Rect, tint: Color32, edge: Color32) {
        if let Some(texture) = self.textures.get(index) {
            p.image(texture.id(), tile, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), tint);
            // the artwork leaves a 4 % margin around its rounded tile, so the edge follows the tile, not the texture
            let radius = (tile.width() * 0.2).round() as u8;
            p.rect_stroke(tile.shrink(tile.width() * 0.04), CornerRadius::same(radius), Stroke::new(1.0, edge), StrokeKind::Inside);
        }
    }
}
