//! The voidflare icon next to "Convert here with voidflare": the small icon of voidflare.exe turned into the 32-bit
//! bitmap with premultiplied alpha that a menu item wants (`MENUITEMINFOW::hbmpItem`).
//!
//! Made once per Explorer process (on the first drag that succeeds) and kept for as long as the DLL is loaded: Explorer may still be drawing the menu
//! after it lets go of our handler, so a bitmap freed with the handler could vanish from an open menu.

use std::path::Path;
use std::sync::Mutex;

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, HBITMAP, ReleaseDC,
};
use windows::Win32::UI::Shell::ExtractIconExW;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};
use windows::core::HSTRING;

/// The bitmap as a raw handle (0 = not made yet): `HBITMAP` itself is not `Send`.
static BITMAP: Mutex<usize> = Mutex::new(0);

/// The menu bitmap for `exe`'s icon, or None when it cannot be made (the menu then shows the text alone). Only a
/// success is kept: a failure (voidflare.exe missing for a moment) is tried again on the next drag.
pub fn menu_bitmap(exe: &Path) -> Option<HBITMAP> {
    let mut raw = BITMAP.lock().ok()?;
    if *raw == 0 {
        *raw = make(exe)?.0 as usize;
    }
    Some(HBITMAP(*raw as *mut _))
}

fn make(exe: &Path) -> Option<HBITMAP> {
    let mut small = HICON::default();
    let found = unsafe { ExtractIconExW(&HSTRING::from(exe.as_os_str()), 0, None, Some(&mut small), 1) };
    if found == 0 || small.is_invalid() {
        return None;
    }
    let bitmap = from_icon(small);
    let _ = unsafe { DestroyIcon(small) };
    bitmap
}

fn header(w: i32, h: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down rows
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Reads the icon's colour bitmap as 32-bit BGRA and copies it, premultiplied, into a new DIB section.
fn from_icon(icon: HICON) -> Option<HBITMAP> {
    let mut info = ICONINFO::default();
    unsafe { GetIconInfo(icon, &mut info) }.ok()?;
    let result = (|| {
        if info.hbmColor.is_invalid() {
            return None; // a monochrome icon: not ours
        }
        let dc = unsafe { GetDC(None) };
        // First call: the size of the colour bitmap.
        let mut probe = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, ..Default::default() }, ..Default::default() };
        let pixels = (|| {
            if unsafe { GetDIBits(dc, info.hbmColor, 0, 0, None, &mut probe, DIB_RGB_COLORS) } == 0 {
                return None;
            }
            let (w, h) = (probe.bmiHeader.biWidth, probe.bmiHeader.biHeight.abs());
            if !(1..=256).contains(&w) || !(1..=256).contains(&h) {
                return None;
            }
            let mut bmi = header(w, h);
            let mut px = vec![0u8; (w * h * 4) as usize];
            let lines = unsafe { GetDIBits(dc, info.hbmColor, 0, h as u32, Some(px.as_mut_ptr().cast()), &mut bmi, DIB_RGB_COLORS) };
            (lines == h).then_some((w, h, px))
        })();
        unsafe { ReleaseDC(None, dc) };
        let (w, h, mut px) = pixels?;
        premultiply(&mut px);
        let bmi = header(w, h);
        let mut bits = std::ptr::null_mut();
        let bitmap = unsafe { CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) }.ok()?;
        if bits.is_null() {
            let _ = unsafe { DeleteObject(bitmap.into()) };
            return None;
        }
        unsafe { std::ptr::copy_nonoverlapping(px.as_ptr(), bits.cast::<u8>(), px.len()) };
        Some(bitmap)
    })();
    unsafe {
        if !info.hbmColor.is_invalid() {
            let _ = DeleteObject(info.hbmColor.into());
        }
        if !info.hbmMask.is_invalid() {
            let _ = DeleteObject(info.hbmMask.into());
        }
    }
    result
}

/// BGRA in place: colour times alpha. An icon with no alpha at all (every pixel 0) is taken as fully opaque.
pub(crate) fn premultiply(px: &mut [u8]) {
    let has_alpha = px.chunks_exact(4).any(|p| p[3] != 0);
    for p in px.chunks_exact_mut(4) {
        if !has_alpha {
            p[3] = 255;
            continue;
        }
        let a = p[3] as u32;
        for c in &mut p[..3] {
            *c = ((*c as u32 * a + 127) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplies_and_treats_no_alpha_as_opaque() {
        let mut px = vec![200, 100, 50, 128, 255, 255, 255, 0];
        premultiply(&mut px);
        assert_eq!(px, vec![100, 50, 25, 128, 0, 0, 0, 0]);
        let mut flat = vec![10, 20, 30, 0];
        premultiply(&mut flat);
        assert_eq!(flat, vec![10, 20, 30, 255]);
    }

    #[test]
    fn makes_a_32_bit_bitmap_from_a_real_exe_icon() {
        use windows::Win32::Graphics::Gdi::{BITMAP, GetObjectW};
        let windir = std::env::var_os("WINDIR").expect("WINDIR");
        let b = make(&Path::new(&windir).join("explorer.exe")).expect("bitmap from explorer.exe");
        let mut bm = BITMAP::default();
        let n = unsafe { GetObjectW(b.into(), size_of::<BITMAP>() as i32, Some((&mut bm as *mut BITMAP).cast())) };
        assert!(n > 0);
        assert_eq!(bm.bmBitsPixel, 32);
        assert!(bm.bmWidth >= 16 && bm.bmWidth == bm.bmHeight, "{}x{}", bm.bmWidth, bm.bmHeight);
        let _ = unsafe { DeleteObject(b.into()) };
        // an exe without an icon gives None, not a crash
        assert!(make(Path::new(r"C:\no\such\file.exe")).is_none());
    }
}
