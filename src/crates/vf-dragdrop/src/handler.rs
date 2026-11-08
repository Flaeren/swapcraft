//! The COM side: class factory, the handler object (`IShellExtInit` + `IContextMenu`) and the two DLL exports.

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_INVALIDARG, E_NOTIMPL, S_FALSE, S_OK};
use windows::Win32::System::Com::{CoTaskMemFree, IClassFactory, IClassFactory_Impl, IDataObject};
use windows::Win32::System::Registry::HKEY;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    CMF_DEFAULTONLY, CMINVOKECOMMANDINFO, GPFIDL_DEFAULT, IContextMenu, IContextMenu_Impl, IShellExtInit, IShellExtInit_Impl, IShellItemArray,
    SHCreateShellItemArrayFromDataObject, SHGetPathFromIDListEx, SIGDN_FILESYSPATH,
};
use windows::Win32::Graphics::Gdi::HBITMAP;
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, HMENU, InsertMenuItemW, MENUITEMINFOW, MFT_SEPARATOR, MFT_STRING, MIIM_BITMAP, MIIM_FTYPE, MIIM_ID, MIIM_STRING,
    MIIM_SUBMENU,
};
use windows::core::{BOOL, GUID, HRESULT, IUnknown, Interface, PSTR, PWSTR, Ref, Result, implement};

use crate::config::{self, Config, Entry};

/// voidflare's drag-and-drop handler. Never change it: the registry entries the hub writes point at it.
pub const CLSID: GUID = GUID::from_u128(0x127b4c5d_6b1d_489c_a1d9_1abf98d0c829);

/// Live objects (factories and handlers), for `DllCanUnloadNow`.
static LIVE: AtomicUsize = AtomicUsize::new(0);

struct Live;
impl Live {
    fn new() -> Live {
        LIVE.fetch_add(1, Ordering::SeqCst);
        Live
    }
}
impl Drop for Live {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Runs `f`, turning a panic into `E_FAIL`: a panic must never unwind into Explorer.
fn guarded<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(E_FAIL.into()))
}

#[implement(IClassFactory)]
struct Factory {
    _live: Live,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        guarded(|| {
            if ppv.is_null() {
                return Err(E_INVALIDARG.into());
            }
            unsafe { *ppv = std::ptr::null_mut() };
            if !outer.is_null() {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            let unknown: IUnknown = Handler::new().into();
            unsafe { unknown.query(riid, ppv).ok() }
        })
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct State {
    files: Vec<PathBuf>,
    target: Option<PathBuf>,
    /// Filled by `QueryContextMenu`, used by `InvokeCommand` (same order as the menu ids).
    entries: Vec<Entry>,
    exe: PathBuf,
}

#[implement(IShellExtInit, IContextMenu)]
struct Handler {
    state: Mutex<State>,
    _live: Live,
}

impl Handler {
    fn new() -> Handler {
        Handler { state: Mutex::new(State::default()), _live: Live::new() }
    }
}

/// The file system path of a folder's ID list, or None (a virtual folder such as a ZIP or the Recycle Bin).
fn folder_path(pidl: *const ITEMIDLIST) -> Option<PathBuf> {
    if pidl.is_null() {
        return None;
    }
    let mut buf = vec![0u16; 32_768];
    let ok = unsafe { SHGetPathFromIDListEx(pidl, &mut buf, GPFIDL_DEFAULT) }.as_bool();
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    (ok && len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

/// The dragged items as file system paths; None if any of them has no path (it is not a plain file or folder).
fn dragged_paths(data: &IDataObject) -> Option<Vec<PathBuf>> {
    let items: IShellItemArray = unsafe { SHCreateShellItemArrayFromDataObject(data) }.ok()?;
    let count = unsafe { items.GetCount() }.ok()?;
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let item = unsafe { items.GetItemAt(i) }.ok()?;
        let name: PWSTR = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }.ok()?;
        let path = unsafe { name.to_string() };
        unsafe { CoTaskMemFree(Some(name.0 as *const c_void)) };
        out.push(PathBuf::from(path.ok()?));
    }
    Some(out)
}

impl IShellExtInit_Impl for Handler_Impl {
    fn Initialize(&self, folder: *const ITEMIDLIST, data: Ref<IDataObject>, _progid: HKEY) -> Result<()> {
        guarded(|| {
            let mut s = self.state.lock().map_err(|_| E_FAIL)?;
            *s = State::default();
            s.target = folder_path(folder);
            if let Some(data) = data.as_ref() {
                s.files = dragged_paths(data).unwrap_or_default();
            }
            Ok(())
        })
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn insert_text(menu: HMENU, pos: u32, id: u32, text: &str, submenu: Option<HMENU>, bitmap: Option<HBITMAP>) -> Result<()> {
    let mut text = wide(text);
    let mut info = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_FTYPE | MIIM_ID | MIIM_STRING,
        fType: MFT_STRING,
        wID: id,
        dwTypeData: PWSTR(text.as_mut_ptr()),
        cch: (text.len() - 1) as u32,
        ..Default::default()
    };
    if let Some(sub) = submenu {
        info.fMask |= MIIM_SUBMENU;
        info.hSubMenu = sub;
    }
    if let Some(bitmap) = bitmap {
        info.fMask |= MIIM_BITMAP;
        info.hbmpItem = bitmap;
    }
    unsafe { InsertMenuItemW(menu, pos, true, &info) }
}

fn insert_separator(menu: HMENU, pos: u32) -> Result<()> {
    let info = MENUITEMINFOW { cbSize: size_of::<MENUITEMINFOW>() as u32, fMask: MIIM_FTYPE, fType: MFT_SEPARATOR, ..Default::default() };
    unsafe { InsertMenuItemW(menu, pos, true, &info) }
}

impl IContextMenu_Impl for Handler_Impl {
    fn QueryContextMenu(&self, menu: HMENU, index: u32, first: u32, last: u32, flags: u32) -> HRESULT {
        // "Nothing added" is HRESULT 0; added items are counted in the code part of a success HRESULT.
        let added = guarded(|| {
            if flags & CMF_DEFAULTONLY != 0 {
                return Ok(0);
            }
            let mut s = self.state.lock().map_err(|_| E_FAIL)?;
            s.entries.clear();
            if s.target.is_none() || s.files.is_empty() {
                return Ok(0);
            }
            let Some(cfg) = config::path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| Config::parse(&t)) else { return Ok(0) };
            let entries = cfg.entries_for(&s.files);
            // ids first..first+n-1 for the entries, first+n for the submenu item itself
            if entries.is_empty() || first.saturating_add(entries.len() as u32) > last {
                return Ok(0);
            }
            let sub = unsafe { CreatePopupMenu() }?;
            let mut pos = 0;
            let filled = (|| -> Result<()> {
                for (n, e) in entries.iter().enumerate() {
                    if e.separator {
                        insert_separator(sub, pos)?;
                        pos += 1;
                    }
                    insert_text(sub, pos, first + n as u32, &e.label, None, None)?;
                    pos += 1;
                }
                insert_text(menu, index, first + entries.len() as u32, &cfg.title, Some(sub), crate::icon::menu_bitmap(&cfg.exe))
            })();
            if let Err(e) = filled {
                // not attached to Explorer's menu: ours to free
                let _ = unsafe { DestroyMenu(sub) };
                return Err(e);
            }
            let n = entries.len() as u32 + 1;
            s.entries = entries;
            s.exe = cfg.exe;
            Ok(n)
        });
        HRESULT(added.unwrap_or(0) as i32)
    }

    fn InvokeCommand(&self, info: *const CMINVOKECOMMANDINFO) -> Result<()> {
        guarded(|| {
            if info.is_null() {
                return Err(E_INVALIDARG.into());
            }
            let verb = unsafe { (*info).lpVerb.0 } as usize;
            // A verb given as text is not ours (we have none); an offset has its high word at zero.
            if verb >> 16 != 0 {
                return Err(E_INVALIDARG.into());
            }
            let s = self.state.lock().map_err(|_| E_FAIL)?;
            let (Some(entry), Some(target)) = (s.entries.get(verb & 0xFFFF), s.target.as_ref()) else { return Err(E_INVALIDARG.into()) };
            launch(&s.exe, &entry.action, target, &s.files).map_err(|_| E_FAIL.into())
        })
    }

    fn GetCommandString(&self, _id: usize, _kind: u32, _reserved: *const u32, _name: PSTR, _max: u32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
}

/// `voidflare.exe --run <action> --to <folder> --list <file>`, the list in `%TEMP%\voidflare-menu`.
///
/// Started with `CreateProcessW` and no handle inheritance: `std::process::Command` lets the child inherit every
/// inheritable handle of the host (Explorer), which voidflare.exe would then hold for as long as its window is
/// open. Seen with the test host: its output pipe stayed open until the window was closed.
fn launch(exe: &std::path::Path, action: &str, target: &std::path::Path, files: &[PathBuf]) -> std::io::Result<()> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{CreateProcessW, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW};
    use windows::core::{HSTRING, PCWSTR};

    let list = crate::write_list(&std::env::temp_dir().join("voidflare-menu"), files)?;
    let args = [
        exe.to_string_lossy().into_owned(),
        "--run".into(),
        action.into(),
        "--to".into(),
        target.to_string_lossy().into_owned(),
        "--list".into(),
        list.to_string_lossy().into_owned(),
    ];
    let mut line: Vec<u16> = crate::command_line(&args).encode_utf16().chain(std::iter::once(0)).collect();
    let app = HSTRING::from(exe.as_os_str());
    let dir = exe.parent().map(|d| HSTRING::from(d.as_os_str()));
    let si = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut pi = PROCESS_INFORMATION::default();
    let started = unsafe {
        CreateProcessW(
            &app,
            Some(PWSTR(line.as_mut_ptr())),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS(0),
            None,
            dir.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
            &si,
            &mut pi,
        )
    };
    match started {
        Ok(()) => {
            unsafe {
                let _ = CloseHandle(pi.hThread);
                let _ = CloseHandle(pi.hProcess);
            }
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&list);
            Err(std::io::Error::other(e))
        }
    }
}

/// # Safety
/// Called by COM with valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(clsid: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> HRESULT {
    guarded(|| {
        if clsid.is_null() || riid.is_null() || ppv.is_null() {
            return Err(E_INVALIDARG.into());
        }
        unsafe { *ppv = std::ptr::null_mut() };
        if unsafe { *clsid } != CLSID {
            return Err(CLASS_E_CLASSNOTAVAILABLE.into());
        }
        let factory: IClassFactory = Factory { _live: Live::new() }.into();
        unsafe { factory.query(riid, ppv).ok() }
    })
    .map_or_else(|e| e.code(), |()| S_OK)
}

#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if LIVE.load(Ordering::SeqCst) == 0 { S_OK } else { S_FALSE }
}
