//! Windows side of the drawing: handing a premultiplied bitmap to a layered window (per-pixel alpha), and asking the
//! system for monitors and the cursor. The drawing itself is in `draw.rs` (re-exported here).

use std::ffi::c_void;
use std::mem::size_of;

use tiny_skia::Pixmap;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow};

pub use crate::draw::*;

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

fn rect(r: RECT) -> Rect {
    Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
}

pub fn monitor_at(x: i32, y: i32) -> Mon {
    unsafe {
        let mon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        Mon { full: rect(mi.rcMonitor), work: rect(mi.rcWork), scale: dx as f32 / 96.0 }
    }
}

pub fn cursor() -> (i32, i32) {
    let mut p = POINT::default();
    let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p) };
    (p.x, p.y)
}
