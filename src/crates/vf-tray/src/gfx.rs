//! Drawing shared by the bar, the glow and the queue card: text (Inter, IBM Plex Mono), pills, and handing a
//! premultiplied bitmap to a layered window (per-pixel alpha).

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::OnceLock;

use tiny_skia::{Color, Paint, PathBuilder, Pixmap};
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow};

// the SwapCraft Black theme (vf-ui theme.rs), as RGBA
pub const CARD: [u8; 4] = [18, 18, 18, 255];
pub const CHIP: [u8; 4] = [30, 30, 29, 255];
pub const LINE: [u8; 4] = [64, 64, 63, 255];
pub const INK: [u8; 4] = [227, 226, 224, 255];
pub const STRONG: [u8; 4] = [250, 249, 246, 255];
pub const MUTED: [u8; 4] = [154, 154, 152, 255];
pub const FAINT: [u8; 4] = [138, 138, 136, 255];
pub const VIOLET: [u8; 4] = [203, 176, 247, 255];
pub const ORANGE: [u8; 4] = [240, 163, 90, 255];
pub const GREEN: [u8; 4] = [63, 214, 184, 255];
pub const RED: [u8; 4] = [255, 98, 89, 255];

static INTER: &[u8] = include_bytes!("../../vf-ui/assets/fonts/Inter-Regular.ttf");
static INTER_SEMI: &[u8] = include_bytes!("../../vf-ui/assets/fonts/Inter-SemiBold.ttf");
static PLEX: &[u8] = include_bytes!("../../vf-ui/assets/fonts/IBMPlexMono-Regular.ttf");

pub struct Fonts {
    pub inter: fontdue::Font,
    pub semi: fontdue::Font,
    pub plex: fontdue::Font,
}

pub fn fonts() -> &'static Fonts {
    static F: OnceLock<Fonts> = OnceLock::new();
    F.get_or_init(|| {
        let load = |b: &[u8]| fontdue::Font::from_bytes(b, fontdue::FontSettings::default()).expect("built-in font");
        Fonts { inter: load(INTER), semi: load(INTER_SEMI), plex: load(PLEX) }
    })
}

pub fn text_width(font: &fontdue::Font, px: f32, spacing: f32, s: &str) -> f32 {
    s.chars().map(|c| font.metrics(c, px).advance_width + spacing).sum()
}

/// `s` shortened with an ellipsis so it is at most `max` pixels wide.
pub fn fit(font: &fontdue::Font, px: f32, max: f32, s: &str) -> String {
    if text_width(font, px, 0.0, s) <= max {
        return s.to_owned();
    }
    let mut out = String::new();
    for c in s.chars() {
        let next = format!("{out}{c}\u{2026}");
        if text_width(font, px, 0.0, &next) > max {
            break;
        }
        out.push(c);
    }
    format!("{out}\u{2026}")
}

/// Draws `s` with its vertical centre at `cy`, its left edge at `x`; coverage is blended into the premultiplied pixmap.
pub fn draw_text(pm: &mut Pixmap, font: &fontdue::Font, px: f32, spacing: f32, x: f32, cy: f32, rgba: [u8; 4], s: &str) {
    let Some(lm) = font.horizontal_line_metrics(px) else { return };
    let baseline = cy + (lm.ascent + lm.descent) / 2.0;
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    let mut pen = x;
    for c in s.chars() {
        let (m, cov) = font.rasterize(c, px);
        let gx = (pen + m.xmin as f32).round() as i32;
        let gy = (baseline - m.height as f32 - m.ymin as f32).round() as i32;
        for row in 0..m.height as i32 {
            for col in 0..m.width as i32 {
                let (dx, dy) = (gx + col, gy + row);
                if dx < 0 || dy < 0 || dx >= w || dy >= h {
                    continue;
                }
                let a = cov[(row as usize) * m.width + col as usize] as f32 / 255.0 * (rgba[3] as f32 / 255.0);
                if a <= 0.0 {
                    continue;
                }
                let i = ((dy * w + dx) * 4) as usize;
                for k in 0..3 {
                    data[i + k] = (rgba[k] as f32 * a + data[i + k] as f32 * (1.0 - a)).round().min(255.0) as u8;
                }
                data[i + 3] = (255.0 * a + data[i + 3] as f32 * (1.0 - a)).round().min(255.0) as u8;
            }
        }
        pen += m.advance_width + spacing;
    }
}

/// Like [`draw_text`] but the right edge is at `x`.
pub fn draw_text_right(pm: &mut Pixmap, font: &fontdue::Font, px: f32, x: f32, cy: f32, rgba: [u8; 4], s: &str) {
    let w = text_width(font, px, 0.0, s);
    draw_text(pm, font, px, 0.0, x - w, cy, rgba, s);
}

/// A rounded rectangle (a full pill when `r` is half the height).
pub fn round_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> tiny_skia::Path {
    let r = r.min(h / 2.0).min(w / 2.0);
    let k = 0.552_284_75 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish().expect("rounded rectangle")
}

pub fn pill(x: f32, y: f32, w: f32, h: f32) -> tiny_skia::Path {
    round_rect(x, y, w, h, h / 2.0)
}

pub fn paint(c: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(c[0], c[1], c[2], c[3]));
    p.anti_alias = true;
    p
}

pub fn with_alpha(c: [u8; 4], a: u8) -> [u8; 4] {
    [c[0], c[1], c[2], a]
}

/// Hands the pixmap to a layered window (per-pixel alpha) at `x`,`y`; `alpha` fades the whole thing (255 = as drawn).
pub fn push(hwnd: HWND, pm: &Pixmap, x: i32, y: i32, alpha: u8) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        if let Ok(bmp) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            let old = SelectObject(mem, bmp.into());
            let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
            for (d, s) in dst.chunks_exact_mut(4).zip(pm.data().chunks_exact(4)) {
                d.copy_from_slice(&[s[2], s[1], s[0], s[3]]); // RGBA -> BGRA, both premultiplied
            }
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
            let _ = UpdateLayeredWindow(hwnd, Some(screen), Some(&POINT { x, y }), Some(&SIZE { cx: w, cy: h }), Some(mem), Some(&POINT { x: 0, y: 0 }), COLORREF(0), Some(&blend), ULW_ALPHA);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp.into());
        }
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
    }
}

/// A monitor: its whole rectangle, its work area (without the taskbar) and its scale (1.0 = 96 dpi).
#[derive(Clone, Copy, Debug)]
pub struct Mon {
    pub full: RECT,
    pub work: RECT,
    pub scale: f32,
}

pub fn monitor_at(x: i32, y: i32) -> Mon {
    unsafe {
        let mon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        Mon { full: mi.rcMonitor, work: mi.rcWork, scale: dx as f32 / 96.0 }
    }
}

pub fn cursor() -> (i32, i32) {
    let mut p = POINT::default();
    let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p) };
    (p.x, p.y)
}

/// Set by the self-test: every window stays out of sight.
pub static FORCE_OFFSCREEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Tests keep every window out of sight (the self-test, or `SWAPCRAFT_TRAY_OFFSCREEN` in the environment).
pub fn offscreen() -> bool {
    FORCE_OFFSCREEN.load(std::sync::atomic::Ordering::Relaxed) || std::env::var_os("SWAPCRAFT_TRAY_OFFSCREEN").is_some()
}

/// Appends a line to the file named by `SWAPCRAFT_TRAY_LOG` (nothing is written when it is not set).
pub fn log(line: &str) {
    if let Some(path) = std::env::var_os("SWAPCRAFT_TRAY_LOG") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{line}");
        }
    }
}
