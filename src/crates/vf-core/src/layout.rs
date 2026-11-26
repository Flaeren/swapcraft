//! Where the programs around the hub live. On Windows the SwapCraft folder shows `SwapCraft.exe` (and `apps\`); the
//! command line, the drop bar and the drag-and-drop DLL sit in `bin\`.
//!
//! The release ZIPs keep them beside the hub, because the updaters already installed look for them there. The hub moves
//! them into `bin\` when it starts, so an old updater, a fresh unzip and the full bundle all end up the same way.

use std::fs;
use std::path::{Path, PathBuf};

use crate::update::{CLI_NAME, DRAG_DLL, HUB_NAME, old_name};

/// The folder for the programs the hub brings along.
pub const BIN: &str = "bin";

/// Only Windows gets the `bin\` folder: on Linux and macOS menu entries and scripts name these programs by path.
const TIDIES: bool = cfg!(windows);

/// The programs that move.
pub const HELPERS: [&str; 3] = [CLI_NAME, crate::tray::EXE_NAME, DRAG_DLL];

/// Where the helper `name` of the hub at `hub_exe` belongs.
pub fn helper_place(hub_exe: &Path, name: &str) -> PathBuf {
    if TIDIES { hub_exe.with_file_name(BIN).join(name) } else { hub_exe.with_file_name(name) }
}

/// Where the helper `name` is now: its place, or beside the hub (a build folder, or not moved yet).
pub fn helper(hub_exe: &Path, name: &str) -> Option<PathBuf> {
    [helper_place(hub_exe, name), hub_exe.with_file_name(name)].into_iter().find(|p| p.is_file())
}

/// The SwapCraft folder, seen from any of its programs: the folder above `bin\` for a helper, its own otherwise.
pub fn home_of(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    let in_bin = TIDIES && dir.file_name().is_some_and(|n| n.eq_ignore_ascii_case(BIN));
    match dir.parent() {
        Some(up) if in_bin && up.join(HUB_NAME).is_file() => Some(up.to_path_buf()),
        _ => Some(dir.to_path_buf()),
    }
}

/// Moves the helpers found beside the hub into `bin\`, over any older copy there; returns the names moved. Only for a
/// hub with its release name, so a build folder is left alone. A running program or a loaded DLL can be moved on the
/// same disk. Best effort: what cannot move stays where it works and is tried again at the next start.
pub fn tidy(hub_exe: &Path) -> Vec<&'static str> {
    let named = hub_exe.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(HUB_NAME));
    if !TIDIES || !named {
        return Vec::new();
    }
    HELPERS.into_iter().filter(|name| move_into(&hub_exe.with_file_name(name), &helper_place(hub_exe, name))).collect()
}

fn move_into(from: &Path, to: &Path) -> bool {
    if !from.is_file() || to.parent().is_none_or(|bin| fs::create_dir_all(bin).is_err()) {
        return false;
    }
    if !to.exists() {
        return fs::rename(from, to).is_ok();
    }
    // an older copy is there, perhaps running: park it under its `.old` name
    let old = old_name(to);
    let _ = fs::remove_file(&old);
    if fs::rename(to, &old).is_err() {
        return false;
    }
    let moved = fs::rename(from, to).is_ok();
    if !moved {
        let _ = fs::rename(&old, to);
    }
    moved
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vf_layout_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(HUB_NAME), b"MZ-hub").unwrap();
        dir
    }

    #[test]
    fn helpers_move_into_bin_and_a_newer_one_replaces_the_one_there() {
        let dir = folder("move");
        let hub = dir.join(HUB_NAME);
        for name in HELPERS {
            fs::write(dir.join(name), b"MZ-first").unwrap();
        }
        assert_eq!(tidy(&hub), HELPERS);
        for name in HELPERS {
            assert!(!dir.join(name).exists(), "{name} still beside the hub");
            assert_eq!(helper(&hub, name).unwrap(), dir.join(BIN).join(name));
        }
        // an updater from before the move puts the new command line beside the hub again
        fs::write(dir.join(CLI_NAME), b"MZ-second").unwrap();
        assert_eq!(tidy(&hub), [CLI_NAME]);
        assert_eq!(fs::read(dir.join(BIN).join(CLI_NAME)).unwrap(), b"MZ-second");
        assert_eq!(tidy(&hub), [""; 0], "nothing left to move");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_hub_in_a_build_folder_moves_nothing() {
        let dir = folder("build");
        fs::write(dir.join(CLI_NAME), b"MZ").unwrap();
        assert_eq!(tidy(&dir.join("voidflare-hub.exe")), [""; 0]);
        assert_eq!(helper(&dir.join("voidflare-hub.exe"), CLI_NAME).unwrap(), dir.join(CLI_NAME));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_program_in_bin_sees_the_folder_above_as_home() {
        let dir = folder("home");
        assert_eq!(home_of(&dir.join(BIN).join(CLI_NAME)).unwrap(), dir);
        assert_eq!(home_of(&dir.join(HUB_NAME)).unwrap(), dir);
        // a `bin` folder with no hub above is just a folder
        let other = dir.join("elsewhere").join(BIN);
        assert_eq!(home_of(&other.join(CLI_NAME)).unwrap(), other);
        fs::remove_dir_all(dir).unwrap();
    }
}
