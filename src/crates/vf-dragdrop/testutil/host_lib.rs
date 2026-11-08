//! Drives a drag-and-drop handler the way File Explorer does: class factory → `IShellExtInit::Initialize` with the
//! drop folder and a data object of the dragged files → `IContextMenu::QueryContextMenu` on a real popup menu →
//! optionally `InvokeCommand`. Shared by `examples/host.rs` (the built DLL) and `tests/handler.rs` (in-process); kept outside `examples\` and
//! `tests\` so cargo does not take it for a target of its own.
#![allow(dead_code)]

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::E_FAIL;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, IClassFactory, IDataObject};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{BHID_DataObject, CMINVOKECOMMANDINFO, IContextMenu, IShellExtInit, SHCreateShellItemArrayFromIDLists, SHParseDisplayName};
use windows::Win32::UI::WindowsAndMessaging::{CreatePopupMenu, DestroyMenu, GetMenuItemCount, GetMenuItemID, GetMenuStringW, GetSubMenu, MF_BYPOSITION};
use windows::core::{GUID, HRESULT, HSTRING, Interface, PCSTR, Result};

pub type GetClassObject = unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;

pub const CLSID: GUID = GUID::from_u128(0x127b4c5d_6b1d_489c_a1d9_1abf98d0c829);

struct Pidl(*mut ITEMIDLIST);
impl Drop for Pidl {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0 as *const c_void)) };
    }
}

fn pidl(path: &Path) -> Result<Pidl> {
    let mut p = std::ptr::null_mut();
    unsafe { SHParseDisplayName(&HSTRING::from(path.as_os_str()), None, &mut p, 0, None) }?;
    Ok(Pidl(p))
}

/// One menu item: (label, command offset from `first`), the submenu as its children.
#[derive(Debug)]
pub struct Item {
    pub label: String,
    pub offset: Option<u32>,
    pub children: Vec<Item>,
    /// The item carries a bitmap (the voidflare icon).
    pub bitmap: bool,
}

fn has_bitmap(menu: windows::Win32::UI::WindowsAndMessaging::HMENU, i: u32) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetMenuItemInfoW, MENUITEMINFOW, MIIM_BITMAP};
    let mut info = MENUITEMINFOW { cbSize: size_of::<MENUITEMINFOW>() as u32, fMask: MIIM_BITMAP, ..Default::default() };
    unsafe { GetMenuItemInfoW(menu, i, true, &mut info) }.is_ok() && !info.hbmpItem.is_invalid()
}

fn read_menu(menu: windows::Win32::UI::WindowsAndMessaging::HMENU, first: u32) -> Vec<Item> {
    let n = unsafe { GetMenuItemCount(Some(menu)) };
    (0..n)
        .map(|i| {
            let mut buf = [0u16; 512];
            let len = unsafe { GetMenuStringW(menu, i as u32, Some(&mut buf), MF_BYPOSITION) };
            let label = String::from_utf16_lossy(&buf[..len.max(0) as usize]);
            let sub = unsafe { GetSubMenu(menu, i) };
            let id = unsafe { GetMenuItemID(menu, i) };
            Item {
                label,
                offset: (id != u32::MAX && id >= first).then(|| id - first),
                children: if sub.0.is_null() { Vec::new() } else { read_menu(sub, first) },
                bitmap: has_bitmap(menu, i as u32),
            }
        })
        .collect()
}

/// What Explorer would show for `files` dropped on `target`, and (with `invoke`) clicks the entry at that offset.
pub fn drive(get: GetClassObject, target: &Path, files: &[PathBuf], invoke: Option<u32>) -> Result<(HRESULT, Vec<Item>)> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let mut factory: *mut c_void = std::ptr::null_mut();
        get(&CLSID, &IClassFactory::IID, &mut factory).ok()?;
        let factory = IClassFactory::from_raw(factory);
        let init: IShellExtInit = factory.CreateInstance(None)?;
        let menu_iface: IContextMenu = init.cast()?;

        let folder = pidl(target)?;
        let items: Vec<Pidl> = files.iter().map(|f| pidl(f)).collect::<Result<_>>()?;
        let raw: Vec<*const ITEMIDLIST> = items.iter().map(|p| p.0 as *const ITEMIDLIST).collect();
        let array = SHCreateShellItemArrayFromIDLists(&raw)?;
        let data: IDataObject = array.BindToHandler(None, &BHID_DataObject)?;
        init.Initialize(Some(folder.0 as *const ITEMIDLIST), &data, None)?;

        let menu = CreatePopupMenu()?;
        let first = 100;
        let hr = menu_iface.QueryContextMenu(menu, 0, first, first + 0x7FFF, 0);
        let read = read_menu(menu, first);
        if let Some(offset) = invoke {
            let info = CMINVOKECOMMANDINFO { cbSize: size_of::<CMINVOKECOMMANDINFO>() as u32, lpVerb: PCSTR(offset as usize as *const u8), nShow: 1, ..Default::default() };
            menu_iface.InvokeCommand(&info)?;
        }
        DestroyMenu(menu).map_err(|_| windows::core::Error::from(E_FAIL))?;
        Ok((hr, read))
    }
}
