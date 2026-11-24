//! Direct2D + DirectWrite drawing for the native windows (overlay, toolbar card, recording bar).
//! Everything is drawn in physical pixels; callers multiply by the monitor scale.

use std::cell::RefCell;
use std::collections::HashMap;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows_core::{Interface, PCWSTR};
use windows_numerics::Vector2;

pub mod col {
    pub const PANEL: u32 = 0xF21C1C21;
    pub const PANEL_HI: u32 = 0xFF2A2A31;
    pub const BORDER: u32 = 0x22FFFFFF;
    pub const TEXT: u32 = 0xFFF3F3F6;
    pub const MUTED: u32 = 0xFFA3A3AE;
    pub const ACCENT: u32 = 0xFF6E8BFF;
    pub const ACCENT_SOFT: u32 = 0x336E8BFF;
    pub const REC: u32 = 0xFFFF4D5E;
    pub const DIM: u32 = 0x8C000000;
    pub const WHITE: u32 = 0xFFFFFFFF;
    pub const BLACK: u32 = 0xFF000000;
}

pub const ICON_FONT: &[u8] = include_bytes!("../assets/phosphor.ttf");

pub mod icon {
    pub const SELECTION: &str = "\u{E626}";
    pub const MONITOR: &str = "\u{E32E}";
    pub const TEXT_T: &str = "\u{E6EE}";
    pub const SCROLL: &str = "\u{EB7A}";
    pub const RECORD: &str = "\u{E3EE}";
    pub const PENCIL: &str = "\u{E3B4}";
    pub const COPY: &str = "\u{E1CA}";
    pub const FOLDER_OPEN: &str = "\u{E256}";
    pub const TRASH: &str = "\u{E4A6}";
    pub const X: &str = "\u{E4F6}";
    pub const STOP: &str = "\u{E46C}";
    pub const PAUSE: &str = "\u{E39E}";
    pub const PLAY: &str = "\u{E3D0}";
    pub const MIC: &str = "\u{E326}";
    pub const MIC_SLASH: &str = "\u{E328}";
    pub const SPEAKER: &str = "\u{E44A}";
    pub const SPEAKER_X: &str = "\u{E45C}";
    pub const CHECK: &str = "\u{E182}";
    pub const FILM: &str = "\u{E792}";
    pub const GIF: &str = "\u{EA82}";
    pub const APP_WINDOW: &str = "\u{E5DA}";
}

pub struct Factories {
    pub d2d: ID2D1Factory,
    pub dw: IDWriteFactory,
    pub icons: Option<IDWriteFontCollection>,
    formats: RefCell<HashMap<(u32, u32, bool), IDWriteTextFormat>>,
}

thread_local! {
    static FACT: &'static Factories = Box::leak(Box::new(Factories::new()));
}

pub fn fact() -> &'static Factories {
    FACT.with(|f| *f)
}

impl Factories {
    fn new() -> Self {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).expect("d2d");
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).expect("dwrite");
            let icons = load_icons(&dw).ok();
            Self { d2d, dw, icons, formats: RefCell::new(HashMap::new()) }
        }
    }

    /// Text format keyed by (pixel size*10, weight, icon font).
    pub fn format(&self, size: f32, weight: u32, icons: bool) -> Option<IDWriteTextFormat> {
        let key = ((size * 10.0) as u32, weight, icons);
        if let Some(f) = self.formats.borrow().get(&key) {
            return Some(f.clone());
        }
        let f = unsafe {
            let name: Vec<u16> = if icons { "Phosphor" } else { "Segoe UI Variable Text" }.encode_utf16().chain([0]).collect();
            let loc: Vec<u16> = "en-us\0".encode_utf16().collect();
            let coll = if icons { self.icons.as_ref() } else { None };
            let f = self
                .dw
                .CreateTextFormat(PCWSTR(name.as_ptr()), coll, DWRITE_FONT_WEIGHT(weight as i32), DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, PCWSTR(loc.as_ptr()))
                .ok()?;
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            f
        };
        self.formats.borrow_mut().insert(key, f.clone());
        Some(f)
    }

    /// Width of a single-line string in pixels.
    pub fn measure(&self, s: &str, size: f32, weight: u32) -> f32 {
        let Some(f) = self.format(size, weight, false) else { return s.len() as f32 * size * 0.5 };
        let w: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            if let Ok(l) = self.dw.CreateTextLayout(&w, &f, 10000.0, 1000.0) {
                let mut m = DWRITE_TEXT_METRICS::default();
                if l.GetMetrics(&mut m).is_ok() {
                    return m.widthIncludingTrailingWhitespace;
                }
            }
        }
        s.len() as f32 * size * 0.5
    }
}

unsafe fn load_icons(dw: &IDWriteFactory) -> windows_core::Result<IDWriteFontCollection> {
    unsafe {
        let f5: IDWriteFactory5 = dw.cast()?;
        let loader = f5.CreateInMemoryFontFileLoader()?;
        f5.RegisterFontFileLoader(&loader)?;
        let file = loader.CreateInMemoryFontFileReference(&f5, ICON_FONT.as_ptr() as _, ICON_FONT.len() as u32, None)?;
        let b: IDWriteFontSetBuilder1 = f5.CreateFontSetBuilder()?.cast()?;
        b.AddFontFile(&file)?;
        let set = b.CreateFontSet()?;
        let c: IDWriteFontCollection1 = f5.CreateFontCollectionFromFontSet(&set)?;
        c.cast()
    }
}

pub fn color(argb: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        a: ((argb >> 24) & 255) as f32 / 255.0,
        r: ((argb >> 16) & 255) as f32 / 255.0,
        g: ((argb >> 8) & 255) as f32 / 255.0,
        b: (argb & 255) as f32 / 255.0,
    }
}

pub fn rf(x: f32, y: f32, w: f32, h: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// A render target bound to a window plus one reusable brush.
pub struct Canvas {
    pub rt: ID2D1RenderTarget,
    hwnd_rt: Option<ID2D1HwndRenderTarget>,
    wic: Option<windows::Win32::Graphics::Imaging::IWICBitmap>,
    brush: ID2D1SolidColorBrush,
    dash: Option<ID2D1StrokeStyle>,
}

impl Canvas {
    pub fn new(hwnd: HWND, w: u32, h: u32) -> Option<Canvas> {
        unsafe {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            let hp = D2D1_HWND_RENDER_TARGET_PROPERTIES { hwnd, pixelSize: D2D_SIZE_U { width: w.max(1), height: h.max(1) }, presentOptions: D2D1_PRESENT_OPTIONS_NONE };
            let hrt = fact().d2d.CreateHwndRenderTarget(&props, &hp).ok()?;
            Self::wrap(hrt.cast().ok()?, Some(hrt), None)
        }
    }

    fn wrap(rt: ID2D1RenderTarget, hwnd_rt: Option<ID2D1HwndRenderTarget>, wic: Option<windows::Win32::Graphics::Imaging::IWICBitmap>) -> Option<Canvas> {
        unsafe {
            rt.SetDpi(96.0, 96.0);
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = rt.CreateSolidColorBrush(&color(col::WHITE), None).ok()?;
            let sp = D2D1_STROKE_STYLE_PROPERTIES { dashStyle: D2D1_DASH_STYLE_CUSTOM, ..Default::default() };
            let dash = fact().d2d.CreateStrokeStyle(&sp, Some(&[4.0, 4.0])).ok();
            Some(Canvas { rt, hwnd_rt, wic, brush, dash })
        }
    }

    /// An off-screen canvas (for tests and thumbnails); read it back with `pixels`.
    pub fn offscreen(w: u32, h: u32) -> Option<Canvas> {
        use windows::Win32::Graphics::Imaging::*;
        use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
        unsafe {
            let wf: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
            let bmp = wf.CreateBitmap(w, h, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad).ok()?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            let rt = fact().d2d.CreateWicBitmapRenderTarget(&bmp, &props).ok()?;
            Self::wrap(rt, None, Some(bmp))
        }
    }

    /// BGRA pixels of an off-screen canvas.
    pub fn pixels(&self) -> Option<crate::capture::Image> {
        use windows::Win32::Graphics::Imaging::*;
        let bmp = self.wic.as_ref()?;
        unsafe {
            let (mut w, mut h) = (0u32, 0u32);
            bmp.GetSize(&mut w, &mut h).ok()?;
            let mut img = crate::capture::Image::new(w, h);
            let r = windows::Win32::Graphics::Imaging::WICRect { X: 0, Y: 0, Width: w as i32, Height: h as i32 };
            bmp.CopyPixels(&r, w * 4, &mut img.px).ok()?;
            let _ = WICBitmapCacheOnLoad;
            Some(img)
        }
    }

    pub fn resize(&self, w: u32, h: u32) {
        if let Some(h_rt) = &self.hwnd_rt {
            unsafe {
                let _ = h_rt.Resize(&D2D_SIZE_U { width: w.max(1), height: h.max(1) });
            }
        }
    }

    pub fn begin(&self) {
        unsafe { self.rt.BeginDraw() }
    }

    /// Returns false when the target must be recreated.
    pub fn end(&self) -> bool {
        unsafe { self.rt.EndDraw(None, None).is_ok() }
    }

    pub fn clear(&self, c: u32) {
        unsafe { self.rt.Clear(Some(&color(c))) }
    }

    fn b(&self, c: u32) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&color(c)) };
        &self.brush
    }

    pub fn fill(&self, x: f32, y: f32, w: f32, h: f32, c: u32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        unsafe { self.rt.FillRectangle(&rf(x, y, w, h), self.b(c)) }
    }

    pub fn round(&self, x: f32, y: f32, w: f32, h: f32, r: f32, c: u32) {
        let rr = D2D1_ROUNDED_RECT { rect: rf(x, y, w, h), radiusX: r, radiusY: r };
        unsafe { self.rt.FillRoundedRectangle(&rr, self.b(c)) }
    }

    pub fn round_stroke(&self, x: f32, y: f32, w: f32, h: f32, r: f32, c: u32, width: f32) {
        let rr = D2D1_ROUNDED_RECT { rect: rf(x, y, w, h), radiusX: r, radiusY: r };
        unsafe { self.rt.DrawRoundedRectangle(&rr, self.b(c), width, None) }
    }

    pub fn stroke(&self, x: f32, y: f32, w: f32, h: f32, c: u32, width: f32) {
        unsafe { self.rt.DrawRectangle(&rf(x, y, w, h), self.b(c), width, None) }
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: u32, width: f32, dashed: bool) {
        let ds = if dashed { self.dash.as_ref() } else { None };
        unsafe { self.rt.DrawLine(Vector2 { X: x0, Y: y0 }, Vector2 { X: x1, Y: y1 }, self.b(c), width, ds) }
    }

    pub fn circle(&self, cx: f32, cy: f32, r: f32, c: u32) {
        let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r };
        unsafe { self.rt.FillEllipse(&e, self.b(c)) }
    }

    pub fn circle_stroke(&self, cx: f32, cy: f32, r: f32, c: u32, width: f32) {
        let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r };
        unsafe { self.rt.DrawEllipse(&e, self.b(c), width, None) }
    }

    pub fn text(&self, s: &str, x: f32, y: f32, w: f32, h: f32, size: f32, weight: u32, c: u32, align: Align) {
        self.text_inner(s, x, y, w, h, size, weight, c, align, false)
    }

    pub fn icon(&self, s: &str, x: f32, y: f32, w: f32, h: f32, size: f32, c: u32) {
        self.text_inner(s, x, y, w, h, size, 400, c, Align::Center, true)
    }

    fn text_inner(&self, s: &str, x: f32, y: f32, w: f32, h: f32, size: f32, weight: u32, c: u32, align: Align, icons: bool) {
        let Some(f) = fact().format(size, weight, icons) else { return };
        unsafe {
            let _ = f.SetTextAlignment(match align {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            });
            let w16: Vec<u16> = s.encode_utf16().collect();
            self.rt.DrawText(&w16, &f, &rf(x, y, w, h), self.b(c), D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        }
    }

    /// Uploads BGRA pixels as a bitmap.
    pub fn bitmap(&self, w: u32, h: u32, px: &[u8]) -> Option<ID2D1Bitmap> {
        unsafe {
            let bp = D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                dpiX: 96.0,
                dpiY: 96.0,
            };
            self.rt.CreateBitmap(D2D_SIZE_U { width: w, height: h }, Some(px.as_ptr() as _), w * 4, &bp).ok()
        }
    }

    pub fn draw_bitmap(&self, bmp: &ID2D1Bitmap, dst: D2D_RECT_F, src: D2D_RECT_F, opacity: f32, nearest: bool) {
        let mode = if nearest { D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR } else { D2D1_BITMAP_INTERPOLATION_MODE_LINEAR };
        unsafe { self.rt.DrawBitmap(bmp, Some(&dst), opacity, mode, Some(&src)) }
    }

    pub fn clip(&self, x: f32, y: f32, w: f32, h: f32) {
        unsafe { self.rt.PushAxisAlignedClip(&rf(x, y, w, h), D2D1_ANTIALIAS_MODE_ALIASED) }
    }

    pub fn unclip(&self) {
        unsafe { self.rt.PopAxisAlignedClip() }
    }

    /// Clips to a rounded rectangle via a layer; pair with `pop_layer`.
    pub fn push_round_clip(&self, x: f32, y: f32, w: f32, h: f32, r: f32) -> bool {
        unsafe {
            let rr = D2D1_ROUNDED_RECT { rect: rf(x, y, w, h), radiusX: r, radiusY: r };
            let Ok(g) = fact().d2d.CreateRoundedRectangleGeometry(&rr) else { return false };
            let Ok(layer) = self.rt.CreateLayer(None) else { return false };
            let Ok(geo) = g.cast::<ID2D1Geometry>() else { return false };
            let mut lp = D2D1_LAYER_PARAMETERS {
                contentBounds: rf(-1e6, -1e6, 2e6, 2e6),
                geometricMask: std::mem::ManuallyDrop::new(Some(geo)),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: windows_numerics::Matrix3x2::identity(),
                opacity: 1.0,
                opacityBrush: std::mem::ManuallyDrop::new(None),
                layerOptions: D2D1_LAYER_OPTIONS_NONE,
            };
            self.rt.PushLayer(&lp, &layer);
            std::mem::ManuallyDrop::drop(&mut lp.geometricMask);
            true
        }
    }

    pub fn pop_layer(&self) {
        unsafe { self.rt.PopLayer() }
    }
}

#[cfg(test)]
mod icon_sheet {
    /// `cargo test icon_sheet -- --ignored` renders candidate glyphs to FRAMEGRAB_OUT\icons.png.
    #[test]
    #[ignore]
    fn icon_sheet() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        }
        let cands: Vec<(&str, u32)> = vec![
            ("cursor", 0xE1DC), ("arrowUR", 0xE092), ("square", 0xE45E), ("circle", 0xE18A), ("lineseg", 0xE6E6),
            ("highlt", 0xEC86), ("textT", 0xE48A), ("num1", 0xE3F6), ("drop", 0xE210), ("crop", 0xE1D4),
            ("undo", 0xE038), ("redo", 0xE036), ("floppy", 0xE248), ("scroll", 0xEB7A), ("record", 0xE3EE),
            ("mic", 0xE326), ("micX", 0xE328), ("spkr", 0xE44A), ("spkrX", 0xE45C), ("eyeX", 0xE224),
            ("cursorClk", 0xE7D8), ("handGrab", 0xE80C), ("listNum", 0xE2F6), ("dropHalf", 0xE566), ("gridFour", 0xE296),
            ("arrowR", 0xE06C), ("export", 0xEAF0), ("magPlus", 0xE310), ("magMinus", 0xE30E), ("sliders", 0xE434),
            ("penNib", 0xE3AC), ("eraser", 0xE8B2), ("paintBr", 0xE6F0), ("selection", 0xE69A), ("checkCirc", 0xE184),
        ];
        let cols = 5;
        let (cw, ch) = (180.0f32, 70.0f32);
        let rows = cands.len().div_ceil(cols);
        let c = super::Canvas::offscreen((cw * cols as f32) as u32, (ch * rows as f32) as u32).unwrap();
        c.begin();
        c.clear(0xFF1C1C21);
        for (i, (name, cp)) in cands.iter().enumerate() {
            let x = (i % cols) as f32 * cw;
            let y = (i / cols) as f32 * ch;
            let s = char::from_u32(*cp).unwrap().to_string();
            c.icon(&s, x, y, 50.0, ch, 30.0, 0xFFFFFFFF);
            c.text(&format!("{name} {cp:X}"), x + 52.0, y, cw - 52.0, ch, 14.0, 400, 0xFFA3A3AE, super::Align::Left);
        }
        c.end();
        let px = c.pixels().unwrap();
        let dir = std::env::var("FRAMEGRAB_OUT").unwrap_or_else(|_| ".".into());
        crate::output::write_png(&std::path::Path::new(&dir).join("icons.png"), &px, true).unwrap();
    }
}
