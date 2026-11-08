//! Loads the built `voidflare_dragdrop.dll` by path, the way Explorer would, and prints the menu it adds:
//!
//!     cargo run --release -p vf-dragdrop --example host -- <dll> <drop folder> [--invoke N] <files...>
//!
//! With `--invoke N` it also clicks entry N (as printed), which starts voidflare.exe for real. The menu file is
//! read from `%APPDATA%\voidflare\dragmenu.tsv`, so point APPDATA somewhere else to try a test file.

#[path = "../testutil/host_lib.rs"]
mod host_lib;

use std::path::PathBuf;

use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{HSTRING, s};

fn print(items: &[host_lib::Item], depth: usize) {
    for it in items {
        let id = it.offset.map_or(String::from("-"), |o| o.to_string());
        println!("{}[{id}] {}", "  ".repeat(depth), if it.label.is_empty() { "----" } else { &it.label });
        print(&it.children, depth + 1);
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut invoke = None;
    if let Some(i) = args.iter().position(|a| a == "--invoke") {
        invoke = args.get(i + 1).and_then(|n| n.parse().ok());
        args.drain(i..=i + 1);
    }
    let [dll, target, files @ ..] = args.as_slice() else {
        eprintln!("usage: host <dll> <drop folder> [--invoke N] <files...>");
        std::process::exit(2);
    };
    let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
    let module = unsafe { LoadLibraryW(&HSTRING::from(dll.as_str())) }.expect("LoadLibraryW");
    let get = unsafe { GetProcAddress(module, s!("DllGetClassObject")) }.expect("no DllGetClassObject export");
    let can_unload = unsafe { GetProcAddress(module, s!("DllCanUnloadNow")) }.is_some();
    println!("exports: DllGetClassObject yes, DllCanUnloadNow {}", if can_unload { "yes" } else { "NO" });
    let get: host_lib::GetClassObject = unsafe { std::mem::transmute(get) };
    match host_lib::drive(get, &PathBuf::from(target), &files, invoke) {
        Ok((hr, items)) => {
            println!("QueryContextMenu -> 0x{:08X} (items added: {})", hr.0, hr.0 & 0xFFFF);
            print(&items, 0);
            if let Some(n) = invoke {
                println!("invoked {n}");
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
