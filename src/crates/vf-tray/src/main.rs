//! swapcraft-tray: SwapCraft's resident helper. Drag a file in Explorer and a bar slides in from the screen edge
//! with the formats the file can become; drop it on one and the conversion starts, the result lands beside the
//! original, an orange glow runs along the screen edges while it works and turns green when it is done, and the
//! tray icon keeps the queue. Windows only for now.
//!
//!   swapcraft-tray                                        run (one copy per user)
//!   swapcraft-tray --selftest <file> <format>             drop <file> on <format> without a mouse, off-screen
//!   swapcraft-tray --render-glow <png> orange|green|red   the glow's bitmap
//!   swapcraft-tray --render-queue <png>                   the queue card with sample entries
//!   swapcraft-tray --render-bar <png> <ext> [n]           the drop bar for that file type, formats from the n-th on dimmed

#![cfg_attr(not(windows), allow(dead_code))]
#![cfg_attr(windows, windows_subsystem = "windows")]

// the part every system shares: what to offer, the queue, and how things look
mod bar_view;
mod card_view;
mod draw;
mod glow;
mod model;
mod offer;
mod queue;
mod watcher;

// Windows

#[cfg(windows)]
mod aura;
#[cfg(windows)]
mod bar;
#[cfg(windows)]
mod flyout;
#[cfg(windows)]
mod gfx;
#[cfg(windows)]
mod tray;

#[cfg(not(windows))]
fn main() {
    eprintln!("swapcraft-tray is Windows only for now.");
}

#[cfg(windows)]
fn main() {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::Ole::OleInitialize;
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
    use windows::core::w;

    // a program without a console: tests print to the file in SWAPCRAFT_TRAY_OUT, or to the console of whoever started it
    attach_console();
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let _ = OleInitialize(None);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--selftest") => {
            let (Some(file), Some(to)) = (args.get(1), args.get(2)) else {
                println!("usage: --selftest <file> <format>");
                return;
            };
            if let Err(e) = bar::selftest(file, to) {
                println!("selftest failed: {e}");
                std::process::exit(1);
            }
        }
        Some("--render-glow") => {
            let color = match args.get(2).map(String::as_str) {
                Some("green") => gfx::GREEN,
                Some("red") => gfx::RED,
                _ => gfx::ORANGE,
            };
            aura::render_png(color, args.get(1).map(String::as_str).unwrap_or("glow.png"));
        }
        Some("--render-bar") => {
            // a PNG file as the drag, with the formats that need an engine marked missing: `<png> <ext> [missing-from-index]`
            let ext = args.get(2).cloned().unwrap_or_else(|| "docx".to_owned());
            let missing_from = args.get(3).and_then(|s| s.parse().ok());
            bar::render_png(args.get(1).map(String::as_str).unwrap_or("bar.png"), &ext, missing_from);
        }
        Some("--render-queue") => {
            queue::push_demo();
            flyout::render_png(args.get(1).map(String::as_str).unwrap_or("queue.png"));
        }
        _ => {
            // one copy per user: a second start just exits
            let _guard = unsafe { CreateMutexW(None, true, w!("Local\\SwapCraftTray")) };
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                return;
            }
            if let Err(e) = bar::run() {
                gfx::log(&format!("stopped: {e}"));
            }
        }
    }
}

/// With `windows_subsystem = "windows"` there is no console; when started from one, print into it.
#[cfg(windows)]
fn attach_console() {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}
