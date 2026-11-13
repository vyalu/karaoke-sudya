//! The «Пародист» sound library: three category folders (Музыка, Мемы, Животные).
//! Sounds lie in a category folder directly («Общее») or in packs (subfolders).
//! The file name is the phrase.
//! A pack can be shared by sending its folder; dropping a folder in adds it as a pack.

use std::path::{Path, PathBuf};

pub const EXT: [&str; 6] = ["mp3", "wav", "ogg", "flac", "m4a", "aac"];

#[derive(Clone, Debug)]
pub struct Sound { pub path: PathBuf, pub name: String, pub cat: usize }

fn is_sound(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| EXT.contains(&e.to_lowercase().as_str())).unwrap_or(false)
}

/// The phrase from a file name.
fn describe(p: &Path) -> String {
    p.file_stem().and_then(|s| s.to_str()).unwrap_or("").replace('_', " ").trim().to_string()
}

/// Everything in the folder, sorted by name.
pub fn list(dir: &Path) -> Vec<Sound> {
    let mut v: Vec<Sound> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && is_sound(p)).map(|p| {
        let name = describe(&p);
        Sound { path: p, name, cat: 0 }
    }).collect()).unwrap_or_default();
    v.sort_by_key(|s| s.name.to_lowercase());
    v
}

/// A name that is safe for a Windows file.
fn clean(name: &str) -> String {
    let n: String = name.chars().filter(|c| !"\\/:*?\"<>|\n\r\t".contains(*c)).take(90).collect();
    let n = n.trim().trim_end_matches(['.', ' ']).to_string();
    if n.is_empty() { "Без названия".into() } else { n }
}

/// A free path in `dir` for this name and extension.
pub fn free_path(dir: &Path, base: &str, ext: &str, except: Option<&Path>) -> PathBuf {
    let mut p = dir.join(format!("{base}.{ext}"));
    let mut k = 2;
    while p.exists() && Some(p.as_path()) != except { p = dir.join(format!("{base} ({k}).{ext}")); k += 1; }
    p
}

/// Copy a file the user picked into the library. Returns the new path.
pub fn add(dir: &Path, src: &Path) -> Result<PathBuf, String> {
    if !is_sound(src) { return Err(format!("«{}» — не звуковой файл (нужен mp3, wav, ogg, flac или m4a)", src.file_name().and_then(|n| n.to_str()).unwrap_or("?"))); }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("mp3").to_lowercase();
    let base = clean(&describe(src));
    let dst = free_path(dir, &base, &ext, None);
    std::fs::copy(src, &dst).map_err(|e| format!("Не удалось скопировать: {e}"))?;
    Ok(dst)
}

/// Rename (new phrase text). Returns the new path.
pub fn rename(path: &Path, name: &str) -> Result<PathBuf, String> {
    let dir = path.parent().ok_or("Нет папки")?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("mp3");
    let dst = free_path(dir, &clean(name), ext, Some(path));
    if dst != path { std::fs::rename(path, &dst).map_err(|e| format!("Не удалось переименовать: {e}"))?; }
    Ok(dst)
}

/// Save the `a..b` seconds of a decoded sound as WAV: either in place of the file
/// (an mp3 becomes a wav with the same name) or next to it as «… (обрезка)».
pub fn save_trimmed(src: &Path, x: &[f32], sr: u32, a: f32, b: f32, overwrite: bool) -> Result<PathBuf, String> {
    let dir = src.parent().ok_or("Нет папки")?;
    let i0 = ((a.max(0.0) * sr as f32) as usize).min(x.len());
    let i1 = ((b * sr as f32) as usize).min(x.len()).max(i0);
    if i1 - i0 < (sr as usize) / 10 { return Err("Слишком короткий отрезок".into()); }
    let name = clean(&describe(src));
    let is_wav = src.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("wav")).unwrap_or(false);
    let dst = if overwrite {
        if is_wav { src.to_path_buf() } else { free_path(dir, &name, "wav", None) }
    } else {
        free_path(dir, &format!("{name} (обрезка)"), "wav", None)
    };
    crate::audio::write_wav(&dst, &x[i0..i1], sr)?;
    if overwrite && !is_wav { let _ = std::fs::remove_file(src); }
    Ok(dst)
}

pub fn remove(path: &Path) -> Result<(), String> { std::fs::remove_file(path).map_err(|e| format!("Не удалось удалить: {e}")) }

/* ---------- categories and packs ---------- */

pub const CATEGORIES: [&str; 3] = ["Музыка", "Мемы", "Животные"];
/// Name shown for the sounds lying in a category folder itself.
pub const LOOSE: &str = "Общее";

#[derive(Clone, Debug)]
pub struct Pack { pub path: PathBuf, pub name: String, pub sounds: Vec<Sound>, pub cat: usize, pub loose: bool }

pub fn cat_dir(root: &Path, cat: usize) -> PathBuf { root.join(CATEGORIES[cat.min(CATEGORIES.len() - 1)]) }

/// All packs of all categories. Creates the category folders; anything left in the
/// root by older versions moves into «Мемы».
pub fn packs(root: &Path) -> Vec<Pack> {
    for c in 0..CATEGORIES.len() { let _ = std::fs::create_dir_all(cat_dir(root, c)); }
    let memes = cat_dir(root, 1);
    for p in std::fs::read_dir(root).map(|rd| rd.flatten().map(|e| e.path()).collect::<Vec<_>>()).unwrap_or_default() {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        if p.is_dir() && !CATEGORIES.contains(&name.as_str()) {
            let _ = std::fs::rename(&p, free_dir(&memes, &name, None));
        } else if p.is_file() && is_sound(&p) {
            let _ = std::fs::rename(&p, memes.join(&name));
        }
    }
    let mut out = Vec::new();
    for c in 0..CATEGORIES.len() {
        let d = cat_dir(root, c);
        let mut loose = list(&d);
        for s in &mut loose { s.cat = c; }
        out.push(Pack { path: d.clone(), name: LOOSE.into(), sounds: loose, cat: c, loose: true });
        let mut sub: Vec<Pack> = std::fs::read_dir(&d).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).map(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            let mut sounds = list(&p);
            for s in &mut sounds { s.cat = c; }
            Pack { sounds, path: p, name, cat: c, loose: false }
        }).collect()).unwrap_or_default();
        sub.sort_by_key(|p| p.name.to_lowercase());
        out.extend(sub);
    }
    out
}

fn free_dir(root: &Path, base: &str, except: Option<&Path>) -> PathBuf {
    let mut p = root.join(base);
    let mut k = 2;
    while p.exists() && Some(p.as_path()) != except { p = root.join(format!("{base} ({k})")); k += 1; }
    p
}

pub fn create_pack(root: &Path, name: &str) -> Result<PathBuf, String> {
    let d = free_dir(root, &clean(name), None);
    std::fs::create_dir_all(&d).map_err(|e| format!("Не удалось создать набор: {e}"))?;
    Ok(d)
}

pub fn rename_pack(path: &Path, name: &str) -> Result<PathBuf, String> {
    let root = path.parent().ok_or("Нет папки")?;
    let d = free_dir(root, &clean(name), Some(path));
    if d != path { std::fs::rename(path, &d).map_err(|e| format!("Не удалось переименовать набор: {e}"))?; }
    Ok(d)
}

pub fn delete_pack(path: &Path) -> Result<(), String> { std::fs::remove_dir_all(path).map_err(|e| format!("Не удалось удалить набор: {e}")) }

/// Copy a whole folder of sounds in as a new pack (named after the folder).
/// Returns the pack path and how many sounds were copied.
pub fn import_folder(root: &Path, src: &Path) -> Result<(PathBuf, usize), String> {
    let files: Vec<PathBuf> = std::fs::read_dir(src).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| p.is_file() && is_sound(p)).collect();
    if files.is_empty() { return Err("В папке нет звуков (mp3, wav, ogg, flac, m4a)".into()); }
    let name = src.file_name().and_then(|n| n.to_str()).unwrap_or("Новый набор");
    let d = create_pack(root, name)?;
    let n = files.iter().filter(|f| add(&d, f).is_ok()).count();
    Ok((d, n))
}

#[cfg(test)]
mod tests {
    #[test]
    fn library_roundtrip() {
        let d = std::env::temp_dir().join("sounds_lib_test");
        let _ = std::fs::remove_dir_all(&d);
        let src = std::env::temp_dir().join("Это_фиаско, братан.mp3");
        std::fs::write(&src, b"x").unwrap();
        let p = super::add(&d, &src).unwrap();
        let p2 = super::add(&d, &src).unwrap();
        assert_ne!(p, p2);
        let v = super::list(&d);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "Это фиаско, братан");
        let r = super::rename(&p, "Вы кто такие?").unwrap();
        let v = super::list(&d);
        let s = v.iter().find(|s| s.path == r).unwrap();
        assert!(s.name == "Вы кто такие", "{s:?}");
        super::remove(&p2).unwrap();
        assert_eq!(super::list(&d).len(), 1);
        assert!(super::add(&d, std::path::Path::new("notes.txt")).is_err());
    }

    #[test]
    fn packs_work() {
        let root = std::env::temp_dir().join("sounds_packs_test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Старый набор")).unwrap();
        std::fs::write(root.join("Старый набор/x.mp3"), b"x").unwrap();
        std::fs::write(root.join("loose.mp3"), b"x").unwrap();
        let v = super::packs(&root);
        // 3 loose «Общее» + the old pack, moved into «Мемы»
        assert_eq!(v.len(), 4);
        assert!(v.iter().any(|p| p.cat == 1 && p.loose && p.sounds.len() == 1));
        assert!(v.iter().any(|p| p.cat == 1 && p.name == "Старый набор" && p.sounds[0].cat == 1));
        // a file dropped straight into a category folder shows up
        std::fs::write(root.join("Животные/Мяу.wav"), b"x").unwrap();
        let v = super::packs(&root);
        assert!(v.iter().any(|p| p.cat == 2 && p.loose && p.sounds[0].name == "Мяу"));
        let src = std::env::temp_dir().join("Мемы с работы");
        let _ = std::fs::remove_dir_all(&src);
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("а.mp3"), b"x").unwrap();
        std::fs::write(src.join("б.wav"), b"x").unwrap();
        std::fs::write(src.join("readme.txt"), b"x").unwrap();
        let (p, n) = super::import_folder(&super::cat_dir(&root, 0), &src).unwrap();
        assert_eq!(n, 2);
        let p = super::rename_pack(&p, "Офис").unwrap();
        let v = super::packs(&root);
        assert!(v.iter().any(|x| x.name == "Офис" && x.cat == 0 && x.sounds.len() == 2));
        super::delete_pack(&p).unwrap();
        assert_eq!(super::packs(&root).len(), 4);
    }

    #[test]
    fn trim_saves_both_ways() {
        let d = std::env::temp_dir().join("sounds_trim_test");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let src = d.join("Мяу.mp3");
        std::fs::write(&src, b"x").unwrap();
        let x: Vec<f32> = (0..16000 * 3).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let n = super::save_trimmed(&src, &x, 16000, 0.5, 2.0, false).unwrap();
        assert!(src.exists() && n.file_name().unwrap().to_str().unwrap() == "Мяу (обрезка).wav");
        let (s, _) = crate::audio::load_mono(&n, 20.0).unwrap();
        assert!((s.len() as i64 - 24000).abs() < 10);
        let o = super::save_trimmed(&src, &x, 16000, 0.0, 1.0, true).unwrap();
        assert!(!src.exists() && o.file_name().unwrap().to_str().unwrap() == "Мяу.wav");
    }
}
