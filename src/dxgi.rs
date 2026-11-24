//! Screen capture with DXGI Desktop Duplication: the GPU copies the desktop, so grabbing several
//! 4K monitors or recording at 60 fps costs a fraction of GDI BitBlt. Callers fall back to GDI
//! whenever this fails (remote sessions, secure desktop, unusual drivers).

use crate::capture::{Image, Rect};
use std::cell::RefCell;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows_core::Interface;

pub struct Dup {
    ctx: ID3D11DeviceContext,
    dev: ID3D11Device,
    dup: IDXGIOutputDuplication,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    /// Desktop rectangle of the output.
    pub rect: Rect,
    rot: DXGI_MODE_ROTATION,
    /// The last copied area (desktop coords) held in `staging`.
    held: Option<Rect>,
}

thread_local! {
    /// One D3D11 device per adapter per thread (created once, reused for every capture).
    static DEVICES: RefCell<Vec<(i64, ID3D11Device, ID3D11DeviceContext)>> = const { RefCell::new(Vec::new()) };
}

fn device_for(adapter: &IDXGIAdapter1) -> Option<(ID3D11Device, ID3D11DeviceContext)> {
    let luid = unsafe { adapter.GetDesc1().ok()? }.AdapterLuid;
    let key = ((luid.HighPart as i64) << 32) | luid.LowPart as i64;
    if let Some(d) = DEVICES.with(|d| d.borrow().iter().find(|e| e.0 == key).map(|e| (e.1.clone(), e.2.clone()))) {
        return Some(d);
    }
    // Same GPU as this thread's window-drawing device: use that one.
    if let Some((dev, luid)) = crate::gfx::shared_d3d() {
        if luid == key {
            if let Ok(ctx) = unsafe { dev.GetImmediateContext() } {
                DEVICES.with(|d| d.borrow_mut().push((key, dev.clone(), ctx.clone())));
                return Some((dev, ctx));
            }
        }
    }
    let mut dev = None;
    let mut ctx = None;
    unsafe {
        D3D11CreateDevice(adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, Some(&[D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1]), D3D11_SDK_VERSION, Some(&mut dev), None, Some(&mut ctx)).ok()?;
    }
    let (dev, ctx) = (dev?, ctx?);
    DEVICES.with(|d| d.borrow_mut().push((key, dev.clone(), ctx.clone())));
    Some((dev, ctx))
}

/// Opens a duplication for every attached output.
pub fn open_all() -> Vec<Dup> {
    let mut v = Vec::new();
    unsafe {
        let Ok(f) = CreateDXGIFactory1::<IDXGIFactory1>() else { return v };
        let mut ai = 0;
        while let Ok(ad) = f.EnumAdapters1(ai) {
            ai += 1;
            let mut oi = 0;
            while let Ok(out) = ad.EnumOutputs(oi) {
                oi += 1;
                let Ok(desc) = out.GetDesc() else { continue };
                if !desc.AttachedToDesktop.as_bool() {
                    continue;
                }
                let Some((dev, ctx)) = device_for(&ad) else { continue };
                let Ok(o1) = out.cast::<IDXGIOutput1>() else { continue };
                let Ok(dup) = o1.DuplicateOutput(&dev) else { continue };
                v.push(Dup { ctx, dev, dup, staging: None, rect: Rect::from_win(desc.DesktopCoordinates), rot: desc.Rotation, held: None });
            }
        }
    }
    v
}

pub fn open_for(r: Rect) -> Option<Dup> {
    open_all().into_iter().find(|d| d.rect.intersect(&r).is_some_and(|i| i == r))
}

impl Dup {
    fn rotated(&self) -> bool {
        self.rot == DXGI_MODE_ROTATION_ROTATE90 || self.rot == DXGI_MODE_ROTATION_ROTATE180 || self.rot == DXGI_MODE_ROTATION_ROTATE270
    }

    /// Waits up to `timeout` ms for a new frame and copies `area` (desktop coords, inside this
    /// output) to the CPU-readable texture. Ok(false) = no new frame; Err = duplication lost.
    pub fn acquire(&mut self, timeout: u32, area: Rect) -> Result<bool, windows_core::Error> {
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            match self.dup.AcquireNextFrame(timeout, &mut info, &mut res) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(false),
                Err(e) => return Err(e),
            }
            let r = (|| -> windows_core::Result<bool> {
                // Pointer-only updates carry no new image.
                if info.LastPresentTime == 0 && self.held.is_some() {
                    return Ok(false);
                }
                let tex: ID3D11Texture2D = res.as_ref().ok_or_else(|| windows_core::Error::from_hresult(windows_core::HRESULT(-1)))?.cast()?;
                let mut td = D3D11_TEXTURE2D_DESC::default();
                tex.GetDesc(&mut td);
                // Rotated outputs copy whole; others copy just the area.
                let (bx, by, bw, bh) = if self.rotated() {
                    (0, 0, td.Width, td.Height)
                } else {
                    let a = area.intersect(&self.rect).unwrap_or(self.rect);
                    ((a.x - self.rect.x) as u32, (a.y - self.rect.y) as u32, a.w as u32, a.h as u32)
                };
                let need_new = !matches!(&self.staging, Some((_, w, h)) if *w == bw && *h == bh);
                if need_new {
                    let sd = D3D11_TEXTURE2D_DESC {
                        Width: bw,
                        Height: bh,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: td.Format,
                        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        Usage: D3D11_USAGE_STAGING,
                        BindFlags: 0,
                        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                        MiscFlags: 0,
                    };
                    let mut t = None;
                    self.dev.CreateTexture2D(&sd, None, Some(&mut t))?;
                    self.staging = Some((t.unwrap(), bw, bh));
                }
                let st = &self.staging.as_ref().unwrap().0;
                let b = D3D11_BOX { left: bx, top: by, front: 0, right: bx + bw, bottom: by + bh, back: 1 };
                self.ctx.CopySubresourceRegion(st, 0, 0, 0, 0, &tex, 0, Some(&b));
                self.held = Some(if self.rotated() { self.rect } else { area.intersect(&self.rect).unwrap_or(self.rect) });
                Ok(true)
            })();
            let _ = self.dup.ReleaseFrame();
            r
        }
    }

    /// Reads `area` (desktop coords) from the last acquired frame.
    pub fn read(&mut self, area: Rect, out: &mut Image) -> bool {
        let Some(held) = self.held else { return false };
        let Some((st, sw, sh)) = &self.staging else { return false };
        let Some(a) = area.intersect(&held) else { return false };
        if out.w != a.w as u32 || out.h != a.h as u32 {
            *out = Image::new(a.w as u32, a.h as u32);
        }
        unsafe {
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            if self.ctx.Map(st, 0, D3D11_MAP_READ, 0, Some(&mut m)).is_err() {
                return false;
            }
            let src = std::slice::from_raw_parts(m.pData as *const u8, (m.RowPitch * sh) as usize);
            let pitch = m.RowPitch as usize;
            let (ox, oy) = (a.x - held.x, a.y - held.y);
            let ow = a.w as usize;
            if !self.rotated() {
                for y in 0..a.h as usize {
                    let s = (oy as usize + y) * pitch + ox as usize * 4;
                    out.px[y * ow * 4..(y + 1) * ow * 4].copy_from_slice(&src[s..s + ow * 4]);
                }
            } else {
                // Texture is in panel orientation; map each desktop pixel back to it.
                let (tw, th) = (*sw as i32, *sh as i32);
                let (dw, dh) = (held.w, held.h);
                for y in 0..a.h {
                    let dy = oy + y;
                    for x in 0..a.w {
                        let dx = ox + x;
                        let (tx, ty) = match self.rot {
                            DXGI_MODE_ROTATION_ROTATE90 => (dy, dw - 1 - dx),
                            DXGI_MODE_ROTATION_ROTATE180 => (dw - 1 - dx, dh - 1 - dy),
                            _ => (dh - 1 - dy, dx),
                        };
                        if tx < 0 || ty < 0 || tx >= tw || ty >= th {
                            continue;
                        }
                        let s = ty as usize * pitch + tx as usize * 4;
                        let d = (y as usize * ow + x as usize) * 4;
                        out.px[d..d + 4].copy_from_slice(&src[s..s + 4]);
                    }
                }
            }
            self.ctx.Unmap(st, 0);
        }
        for p in out.px.chunks_exact_mut(4) {
            p[3] = 255;
        }
        true
    }
}

/// Grabs every monitor through DXGI. None if any output can't be duplicated (caller uses GDI).
pub fn grab_monitors(mons: &[crate::capture::Monitor]) -> Option<Vec<Image>> {
    if std::env::var_os("FRAMEGRAB_NO_DXGI").is_some() {
        return None;
    }
    let mut dups = open_all();
    let mut out = Vec::with_capacity(mons.len());
    for m in mons {
        let d = dups.iter_mut().find(|d| d.rect == m.rect)?;
        // A fresh duplication hands out the current desktop on its first frame.
        let mut ok = false;
        for _ in 0..3 {
            if d.acquire(150, m.rect).ok()? {
                ok = true;
                break;
            }
        }
        if !ok {
            return None;
        }
        let mut img = Image::new(1, 1);
        if !d.read(m.rect, &mut img) {
            return None;
        }
        out.push(img);
    }
    Some(out)
}

/// Drops the cached D3D11 devices (they're recreated on the next capture, ~10 ms).
pub fn release_devices() {
    DEVICES.with(|d| d.borrow_mut().clear());
}
