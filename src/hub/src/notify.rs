//! A short system notification (bottom right on Windows) for what finishes while nobody is looking at the hub: one per
//! app updated. The system shows it and takes it away by itself; it never takes the focus.

/// True when the hub is minimized or another program is in front: only then is a notification worth showing.
#[cfg(windows)]
pub fn out_of_sight(_ctx: &eframe::egui::Context, frame: &eframe::Frame) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, IsIconic};
    let Some(hwnd) = hwnd(frame) else { return false };
    // our own dialogs count as the hub being in front, so the foreground is compared by process
    let mut front = 0u32;
    unsafe {
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut front));
        IsIconic(hwnd).as_bool() || front != std::process::id()
    }
}

#[cfg(not(windows))]
pub fn out_of_sight(ctx: &eframe::egui::Context, _frame: &eframe::Frame) -> bool {
    ctx.input(|i| i.viewport().minimized == Some(true) || i.viewport().focused == Some(false))
}

#[cfg(windows)]
fn hwnd(frame: &eframe::Frame) -> Option<windows::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(windows::Win32::Foundation::HWND(h.hwnd.get() as *mut _)),
        _ => None,
    }
}

/// Windows ties a notification to an icon in the notification area: ours is there only while one is being shown.
#[cfg(windows)]
mod tray {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Shell::*;
    use windows::Win32::UI::WindowsAndMessaging::{IDI_APPLICATION, LoadIconW};
    use windows::core::PCWSTR;

    const ICON_ID: u32 = 7;
    /// What one notification takes on screen: Windows shows them one after the other, and taking the icon away
    /// drops the ones still waiting (UPDATE ALL can finish nine apps faster than nine can be read).
    const EACH: Duration = Duration::from_secs(8);
    /// Left after the last one has had its turn.
    const SPARE: Duration = Duration::from_secs(7);
    /// The window the icon belongs to (0: no icon).
    static OWNER: AtomicIsize = AtomicIsize::new(0);
    /// When the last notification waiting will have had its turn.
    static QUEUE_END: Mutex<Option<Instant>> = Mutex::new(None);

    /// The end of the queue once one more notification joins it.
    pub fn queue_end(before: Option<Instant>, now: Instant) -> Instant {
        before.map_or(now, |end| end.max(now)) + EACH
    }

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

    pub fn show(hwnd: HWND, text: &str, error: bool) -> bool {
        let shown = unsafe {
            let mut nid = data(hwnd);
            if OWNER.load(Ordering::SeqCst) == 0 {
                let module = GetModuleHandleW(None).unwrap_or_default();
                // resource 1 is the SwapCraft icon embedded by build.rs
                nid.hIcon = LoadIconW(Some(module.into()), PCWSTR(1 as *const u16)).or_else(|_| LoadIconW(None, IDI_APPLICATION)).unwrap_or_default();
                nid.uFlags = NIF_ICON | NIF_TIP;
                nid.szTip = wide("SwapCraft");
                if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                    return false;
                }
                OWNER.store(hwnd.0 as isize, Ordering::SeqCst);
            }
            nid.uFlags = NIF_INFO;
            nid.szInfoTitle = wide("SwapCraft");
            nid.szInfo = wide(text);
            nid.dwInfoFlags = if error { NIIF_ERROR } else { NIIF_INFO };
            Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool()
        };
        let now = Instant::now();
        let end = {
            let mut queue = QUEUE_END.lock().unwrap_or_else(|e| e.into_inner());
            let end = queue_end(*queue, now);
            *queue = Some(end);
            end
        };
        std::thread::spawn(move || {
            std::thread::sleep(end - now + SPARE);
            // only the timer of the last notification removes the icon
            if *QUEUE_END.lock().unwrap_or_else(|e| e.into_inner()) == Some(end) {
                clear();
            }
        });
        shown
    }

    pub fn clear() {
        let owner = OWNER.swap(0, Ordering::SeqCst);
        if owner != 0 {
            let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data(HWND(owner as *mut _))) };
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::time::{Duration, Instant};

    #[test]
    fn the_icon_waits_for_every_queued_notification() {
        let start = Instant::now();
        let mut end = None;
        // nine apps finishing one second apart still need nine turns on screen
        for second in 0..9 {
            end = Some(super::tray::queue_end(end, start + Duration::from_secs(second)));
        }
        assert_eq!(end, Some(start + Duration::from_secs(72)));
        // after a quiet spell the queue starts again from now
        assert_eq!(super::tray::queue_end(end, start + Duration::from_secs(100)), start + Duration::from_secs(108));
    }
}

/// Shows `text` under the title "SwapCraft". Nothing happens where the system has no way to show it.
#[cfg(windows)]
pub fn show(frame: &eframe::Frame, text: &str, error: bool) {
    if let Some(hwnd) = hwnd(frame) {
        tray::show(hwnd, text, error);
    }
}

/// Through `notify-send` (libnotify's own command).
#[cfg(target_os = "linux")]
pub fn show(_frame: &eframe::Frame, text: &str, _error: bool) {
    spawn(std::process::Command::new("notify-send").args(["--app-name=SwapCraft", "--icon=swapcraft", "SwapCraft", text]));
}

#[cfg(target_os = "macos")]
pub fn show(_frame: &eframe::Frame, text: &str, _error: bool) {
    // the text travels as an argument, never as part of the script
    spawn(std::process::Command::new("osascript").args([
        "-e",
        "on run argv",
        "-e",
        "display notification (item 1 of argv) with title \"SwapCraft\"",
        "-e",
        "end run",
        text,
    ]));
}

#[cfg(not(windows))]
fn spawn(cmd: &mut std::process::Command) {
    use std::process::Stdio;
    if let Ok(mut child) = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Takes our icon out of the notification area when the hub closes.
pub fn clear() {
    #[cfg(windows)]
    tray::clear();
}
