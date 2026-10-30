//! The Journal look, as in `design-styles\journalpatrimony`: a torch that follows the pointer, a grid of
//! fine dots that swell and part around it, and borders that light up where it passes. Dark theme only.
//! Nothing animates on its own: the hub repaints only while the torch is still catching up with the pointer.

use std::f32::consts::TAU;

use eframe::egui::{self, Color32, Id, Mesh, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};
use vf_ui::theme::Tokens;

/// Dot spacing.
const CELL: f32 = 26.0;
/// How far the pointer swells and pushes the dots.
const REACH: f32 = 120.0;
const TORCH_R: f32 = 170.0;
/// How much of the way to the pointer the torch covers each frame at 60 fps.
const FOLLOW: f32 = 0.12;

#[derive(Clone, Copy)]
pub struct Torch {
    pub pos: Pos2,
    /// 0 when the pointer is outside the window (or the theme is light), 1 when it is on it.
    pub power: f32,
    /// 0..1, grows while a file is dragged over the window.
    pub boost: f32,
}

impl Default for Torch {
    fn default() -> Self {
        Self { pos: Pos2::ZERO, power: 0.0, boost: 0.0 }
    }
}

/// Follows the pointer with a lag, so the torch glides.
#[derive(Default)]
pub struct Fx {
    pos: Option<Pos2>,
}

impl Fx {
    /// `drop_target`: where the torch goes while a file is dragged over the window (the OS gives no
    /// pointer position during such a drag).
    pub fn update(&mut self, ctx: &egui::Context, t: &Tokens, drop_target: Option<Pos2>) -> Torch {
        let dragging = ctx.input(|i| !i.raw.hovered_files.is_empty());
        let target = if dragging { drop_target } else { ctx.input(|i| i.pointer.hover_pos()) };
        let power = ctx.animate_bool_with_time(Id::NULL.with("torch-power"), t.dark && target.is_some(), 0.3);
        let boost = ctx.animate_bool_with_time(Id::NULL.with("torch-boost"), dragging && target.is_some(), 0.3);
        if let Some(goal) = target {
            let pos = self.pos.get_or_insert(goal);
            let dt = ctx.input(|i| i.stable_dt).min(0.1);
            let k = 1.0 - (1.0 - FOLLOW).powf(dt * 60.0);
            *pos += (goal - *pos) * k;
            if (goal - *pos).length() > 0.5 {
                ctx.request_repaint();
            }
        }
        Torch { pos: self.pos.unwrap_or(Pos2::ZERO), power, boost }
    }
}

impl Torch {
    fn radius(&self) -> f32 {
        TORCH_R * (1.0 + 0.6 * self.boost)
    }

    /// 0..1: how close `at` is to the torch, within `reach`.
    pub fn near(&self, at: Pos2, reach: f32) -> f32 {
        (1.0 - (at - self.pos).length() / reach).max(0.0) * self.power
    }

    /// The page behind everything: the torch glow and the dot grid.
    pub fn backdrop(&self, p: &Painter, area: Rect, t: &Tokens) {
        if !t.dark {
            return;
        }
        if self.power > 0.0 {
            let tint = t.ink.lerp_to_gamma(t.accent, self.boost);
            radial(p, self.pos, self.radius(), tint, (0.10 + 0.05 * self.boost) * self.power);
        }
        let cell = if area.width() < 700.0 { 22.0 } else { CELL };
        let (cols, rows) = ((area.width() / cell).ceil() as i32, (area.height() / cell).ceil() as i32);
        for row in 0..=rows {
            for col in 0..=cols {
                let home = pos2(area.left() + col as f32 * cell, area.top() + row as f32 * cell);
                let away = home - self.pos;
                let d = away.length();
                let k = (1.0 - d / REACH).max(0.0) * self.power;
                let at = if k > 0.0 && d > 0.0 { home + away / d * (k * 10.0) } else { home };
                let alpha = 0.14 + k * 0.55;
                let size = 1.0 + k * 1.4;
                let c = Color32::from_rgba_unmultiplied(t.ink.r(), t.ink.g(), t.ink.b(), (alpha * 255.0) as u8);
                p.rect_filled(Rect::from_center_size(at, vec2(size, size)), 0.0, c);
            }
        }
    }

    /// Draws `rect`'s border in `base`, and `glow` over the stretches the torch is near. With `base` set the
    /// whole border is drawn (a dashed one when `gap` > 0); without it only the lit stretches are.
    pub fn border(&self, p: &Painter, rect: Rect, base: Option<Color32>, glow: Color32, dash: f32, gap: f32, reach: f32) {
        let r = rect.shrink(0.5);
        let reach = reach * (1.0 + 0.6 * self.boost);
        if base.is_none() && (self.power <= 0.0 || r.distance_to_pos(self.pos) > reach) {
            return;
        }
        walk(r, dash, gap, |a, b| {
            let k = self.near(a.lerp(b, 0.5), reach);
            match base {
                Some(base) => {
                    p.line_segment([a, b], Stroke::new(1.0, base.lerp_to_gamma(glow, k * 0.9)));
                }
                None if k > 0.02 => {
                    p.line_segment([a, b], Stroke::new(1.0, glow.gamma_multiply(k * 0.8)));
                }
                None => {}
            }
        });
    }
}

/// Calls `f` with each dash along the four sides of `r` (a solid line cut into `dash` pieces when `gap` is 0).
fn walk(r: Rect, dash: f32, gap: f32, mut f: impl FnMut(Pos2, Pos2)) {
    let sides = [(r.left_top(), r.right_top()), (r.right_top(), r.right_bottom()), (r.right_bottom(), r.left_bottom()), (r.left_bottom(), r.left_top())];
    for (a, b) in sides {
        let len = (b - a).length();
        let dir = (b - a) / len.max(f32::EPSILON);
        let mut s = 0.0;
        while s < len {
            let e = (s + dash).min(len);
            f(a + dir * s, a + dir * e);
            s += dash + gap;
        }
    }
}

/// A soft round glow: `alpha` at the centre, fading smoothly to nothing at `radius`.
pub fn radial(p: &Painter, c: Pos2, radius: f32, color: Color32, alpha: f32) {
    const SEG: usize = 48;
    const RINGS: usize = 8;
    if alpha <= 0.0 {
        return;
    }
    let at = |f: f32| Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), (alpha * 255.0 * (1.0 - f) * (1.0 - f)).round().clamp(0.0, 255.0) as u8);
    let mut mesh = Mesh::default();
    mesh.colored_vertex(c, at(0.0));
    for ring in 1..=RINGS {
        let f = ring as f32 / RINGS as f32;
        for s in 0..SEG {
            let a = s as f32 / SEG as f32 * TAU;
            mesh.colored_vertex(c + vec2(a.cos(), a.sin()) * radius * f, at(f));
        }
    }
    for s in 0..SEG {
        mesh.add_triangle(0, 1 + s as u32, 1 + ((s + 1) % SEG) as u32);
    }
    for ring in 1..RINGS {
        let (inner, outer) = (1 + (ring - 1) * SEG, 1 + ring * SEG);
        for s in 0..SEG {
            let n = (s + 1) % SEG;
            let (a, b, c2, d) = ((inner + s) as u32, (outer + s) as u32, (outer + n) as u32, (inner + n) as u32);
            mesh.add_triangle(a, b, c2);
            mesh.add_triangle(a, c2, d);
        }
    }
    p.add(Shape::mesh(mesh));
}
