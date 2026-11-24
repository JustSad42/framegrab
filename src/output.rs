//! Saving PNGs and putting images / text on the clipboard.

use crate::capture::Image;
use std::path::Path;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFOHEADER};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;

const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;

pub fn encode_png(img: &Image, fast: bool) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity((img.w * img.h) as usize);
    {
        let mut enc = png::Encoder::new(&mut out, img.w, img.h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(if fast { png::Compression::Fast } else { png::Compression::Balanced });
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        let mut rgb = Vec::with_capacity((img.w * img.h * 3) as usize);
        for p in img.px.chunks_exact(4) {
            rgb.extend_from_slice(&[p[2], p[1], p[0]]);
        }
        w.write_image_data(&rgb).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

pub fn write_png(path: &Path, img: &Image, fast: bool) -> Result<(), String> {
    let bytes = encode_png(img, fast)?;
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = path.with_extension("png.part");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

pub fn read_png(path: &Path) -> Result<Image, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(f));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; r.output_buffer_size().ok_or("image too large")?];
    let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width, info.height);
    let mut img = Image::new(w, h);
    let ch = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        _ => return Err("unsupported PNG".into()),
    };
    for i in 0..(w * h) as usize {
        let s = &buf[i * ch..i * ch + ch];
        let (r_, g, b) = if ch >= 3 { (s[0], s[1], s[2]) } else { (s[0], s[0], s[0]) };
        img.px[i * 4..i * 4 + 4].copy_from_slice(&[b, g, r_, 255]);
    }
    Ok(img)
}

struct Clip;
impl Clip {
    fn open() -> Option<Clip> {
        for _ in 0..10 {
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Some(Clip);
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        None
    }
}
impl Drop for Clip {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

unsafe fn global(bytes: &[u8]) -> Option<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).ok()?;
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(h);
        Some(h)
    }
}

pub fn copy_image(img: &Image) -> bool {
    let hdr = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: img.w as i32,
        biHeight: img.h as i32,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: img.w * img.h * 4,
        ..Default::default()
    };
    let mut data = Vec::with_capacity(40 + img.px.len());
    data.extend_from_slice(unsafe { std::slice::from_raw_parts(&hdr as *const _ as *const u8, 40) });
    let stride = (img.w * 4) as usize;
    for row in (0..img.h as usize).rev() {
        data.extend_from_slice(&img.px[row * stride..(row + 1) * stride]);
    }
    let Some(_c) = Clip::open() else { return false };
    unsafe {
        let _ = EmptyClipboard();
        match global(&data) {
            Some(h) => SetClipboardData(CF_DIB, Some(HANDLE(h.0))).is_ok(),
            None => false,
        }
    }
}

pub fn copy_text(s: &str) -> bool {
    let w: Vec<u16> = s.encode_utf16().chain([0]).collect();
    let bytes = unsafe { std::slice::from_raw_parts(w.as_ptr() as *const u8, w.len() * 2) };
    let Some(_c) = Clip::open() else { return false };
    unsafe {
        let _ = EmptyClipboard();
        match global(bytes) {
            Some(h) => SetClipboardData(CF_UNICODETEXT, Some(HANDLE(h.0))).is_ok(),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn png_roundtrip() {
        let mut img = super::Image::new(3, 2);
        for (i, p) in img.px.chunks_exact_mut(4).enumerate() {
            p.copy_from_slice(&[i as u8 * 10, 20, 30, 255]);
        }
        let p = std::env::temp_dir().join("framegrab-test-rt.png");
        super::write_png(&p, &img, true).unwrap();
        let back = super::read_png(&p).unwrap();
        assert_eq!(back.px, img.px);
        let _ = std::fs::remove_file(p);
    }
}
