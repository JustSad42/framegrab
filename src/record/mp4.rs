//! H.264 + AAC MP4 writing with the Media Foundation sink writer (hardware encoder when available).

use windows::Win32::Media::MediaFoundation::*;
use windows_core::HSTRING;

pub struct Mp4 {
    w: IMFSinkWriter,
    v: u32,
    a: Option<u32>,
    pub width: u32,
    pub height: u32,
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
            Ok(Mp4 { w, v, a, width, height })
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
                    for x in 0..w {
                        let (b, g, r) = (src[x * 4] as i32, src[x * 4 + 1] as i32, src[x * 4 + 2] as i32);
                        dst[x] = (((47 * r + 157 * g + 16 * b + 128) >> 8) + 16) as u8;
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
}
