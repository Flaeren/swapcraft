//! The tray icon and what happens when the queue changes: the edge glow, the tooltip, and one notification when a
//! batch is over (click it to see the result in its folder).

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::{aura, flyout, gfx, queue};

pub const WM_TRAY: u32 = WM_APP + 3;
const ICON_ID: u32 = 1;
const MENU_QUIT: usize = 1;
const MENU_OPEN: usize = 2;
/// The notification was clicked (the shell sends it in the low word of the callback's lParam).
const NIN_BALLOONUSERCLICK: u32 = 0x0405;

static WAS_ACTIVE: AtomicBool = AtomicBool::new(false);
/// How many were done and failed when the current batch began, so a batch is judged by its own results.
static BASE: Mutex<(usize, usize)> = Mutex::new((0, 0));
static BALLOON_TARGET: Mutex<Option<PathBuf>> = Mutex::new(None);

fn wide<const N: usize>(s: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (i, c) in s.encode_utf16().take(N - 1).enumerate() {
        out[i] = c;
    }
    out
}

fn data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: ICON_ID, ..Default::default() }
}

/// Puts the icon in the notification area (again after Explorer restarts).
pub fn add_icon(hwnd: HWND) {
    unsafe {
        let hi = GetModuleHandleW(None).unwrap_or_default();
        // resource 1 is the SwapCraft icon embedded by build.rs
        let icon = LoadIconW(Some(hi.into()), PCWSTR(1 as *const u16)).or_else(|_| LoadIconW(None, IDI_APPLICATION)).unwrap_or_default();
        let mut nid = data(hwnd);
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = icon;
        nid.szTip = wide("SwapCraft");
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        nid.Anonymous.uVersion = 4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
    }
}

pub fn remove_icon(hwnd: HWND) {
    let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data(hwnd)) };
}

fn set_tip(hwnd: HWND, text: &str) {
    let mut nid = data(hwnd);
    nid.uFlags = NIF_TIP;
    nid.szTip = wide(text);
    let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
}

fn balloon(hwnd: HWND, title: &str, text: &str, target: Option<PathBuf>, error: bool) {
    *BALLOON_TARGET.lock().unwrap_or_else(|e| e.into_inner()) = target;
    let mut nid = data(hwnd);
    nid.uFlags = NIF_INFO;
    nid.szInfoTitle = wide(title);
    nid.szInfo = wide(text);
    nid.dwInfoFlags = if error { NIIF_ERROR } else { NIIF_INFO };
    let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
}

/// The queue changed (called on the window's thread): glow, tooltip, card, and the notification at the end.
pub fn on_queue(hwnd: HWND) {
    let c = queue::counts();
    let active = c.active() > 0;
    let was = WAS_ACTIVE.swap(active, Ordering::Relaxed);
    if active && !was {
        *BASE.lock().unwrap_or_else(|e| e.into_inner()) = (c.done, c.failed);
    }
    if active {
        aura::working();
        set_tip(hwnd, &format!("SwapCraft \u{b7} {} in corso", c.active()));
    } else {
        set_tip(hwnd, "SwapCraft");
    }
    flyout::refresh();
    if was && !active {
        let (base_done, base_failed) = *BASE.lock().unwrap_or_else(|e| e.into_inner());
        let (done, failed) = (c.done.saturating_sub(base_done), c.failed.saturating_sub(base_failed));
        gfx::log(&format!("batch over: done={done} failed={failed}"));
        if failed > 0 {
            aura::finish(false);
            if !flyout::is_visible() {
                let (name, why) = queue::snapshot().iter().rev().find_map(|e| if let queue::Status::Failed(m) = &e.status { Some((e.name.clone(), m.clone())) } else { None }).unwrap_or_default();
                balloon(hwnd, "Conversione non riuscita", &format!("{name}: {why}"), None, true);
            }
        } else if done > 0 {
            aura::finish(true);
            if !flyout::is_visible() {
                let last = queue::last_result();
                let text = if done == 1 {
                    let name = last.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    format!("{name} \u{e8} pronto. Clicca per vederlo nella cartella.")
                } else {
                    format!("{done} file convertiti. Clicca per vederli nella cartella.")
                };
                balloon(hwnd, "SwapCraft", &text, last, false);
            }
        } else {
            aura::stop(); // cancelled: nothing to announce
        }
    }
}

/// The tray icon's own messages: a click opens the queue card, a right click the little menu, and a click on
/// the notification shows the result in its folder.
pub fn on_tray(hwnd: HWND, lparam: LPARAM) {
    match (lparam.0 as u32) & 0xffff {
        x if x == WM_LBUTTONUP => flyout::toggle(),
        x if x == WM_RBUTTONUP || x == WM_CONTEXTMENU => menu(hwnd),
        NIN_BALLOONUSERCLICK => {
            let target = BALLOON_TARGET.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(p) = target {
                queue::reveal(&p);
            }
        }
        _ => {}
    }
}

fn menu(hwnd: HWND) {
    unsafe {
        let Ok(m) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(m, MF_STRING, MENU_OPEN, w!("Mostra la coda"));
        let _ = AppendMenuW(m, MF_STRING, MENU_QUIT, w!("Esci da SwapCraft"));
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let _ = SetForegroundWindow(hwnd); // so the menu closes when the person clicks elsewhere
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, p.x, p.y, Some(0), hwnd, None);
        let _ = DestroyMenu(m);
        match cmd.0 as usize {
            MENU_OPEN => flyout::toggle(),
            MENU_QUIT => {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            _ => {}
        }
    }
}
