//! Text recognition with the OCR engine built into Windows (Windows.Media.Ocr).

use crate::capture::Image;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows_core::HSTRING;

fn engine(lang: &str) -> windows_core::Result<OcrEngine> {
    if !lang.trim().is_empty() {
        if let Ok(l) = Language::CreateLanguage(&HSTRING::from(lang.trim())) {
            if OcrEngine::IsLanguageSupported(&l).unwrap_or(false) {
                return OcrEngine::TryCreateFromLanguage(&l);
            }
        }
    }
    OcrEngine::TryCreateFromUserProfileLanguages()
}

/// Installed OCR languages as (tag, display name).
pub fn languages() -> Vec<(String, String)> {
    let mut v = Vec::new();
    if let Ok(list) = OcrEngine::AvailableRecognizerLanguages() {
        for i in 0..list.Size().unwrap_or(0) {
            if let Ok(l) = list.GetAt(i) {
                v.push((l.LanguageTag().map(|s| s.to_string()).unwrap_or_default(), l.DisplayName().map(|s| s.to_string()).unwrap_or_default()));
            }
        }
    }
    v
}

/// Upscales small captures 2× (the engine reads small UI text much better that way).
fn prepare(img: &Image, max: u32) -> Image {
    let mut out = img.clone();
    if img.w * 2 <= max && img.h * 2 <= max && (img.w as u64 * img.h as u64) < 2_500_000 {
        let (w, h) = (img.w * 2, img.h * 2);
        let mut up = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                // Bilinear sample at the source pixel centre.
                let fx = ((x as f32 + 0.5) / 2.0 - 0.5).max(0.0);
                let fy = ((y as f32 + 0.5) / 2.0 - 0.5).max(0.0);
                let (x0, y0) = (fx as u32, fy as u32);
                let (x1, y1) = ((x0 + 1).min(img.w - 1), (y0 + 1).min(img.h - 1));
                let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                let p = |xx: u32, yy: u32, c: usize| img.px[((yy * img.w + xx) * 4) as usize + c] as f32;
                let o = ((y * w + x) * 4) as usize;
                for c in 0..3 {
                    let top = p(x0, y0, c) * (1.0 - tx) + p(x1, y0, c) * tx;
                    let bot = p(x0, y1, c) * (1.0 - tx) + p(x1, y1, c) * tx;
                    up.px[o + c] = (top * (1.0 - ty) + bot * ty) as u8;
                }
                up.px[o + 3] = 255;
            }
        }
        out = up;
    } else if img.w > max || img.h > max {
        out = img.fit(max, max);
    }
    out
}

/// Recognizes the text in an image. Run off the UI thread.
pub fn recognize(img: &Image, lang: &str) -> Result<String, String> {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    let run = || -> windows_core::Result<String> {
        let eng = engine(lang)?;
        let max = OcrEngine::MaxImageDimension().unwrap_or(10000);
        let img = prepare(img, max);
        let w = DataWriter::new()?;
        w.WriteBytes(&img.px)?;
        let buf = w.DetachBuffer()?;
        let sb = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(&buf, BitmapPixelFormat::Bgra8, img.w as i32, img.h as i32, BitmapAlphaMode::Premultiplied)?;
        let res = eng.RecognizeAsync(&sb)?.join()?;
        let lines = res.Lines()?;
        let mut out = Vec::new();
        for i in 0..lines.Size()? {
            out.push(lines.GetAt(i)?.Text()?.to_string());
        }
        Ok(out.join("\n"))
    };
    match run() {
        Ok(s) => Ok(s),
        Err(e) if e.code().0 == 0 => Err("No OCR language is installed (Settings › Time & language › Language)".into()),
        Err(e) => Err(format!("Text recognition failed: {}", e.message())),
    }
}
