//! H.264 + AAC MP4 writing with the Media Foundation sink writer (hardware encoder when available).

use windows::Win32::Media::MediaFoundation::*;
use windows_core::HSTRING;

pub struct Mp4 {
    w: IMFSinkWriter,
    v: u32,
    a: Option<u32>,
    pub width: u32,
    pub height: u32,
    api: Option<ICodecAPI>,
    fps: u32,
    frames: std::cell::Cell<u64>,
}

fn u4(n: u32) -> windows::Win32::System::Variant::VARIANT {
    let mut v = windows::Win32::System::Variant::VARIANT::default();
    unsafe {
        let inner = &mut *v.Anonymous.Anonymous;
        inner.vt = windows::Win32::System::Variant::VT_UI4;
        inner.Anonymous.ulVal = n;
    }
    v
}

/// The encoder's settings interface, with a one-second GOP requested. Encoders often ignore the
/// GOP size once streaming, so `video` also forces a keyframe every second.
fn codec_api(w: &IMFSinkWriter, stream: u32, fps: u32) -> Option<ICodecAPI> {
    use windows_core::Interface;
    unsafe {
        let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
        w.GetServiceForStream(stream, &windows_core::GUID::zeroed(), &ICodecAPI::IID, &mut p).ok()?;
        if p.is_null() {
            return None;
        }
        let api = ICodecAPI::from_raw(p);
        let _ = api.SetValue(&CODECAPI_AVEncMPVGOPSize, &u4(fps.max(1)));
        Some(api)
    }
}

fn pack(a: u32, b: u32) -> u64 {
    ((a as u64) << 32) | b as u64
}

impl Mp4 {
    pub fn new(path: &std::path::Path, width: u32, height: u32, fps: u32, audio: bool) -> windows_core::Result<Mp4> {
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 3)?;
            let attrs = attrs.unwrap();
            attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
            attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
            attrs.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
            let w = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, &attrs)?;

            // Screen content compresses well; ~0.1 bit per pixel per frame, 2–50 Mbit/s.
            let bitrate = ((width as f64 * height as f64 * fps as f64 * 0.1) as u32).clamp(2_000_000, 50_000_000);
            let out = MFCreateMediaType()?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
            out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            out.SetUINT32(&MF_MT_MPEG2_PROFILE, 100)?; // High
            out.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
            out.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            // A keyframe every second: trims (which cut at keyframes) land close to where you want.
            out.SetUINT32(&MF_MT_MAX_KEYFRAME_SPACING, fps.max(1))?;
            let v = w.AddStream(&out)?;

            let inp = MFCreateMediaType()?;
            inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            inp.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            inp.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            inp.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
            inp.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            inp.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            inp.SetUINT32(&MF_MT_DEFAULT_STRIDE, width)?;
            w.SetInputMediaType(v, &inp, None)?;
            let api = codec_api(&w, v, fps);

            let a = if audio {
                let out = MFCreateMediaType()?;
                out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
                out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, super::audio::RATE)?;
                out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
                out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24000)?;
                out.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
                let a = w.AddStream(&out)?;
                let inp = MFCreateMediaType()?;
                inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                inp.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
                inp.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                inp.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, super::audio::RATE)?;
                inp.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
                inp.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
                inp.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, super::audio::RATE * 4)?;
                w.SetInputMediaType(a, &inp, None)?;
                Some(a)
            } else {
                None
            };
            w.BeginWriting()?;
            Ok(Mp4 { w, v, a, width, height, api, fps, frames: Default::default() })
        }
    }

    fn write(&self, stream: u32, bytes: &[u8], t: i64, dur: i64) -> windows_core::Result<()> {
        unsafe {
            let buf = MFCreateMemoryBuffer(bytes.len() as u32)?;
            let mut p: *mut u8 = std::ptr::null_mut();
            buf.Lock(&mut p, None, None)?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            buf.Unlock()?;
            buf.SetCurrentLength(bytes.len() as u32)?;
            let s = MFCreateSample()?;
            s.AddBuffer(&buf)?;
            s.SetSampleTime(t)?;
            s.SetSampleDuration(dur)?;
            self.w.WriteSample(stream, &s)
        }
    }

    /// NV12 frame at `t` (100 ns units).
    pub fn video(&self, nv12: &[u8], t: i64, dur: i64) -> windows_core::Result<()> {
        let n = self.frames.get();
        self.frames.set(n + 1);
        if n > 0 && n % self.fps.max(1) as u64 == 0 {
            if let Some(api) = &self.api {
                let _ = unsafe { api.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &u4(1)) };
            }
        }
        self.write(self.v, nv12, t, dur)
    }

    pub fn audio(&self, pcm: &[i16], first_sample: u64) -> windows_core::Result<()> {
        let Some(a) = self.a else { return Ok(()) };
        let bytes = unsafe { std::slice::from_raw_parts(pcm.as_ptr() as *const u8, pcm.len() * 2) };
        let rate = super::audio::RATE as i64;
        let t = first_sample as i64 * 10_000_000 / rate;
        let dur = (pcm.len() / 2) as i64 * 10_000_000 / rate;
        self.write(a, bytes, t, dur)
    }

    pub fn finish(self) -> windows_core::Result<()> {
        unsafe { self.w.Finalize() }
    }
}

/// BGRA (top-down) → NV12 (BT.709, limited range), split across threads by row bands.
pub fn bgra_to_nv12(px: &[u8], w: usize, h: usize, out: &mut Vec<u8>) {
    out.resize(w * h * 3 / 2, 0);
    let (ys, uvs) = out.split_at_mut(w * h);
    let bands = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8);
    // Bands must cover an even number of rows so each owns whole chroma rows.
    let rows_per = ((h / 2).div_ceil(bands)) * 2;
    std::thread::scope(|sc| {
        let mut y_rest = ys;
        let mut uv_rest = uvs;
        let mut row = 0;
        while row < h {
            let n = rows_per.min(h - row);
            let (yb, yr) = y_rest.split_at_mut(n * w);
            let (ub, ur) = uv_rest.split_at_mut((n / 2) * w);
            y_rest = yr;
            uv_rest = ur;
            let r0 = row;
            sc.spawn(move || {
                for y in 0..n {
                    let src = &px[(r0 + y) * w * 4..(r0 + y + 1) * w * 4];
                    let dst = &mut yb[y * w..(y + 1) * w];
                    for (d, p) in dst.iter_mut().zip(src.chunks_exact(4)) {
                        let (b, g, r) = (p[0] as u32, p[1] as u32, p[2] as u32);
                        *d = (((47 * r + 157 * g + 16 * b + 128) >> 8) + 16) as u8;
                    }
                }
                for cy in 0..n / 2 {
                    let a = &px[(r0 + cy * 2) * w * 4..(r0 + cy * 2 + 1) * w * 4];
                    let b2 = &px[(r0 + cy * 2 + 1) * w * 4..(r0 + cy * 2 + 2) * w * 4];
                    let dst = &mut ub[cy * w..(cy + 1) * w];
                    for cx in 0..w / 2 {
                        let i = cx * 8;
                        let sum = |o: usize| a[i + o] as i32 + a[i + 4 + o] as i32 + b2[i + o] as i32 + b2[i + 4 + o] as i32;
                        let (b, g, r) = (sum(0) / 4, sum(1) / 4, sum(2) / 4);
                        let u = ((-26 * r - 87 * g + 112 * b + 128) >> 8) + 128;
                        let v = ((112 * r - 102 * g - 10 * b + 128) >> 8) + 128;
                        dst[cx * 2] = u.clamp(0, 255) as u8;
                        dst[cx * 2 + 1] = v.clamp(0, 255) as u8;
                    }
                }
            });
            row += n;
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn nv12_colours() {
        let (w, h) = (4, 4);
        let mut px = vec![0u8; w * h * 4];
        for p in px.chunks_exact_mut(4) {
            p.copy_from_slice(&[255, 255, 255, 255]);
        }
        let mut out = Vec::new();
        super::bgra_to_nv12(&px, w, h, &mut out);
        assert_eq!(out.len(), 24);
        assert!(out[0] >= 234 && out[0] <= 236);
        assert!((out[16] as i32 - 128).abs() <= 1);
    }

    /// Worst case for 1080p60: every frame changes. `cargo test --release bench_1080p60 -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_1080p60() {
        use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        fn cpu() -> f64 {
            let (mut a, mut b, mut k, mut u) = Default::default();
            unsafe { GetProcessTimes(GetCurrentProcess(), &mut a, &mut b, &mut k, &mut u).unwrap() };
            let t = |f: windows::Win32::Foundation::FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 1e7;
            t(k) + t(u)
        }
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        let (w, h, fps, n) = (1920usize, 1080usize, 60u32, 600i64);
        let dir = std::env::var("FRAMEGRAB_OUT").unwrap_or_else(|_| std::env::temp_dir().display().to_string());
        let path = std::path::Path::new(&dir).join("bench-1080p60.mp4");
        let mp4 = super::Mp4::new(&path, w as u32, h as u32, fps, false).unwrap();
        let mut px = vec![0u8; w * h * 4];
        let mut nv12 = Vec::new();
        let dur = 10_000_000 / fps as i64;
        let (t0, c0) = (std::time::Instant::now(), cpu());
        let mut conv = 0.0;
        for f in 0..n {
            // Moving gradient + scrolling bars: nothing repeats between frames.
            for y in 0..h {
                let row = &mut px[y * w * 4..(y + 1) * w * 4];
                for (x, p) in row.chunks_exact_mut(4).enumerate() {
                    let v = (x + y + f as usize * 7) as u8;
                    p[0] = v;
                    p[1] = ((y + f as usize * 3) % 256) as u8;
                    p[2] = if (x / 40 + f as usize) % 2 == 0 { 230 } else { 30 };
                    p[3] = 255;
                }
            }
            let tc = std::time::Instant::now();
            super::bgra_to_nv12(&px, w, h, &mut nv12);
            conv += tc.elapsed().as_secs_f64();
            mp4.video(&nv12, f * dur, dur).unwrap();
        }
        mp4.finish().unwrap();
        let (wall, c) = (t0.elapsed().as_secs_f64(), cpu() - c0);
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
        println!("1080p60 x{n}: wall {wall:.2}s (incl. synthetic frame drawing) = {:.0} fps, convert {:.2} ms/frame, process CPU {c:.2}s => {:.1}% of all {cores} cores over a 10 s recording", n as f64 / wall, conv * 1000.0 / n as f64, c / 10.0 / cores * 100.0);
        println!("file: {} ({} KB)", path.display(), std::fs::metadata(&path).map(|m| m.len() / 1024).unwrap_or(0));
    }
}
