//! The handler driven as Explorer drives it (COM factory, a real shell data object, a real popup menu), in this
//! process. One test function: it points APPDATA at a scratch folder, which is process-wide.
#![cfg(windows)]

#[path = "../testutil/host_lib.rs"]
mod host_lib;

use std::fs;
use std::path::PathBuf;

use voidflare_dragdrop::config::{CLSID_STR, Config, Entry};

fn labels(items: &[host_lib::Item]) -> Vec<String> {
    items.iter().map(|i| i.label.clone()).collect()
}

#[test]
fn explorer_sees_the_entries_that_fit_every_dragged_file() {
    assert_eq!(format!("{{{:?}}}", voidflare_dragdrop::CLSID), CLSID_STR, "registry id and COM id agree");

    let base = std::env::temp_dir().join(format!("vf_dragdrop_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let (src, drop_on, appdata) = (base.join("src città"), base.join("drop here"), base.join("appdata"));
    for d in [&src, &drop_on, &appdata.join("voidflare")] {
        fs::create_dir_all(d).unwrap();
    }
    let file = |n: &str| {
        let p = src.join(n);
        fs::write(&p, b"x").unwrap();
        p
    };
    let (docx, xlsx, zip) = (file("Report.docx"), file("Budget.xlsx"), file("Old.zip"));

    let e = |a: &str, s: bool, l: &str| Entry { action: a.into(), separator: s, label: l.into() };
    let cfg = Config {
        exe: base.join("no such voidflare.exe"),
        title: "Convert here with voidflare".into(),
        types: vec![
            ("docx".into(), vec![e("route:pdf", false, "PDF"), e("route:txt", false, "Text"), e("recipe:Report pack", true, "Recipe: Report pack")]),
            ("xlsx".into(), vec![e("route:pdf", false, "PDF"), e("recipe:Report pack", true, "Recipe: Report pack")]),
        ],
    };
    fs::write(appdata.join("voidflare").join("dragmenu.tsv"), cfg.write()).unwrap();
    // SAFETY: the only test in this binary; nothing else reads the environment meanwhile.
    unsafe { std::env::set_var("APPDATA", &appdata) };

    let get: host_lib::GetClassObject = voidflare_dragdrop::DllGetClassObject;

    // One Word file: one submenu with its three entries and a line before the recipe.
    let (hr, menu) = host_lib::drive(get, &drop_on, &[docx.clone()], None).unwrap();
    assert_eq!(hr.0, 4, "3 entries + the submenu item");
    assert_eq!(labels(&menu), ["Convert here with voidflare"]);
    assert_eq!(labels(&menu[0].children), ["PDF", "Text", "", "Recipe: Report pack"]);
    assert_eq!(menu[0].children.iter().filter_map(|c| c.offset).collect::<Vec<_>>(), [0, 1, 2]);
    // voidflare.exe missing: no icon, the text alone (and no failure cached for later).
    assert!(!menu[0].bitmap);

    // With an exe that has an icon: the submenu item shows it, the entries inside do not.
    let mut with_icon = Config::parse(&cfg.write()).unwrap();
    with_icon.exe = std::path::Path::new(&std::env::var_os("WINDIR").unwrap()).join("explorer.exe");
    fs::write(appdata.join("voidflare").join("dragmenu.tsv"), with_icon.write()).unwrap();
    let (_, menu) = host_lib::drive(get, &drop_on, &[docx.clone()], None).unwrap();
    assert!(menu[0].bitmap, "icon on «Convert here with voidflare»");
    assert!(menu[0].children.iter().all(|c| !c.bitmap));
    fs::write(appdata.join("voidflare").join("dragmenu.tsv"), cfg.write()).unwrap();

    // Word + Excel: only what both have.
    let (_, menu) = host_lib::drive(get, &drop_on, &[docx.clone(), xlsx.clone()], None).unwrap();
    assert_eq!(labels(&menu[0].children), ["PDF", "", "Recipe: Report pack"]);

    // A type voidflare does not read, or a folder, among them: nothing at all.
    let (hr, menu) = host_lib::drive(get, &drop_on, &[docx.clone(), zip], None).unwrap();
    assert_eq!((hr.0, menu.len()), (0, 0));
    let (hr, _) = host_lib::drive(get, &drop_on, &[src.clone()], None).unwrap();
    assert_eq!(hr.0, 0);

    // No menu file (the menu was removed): nothing.
    fs::rename(appdata.join("voidflare").join("dragmenu.tsv"), appdata.join("voidflare").join("off.tsv")).unwrap();
    let (hr, _) = host_lib::drive(get, &drop_on, &[docx.clone()], None).unwrap();
    assert_eq!(hr.0, 0);
    fs::rename(appdata.join("voidflare").join("off.tsv"), appdata.join("voidflare").join("dragmenu.tsv")).unwrap();

    // A click with voidflare.exe missing: an error for Explorer, and the file list is not left behind.
    let lists = || -> Vec<PathBuf> {
        fs::read_dir(std::env::temp_dir().join("voidflare-menu"))
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(&format!("drop-{}-", std::process::id()))).collect())
            .unwrap_or_default()
    };
    assert!(host_lib::drive(get, &drop_on, &[docx], Some(0)).is_err());
    assert!(lists().is_empty(), "{:?}", lists());

    // Every object was released: Explorer may unload the DLL.
    assert_eq!(voidflare_dragdrop::DllCanUnloadNow().0, 0, "S_OK");
    let _ = fs::remove_dir_all(&base);
}
