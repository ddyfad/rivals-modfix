//! rivals-modfix.exe: see README.md. The logic lives in lib.rs.
use rivals_modfix::*;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const OODLE_URL: &str = "https://github.com/WorkingRobot/OodleUE/raw/refs/heads/main/Engine/Source/Programs/Shared/EpicGames.Oodle/Sdk/2.9.10/win/redist/oo2core_9_win64.dll";

struct Oodle { _lib: libloading::Library, decompress: unsafe extern "C" fn(*const u8, isize, *mut u8, isize, i32, i32, i32, *mut u8, isize, *mut u8, *mut u8, *mut u8, isize, i32) -> isize }
impl Oodle {
    fn load(dir: &Path) -> Result<Oodle, String> {
        let name = if cfg!(windows) { "oo2core_9_win64.dll" } else { "liboo2corelinux64.so.9" };
        let mut candidates = vec![dir.join(name), PathBuf::from(name)];
        if !cfg!(windows) { if let Ok(h) = std::env::var("HOME") { candidates.push(PathBuf::from(h).join(".cargo/bin").join(name)); } }
        let mut found = candidates.iter().find(|p| p.exists()).cloned();
        if found.is_none() && cfg!(windows) {
            let target = dir.join(name);
            println!("Downloading {name} (the game's compression library) ...");
            let st = std::process::Command::new("curl.exe").args(["-L", "-s", "-o"]).arg(&target).arg(OODLE_URL).status();
            if st.map(|s| s.success()).unwrap_or(false) && target.exists() { found = Some(target); }
        }
        let path = found.ok_or(format!("{name} not found next to this program and could not be downloaded; put it there and run again"))?;
        unsafe {
            let lib = libloading::Library::new(&path).map_err(|e| e.to_string())?;
            let f = *lib.get::<unsafe extern "C" fn(*const u8, isize, *mut u8, isize, i32, i32, i32, *mut u8, isize, *mut u8, *mut u8, *mut u8, isize, i32) -> isize>(b"OodleLZ_Decompress\0").map_err(|e| e.to_string())?;
            Ok(Oodle { _lib: lib, decompress: f })
        }
    }
    fn decompress(&self, comp: &[u8], raw_len: usize) -> Result<Vec<u8>, String> {
        let mut out = vec![0u8; raw_len];
        let r = unsafe { (self.decompress)(comp.as_ptr(), comp.len() as isize, out.as_mut_ptr(), raw_len as isize, 1, 0, 0, std::ptr::null_mut(), 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0, 3) };
        if r as usize != raw_len { return Err("Oodle decompression failed".into()); }
        Ok(out)
    }
}

fn find_mods_dir(arg: Option<String>) -> Option<PathBuf> {
    if let Some(a) = arg {
        let p = PathBuf::from(a);
        if p.join("~mods").is_dir() { return Some(p.join("~mods")); }
        if p.is_dir() { return Some(p); }
    }
    let tail = "steamapps/common/MarvelRivals/MarvelGame/Marvel/Content/Paks/~mods";
    let mut roots = vec![PathBuf::from(r"C:\Program Files (x86)\Steam")];
    if let Ok(vdf) = fs::read_to_string(r"C:\Program Files (x86)\Steam\steamapps\libraryfolders.vdf") {
        for line in vdf.lines() {
            let l = line.trim();
            if l.starts_with("\"path\"") { if let Some(v) = l.split('"').nth(3) { roots.push(PathBuf::from(v.replace("\\\\", "\\"))); } }
        }
    }
    roots.into_iter().map(|r| r.join(tail)).find(|p| p.is_dir())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let check = args.iter().any(|a| a == "--check");
    let pause = !args.iter().any(|a| a == "--no-pause") && cfg!(windows);
    let dir_arg = args.iter().find(|a| !a.starts_with("--")).cloned();
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or(PathBuf::from("."));
    println!("rivals-modfix: ports mods across the Oct 30 2026 Marvel Rivals update\n");
    let Some(mods) = find_mods_dir(dir_arg) else {
        println!("Could not find the ~mods folder. Run this with the path to it, e.g.\n  rivals-modfix.exe \"C:\\Program Files (x86)\\Steam\\steamapps\\common\\MarvelRivals\\MarvelGame\\Marvel\\Content\\Paks\\~mods\"");
        finish(pause); return;
    };
    println!("Mods folder: {}\n", mods.display());
    let backup = mods.parent().unwrap_or(&mods).join("modfix-backup");
    let mut oodle: Option<Oodle> = None;
    let mut utocs: Vec<PathBuf> = fs::read_dir(&mods).map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().map(|x| x.eq_ignore_ascii_case("utoc")).unwrap_or(false)).collect()).unwrap_or_default();
    utocs.sort();
    if utocs.is_empty() { println!("No .utoc mods found."); finish(pause); return; }
    let (mut n_fixed, mut n_ok, mut n_skip) = (0, 0, 0);
    for utoc in &utocs {
        let name = utoc.file_stem().unwrap().to_string_lossy().to_string();
        let ucas_path = utoc.with_extension("ucas");
        let toc = match read_toc(utoc) { Ok(t) => t, Err(e) => { println!("SKIP  {name}: {e}"); n_skip += 1; continue; } };
        let Ok(ucas) = fs::read(&ucas_path) else { println!("SKIP  {name}: no .ucas next to it"); n_skip += 1; continue; };
        // A mod file dated before the update is assumed old when a material reads cleanly both ways.
        let assume_old = fs::metadata(utoc).and_then(|m| m.modified()).map(|t| t < std::time::UNIX_EPOCH + std::time::Duration::from_secs(UPDATE_TIME)).unwrap_or(false);
        let mut dec = |block: &[u8], raw_len: usize| -> Result<Vec<u8>, String> {
            if oodle.is_none() { oodle = Some(Oodle::load(&exe_dir)?); }
            oodle.as_ref().unwrap().decompress(block, raw_len)
        };
        let res = match process_mod(&toc, &ucas, assume_old, check, &mut dec) { Ok(r) => r, Err(e) => { println!("SKIP  {name}: {e}"); n_skip += 1; continue; } };
        let (fixed, materials) = (res.fixed, res.materials);
        if !res.problems.is_empty() { println!("SKIP  {name}: {} material(s) could not be read safely ({})", res.problems.len(), res.problems[0]); n_skip += 1; continue; }
        let encrypted = toc.flags & 2 != 0;
        if fixed == 0 && !encrypted { println!("ok    {name}: {}", if materials == 0 { "no material instances, nothing to do".to_string() } else { format!("{materials} material instance(s) already in the current layout") }); n_ok += 1; continue; }
        if check {
            if fixed == 0 { println!("ENC   {name}: encrypted, would be decrypted"); }
            else { println!("STALE {name}: {fixed} of {materials} material instance(s) still in the old layout{}", if encrypted { ", encrypted" } else { "" }); }
            n_fixed += 1; continue;
        }
        let mut failed = false;
        let _ = fs::create_dir_all(&backup);
        for ext in ["utoc", "ucas", "pak"] {
            let src = utoc.with_extension(ext);
            if src.exists() { if let Err(e) = fs::copy(&src, backup.join(src.file_name().unwrap())) { println!("SKIP  {name}: backup failed: {e}"); failed = true; break; } }
        }
        if failed { n_skip += 1; continue; }
        match fs::write(utoc, &res.utoc).and_then(|_| fs::write(&ucas_path, &res.ucas)).map_err(|e| e.to_string()) {
            Ok(()) => {
                if fixed == 0 { println!("FIXED {name}: decrypted"); }
                else { println!("FIXED {name}: {fixed} of {materials} material instance(s) ported to the new layout{}", if encrypted { ", decrypted" } else { "" }); }
                n_fixed += 1;
            }
            Err(e) => {
                println!("SKIP  {name}: writing failed ({e}); restoring the original");
                for ext in ["utoc", "ucas"] { let _ = fs::copy(backup.join(format!("{name}.{ext}")), utoc.with_extension(ext)); }
                n_skip += 1;
            }
        }
    }
    println!("\n{} fixed, {} already fine, {} skipped.", n_fixed, n_ok, n_skip);
    if n_fixed > 0 && !check { println!("Originals are in {}. Delete that folder once the game runs.", backup.display()); }
    finish(pause);
}

fn finish(pause: bool) {
    if pause { print!("\nPress Enter to close."); let _ = std::io::stdout().flush(); let mut s = String::new(); let _ = std::io::stdin().read_line(&mut s); }
}
