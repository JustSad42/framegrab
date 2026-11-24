//! Saving PNGs and putting images / text on the clipboard.

use crate::capture::Image;
use std::path::Path;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFOHEADER};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;

const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;

pub fn has_alpha(img: &Image) -> bool {
    img.px.chunks_exact(4).any(|p| p[3] != 255)
}

fn rgb_bytes(img: &Image, flatten_on: u8) -> Vec<u8> {
    let mut rgb = Vec::with_capacity((img.w * img.h * 3) as usize);
    for p in img.px.chunks_exact(4) {
        let a = p[3] as u32;
        let f = |c: u8| ((c as u32 * a + flatten_on as u32 * (255 - a)) / 255) as u8;
        rgb.extend_from_slice(&[f(p[2]), f(p[1]), f(p[0])]);
    }
    rgb
}

pub fn encode_png(img: &Image, fast: bool) -> Result<Vec<u8>, String> {
    let alpha = has_alpha(img);
    let mut out = Vec::with_capacity((img.w * img.h) as usize);
    {
        let mut enc = png::Encoder::new(&mut out, img.w, img.h);
        enc.set_color(if alpha { png::ColorType::Rgba } else { png::ColorType::Rgb });
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(if fast { png::Compression::Fast } else { png::Compression::Balanced });
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        if alpha {
            w.write_image_data(&img.to_rgba()).map_err(|e| e.to_string())?;
        } else {
            w.write_image_data(&rgb_bytes(img, 255)).map_err(|e| e.to_string())?;
        }
    }
    Ok(out)
}

/// Encodes in the chosen format. JPG has no transparency, so it's flattened onto white.
pub fn encode(img: &Image, fmt: crate::config::ImgFormat, quality: u8) -> Result<Vec<u8>, String> {
    use crate::config::ImgFormat;
    let q = quality.clamp(10, 100);
    match fmt {
        ImgFormat::Png => encode_png(img, false),
        ImgFormat::Jpg => {
            if img.w > 65535 || img.h > 65535 {
                return encode_png(img, false);
            }
            let mut out = Vec::new();
            jpeg_encoder::Encoder::new(&mut out, q).encode(&rgb_bytes(img, 255), img.w as u16, img.h as u16, jpeg_encoder::ColorType::Rgb).map_err(|e| e.to_string())?;
            Ok(out)
        }
        ImgFormat::Webp => {
            let mut out = Vec::new();
            if has_alpha(img) {
                image_webp::WebPEncoder::new(&mut out).encode(&img.to_rgba(), img.w, img.h, image_webp::ColorType::Rgba8).map_err(|e| e.to_string())?;
            } else {
                image_webp::WebPEncoder::new(&mut out).encode(&rgb_bytes(img, 255), img.w, img.h, image_webp::ColorType::Rgb8).map_err(|e| e.to_string())?;
            }
            Ok(out)
        }
        ImgFormat::Avif => {
            let rgba: Vec<rgb::RGBA8> = img.px.chunks_exact(4).map(|p| rgb::RGBA8::new(p[2], p[1], p[0], p[3])).collect();
            let enc = ravif::Encoder::new().with_quality(q as f32).with_alpha_quality(q as f32).with_speed(8);
            let r = enc.encode_rgba(ravif::Img::new(&rgba[..], img.w as usize, img.h as usize)).map_err(|e| e.to_string())?;
            Ok(r.avif_file)
        }
    }
}

pub fn write_image(path: &Path, img: &Image, fmt: crate::config::ImgFormat, quality: u8) -> Result<Vec<u8>, String> {
    let bytes = encode(img, fmt, quality)?;
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(bytes)
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

/// Copies an image: CF_DIB (flattened on white for apps without alpha support) plus a "PNG"
/// clipboard format that keeps transparency (Discord, browsers, Office use it).
/// Test hook: FRAMEGRAB_FAKE_CLIPBOARD=<dir> writes clipboard.png / clipboard.txt there instead.
fn fake_clipboard() -> Option<std::path::PathBuf> {
    std::env::var_os("FRAMEGRAB_FAKE_CLIPBOARD").map(std::path::PathBuf::from)
}

pub fn copy_image(img: &Image) -> bool {
    if let Some(d) = fake_clipboard() {
        return write_png(&d.join("clipboard.png"), img, true).is_ok();
    }
    let alpha = has_alpha(img);
    let flat;
    let src = if alpha {
        let mut f = img.clone();
        for p in f.px.chunks_exact_mut(4) {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 255 * (255 - a)) / 255) as u8;
            }
            p[3] = 255;
        }
        flat = f;
        &flat
    } else {
        img
    };
    let png_bytes = if alpha { encode_png(img, true).ok() } else { None };
    copy_dib(src, png_bytes.as_deref())
}

fn copy_dib(img: &Image, png_bytes: Option<&[u8]>) -> bool {
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
        let ok = match global(&data) {
            Some(h) => SetClipboardData(CF_DIB, Some(HANDLE(h.0))).is_ok(),
            None => false,
        };
        if let Some(pb) = png_bytes {
            let name: Vec<u16> = "PNG\0".encode_utf16().collect();
            let fmt = RegisterClipboardFormatW(windows_core::PCWSTR(name.as_ptr()));
            if let Some(h) = global(pb) {
                let _ = SetClipboardData(fmt, Some(HANDLE(h.0)));
            }
        }
        ok
    }
}

pub fn copy_text(s: &str) -> bool {
    if let Some(d) = fake_clipboard() {
        return std::fs::write(d.join("clipboard.txt"), s).is_ok();
    }
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
