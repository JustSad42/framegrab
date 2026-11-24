//! The last 20 captures, kept even when saving is off: a small index plus thumbnails in
//! %LOCALAPPDATA%\Framegrab\history. Saved PNGs are referenced in place; anything else gets a
//! private PNG copy (so Copy / Pin / Edit work for JPG/WebP/AVIF and clipboard-only captures).
//! Only the resident app writes it; Settings reads it and asks the app to remove entries.

use crate::capture::Image;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MAX: usize = 20;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Entry {
    pub id: u64,
    /// A PNG of the capture (the saved file or a private copy).
    pub image: String,
    /// The saved file, if saving was on (may be JPG/WebP/AVIF).
    pub saved: Option<String>,
    /// True when `image` is our own copy (deleted with the entry).
    pub owned: bool,
    pub thumb: String,
    pub w: u32,
    pub h: u32,
    pub time: String,
}

pub fn dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let d = base.join(if crate::config::test_mode() { "Framegrab-test" } else { "Framegrab" }).join("history");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn index_path() -> PathBuf {
    dir().join("index.json")
}

pub fn load() -> Vec<Entry> {
    std::fs::read_to_string(index_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn store(v: &[Entry]) {
    let p = index_path();
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(v).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

fn delete_files(e: &Entry) {
    if e.owned {
        let _ = std::fs::remove_file(&e.image);
    }
    let _ = std::fs::remove_file(&e.thumb);
}

/// Records a finished capture. `saved` is the file written to disk, if any. Worker thread.
pub fn add(img: &Image, saved: Option<&Path>) {
    let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let d = dir();
    let png_saved = saved.filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")));
    let (image, owned) = match png_saved {
        Some(p) => (p.to_path_buf(), false),
        None => {
            let p = d.join(format!("{id}.png"));
            if crate::output::write_png(&p, img, true).is_err() {
                return;
            }
            (p, true)
        }
    };
    let thumb = d.join(format!("{id}-thumb.png"));
    let _ = crate::output::write_png(&thumb, &img.fit(360, 220), true);
    let e = Entry {
        id,
        image: image.display().to_string(),
        saved: saved.map(|p| p.display().to_string()),
        owned,
        thumb: thumb.display().to_string(),
        w: img.w,
        h: img.h,
        time: crate::config::stamp(),
    };
    let mut v = load();
    v.insert(0, e);
    while v.len() > MAX {
        if let Some(old) = v.pop() {
            delete_files(&old);
        }
    }
    store(&v);
}

pub fn remove(id: u64) {
    let mut v = load();
    if let Some(i) = v.iter().position(|e| e.id == id) {
        let e = v.remove(i);
        delete_files(&e);
        store(&v);
    }
}

pub fn clear() {
    for e in load() {
        delete_files(&e);
    }
    store(&[]);
}

/// The entry with `id`, if its image file still exists.
pub fn find(id: u64) -> Option<Entry> {
    load().into_iter().find(|e| e.id == id && Path::new(&e.image).exists())
}
