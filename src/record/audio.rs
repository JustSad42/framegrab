//! WASAPI capture of system sound (loopback) and/or the microphone, resampled to 48 kHz stereo
//! and mixed on a clock shared with the video, so silence and pauses stay in sync.

use super::Clock;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
use windows::Win32::Media::Multimedia::WAVE_FORMAT_IEEE_FLOAT;
use windows::Win32::System::Com::*;

pub const RATE: u32 = 48000;

enum Fmt {
    F32,
    I16,
    I32,
    I24,
}

struct Source {
    client: IAudioClient,
    cap: IAudioCaptureClient,
    fmt: Fmt,
    channels: usize,
    rate: u32,
    buf: VecDeque<[f32; 2]>,
    // Linear resampler state.
    pos: f64,
    prev: [f32; 2],
    gain: f32,
}

unsafe fn open(flow: EDataFlow, loopback: bool, gain: f32) -> windows_core::Result<Source> {
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let dev = en.GetDefaultAudioEndpoint(flow, eConsole)?;
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
        let wf = client.GetMixFormat()?;
        let f = &*wf;
        let bits = f.wBitsPerSample;
        let tag = f.wFormatTag as u32;
        let is_float = tag == WAVE_FORMAT_IEEE_FLOAT
            || (tag == WAVE_FORMAT_EXTENSIBLE && {
                let ext = &*(wf as *const WAVEFORMATEXTENSIBLE);
                let sub = ext.SubFormat;
                sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
            });
        let fmt = match (is_float, bits) {
            (true, 32) => Fmt::F32,
            (false, 16) => Fmt::I16,
            (false, 24) => Fmt::I24,
            (false, 32) => Fmt::I32,
            _ => Fmt::F32,
        };
        let channels = f.nChannels.max(1) as usize;
        let rate = f.nSamplesPerSec;
        let flags = if loopback { AUDCLNT_STREAMFLAGS_LOOPBACK } else { 0 };
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, wf, None)?;
        CoTaskMemFree(Some(wf as _));
        let cap: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        Ok(Source { client, cap, fmt, channels, rate, buf: VecDeque::new(), pos: 0.0, prev: [0.0; 2], gain })
    }
}

impl Source {
    fn sample(&self, p: *const u8, i: usize, ch: usize) -> f32 {
        unsafe {
            match self.fmt {
                Fmt::F32 => *(p as *const f32).add(i * self.channels + ch),
                Fmt::I16 => *(p as *const i16).add(i * self.channels + ch) as f32 / 32768.0,
                Fmt::I32 => *(p as *const i32).add(i * self.channels + ch) as f32 / 2147483648.0,
                Fmt::I24 => {
                    let b = p.add((i * self.channels + ch) * 3);
                    let v = ((*b as i32) | ((*b.add(1) as i32) << 8) | ((*b.add(2) as i8 as i32) << 16)) as f32;
                    v / 8388608.0
                }
            }
        }
    }

    /// Pulls every pending packet into the 48 kHz buffer.
    fn drain(&mut self, keep: bool) {
        unsafe {
            loop {
                let n = match self.cap.GetNextPacketSize() {
                    Ok(n) if n > 0 => n,
                    _ => break,
                };
                let _ = n;
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                if self.cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None).is_err() {
                    break;
                }
                if keep {
                    let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                    let step = self.rate as f64 / RATE as f64;
                    for i in 0..frames as usize {
                        let cur = if silent {
                            [0.0, 0.0]
                        } else {
                            let l = self.sample(data, i, 0);
                            let r = if self.channels > 1 { self.sample(data, i, 1) } else { l };
                            [l * self.gain, r * self.gain]
                        };
                        // Emit output samples that fall between prev and cur.
                        while self.pos <= 1.0 {
                            let t = self.pos as f32;
                            self.buf.push_back([self.prev[0] + (cur[0] - self.prev[0]) * t, self.prev[1] + (cur[1] - self.prev[1]) * t]);
                            self.pos += step;
                        }
                        self.pos -= 1.0;
                        self.prev = cur;
                    }
                }
                let _ = self.cap.ReleaseBuffer(frames);
            }
        }
        // Never let one source lag far behind (e.g. device clock drift): cap at 1 s.
        while self.buf.len() > RATE as usize {
            self.buf.pop_front();
        }
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

/// Runs until `stop`; calls `out(samples, first_sample_index)` with interleaved stereo i16.
pub fn run(system: bool, mic: bool, clock: Arc<Clock>, stop: Arc<AtomicBool>, mut out: impl FnMut(Vec<i16>, u64)) -> Result<(), String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut sources = Vec::new();
    let mut errs = Vec::new();
    if system {
        match unsafe { open(eRender, true, 1.0) } {
            Ok(s) => sources.push(s),
            Err(e) => errs.push(format!("system sound: {}", e.message())),
        }
    }
    if mic {
        match unsafe { open(eCapture, false, 1.0) } {
            Ok(s) => sources.push(s),
            Err(e) => errs.push(format!("microphone: {}", e.message())),
        }
    }
    if sources.is_empty() {
        return Err(errs.join("; "));
    }
    // Mix a little behind real time so late packets still land in the right place.
    let lag = (RATE / 10) as u64;
    let mut produced: u64 = 0;
    let mut was_paused = false;
    while !stop.load(Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(10));
        let paused = clock.paused();
        for s in &mut sources {
            s.drain(!paused);
            if paused {
                s.buf.clear();
            }
        }
        if paused {
            was_paused = true;
            continue;
        }
        if was_paused {
            was_paused = false;
        }
        let now = (clock.elapsed().as_secs_f64() * RATE as f64) as u64;
        let target = now.saturating_sub(lag);
        if target <= produced {
            continue;
        }
        let n = (target - produced) as usize;
        let mut pcm = Vec::with_capacity(n * 2);
        for _ in 0..n {
            let mut l = 0.0f32;
            let mut r = 0.0f32;
            for s in &mut sources {
                if let Some(v) = s.buf.pop_front() {
                    l += v[0];
                    r += v[1];
                }
            }
            pcm.push((l.clamp(-1.0, 1.0) * 32767.0) as i16);
            pcm.push((r.clamp(-1.0, 1.0) * 32767.0) as i16);
        }
        out(pcm, produced);
        produced = target;
    }
    Ok(())
}
