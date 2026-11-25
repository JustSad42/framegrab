//! Trim a recording (or any MP4) and read text from any frame.
//! Scrubbing and playback decode with Media Foundation; saving copies the compressed samples
//! between the cut points into a new MP4 (no re-encode, so it's instant and lossless). The start
//! snaps to a keyframe because a cut can only begin there without re-encoding.

use crate::capture::Image;
use crate::editor::{icon_button, primary_button, separator, text_button};
use crate::ui::{self, ACCENT, BORDER, CARD, FAINT, MUTED, PANEL, TEXT};
use egui::{Color32, CornerRadius, Sense, Stroke, pos2, vec2};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Variant::{VT_I8, VT_UI8};
use windows_core::{GUID, HSTRING};

const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const ANY: u32 = MF_SOURCE_READER_ANY_STREAM.0 as u32;
const ALL: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
const SEC: i64 = 10_000_000;

// ---------------------------------------------------------------- media

fn start_mf() {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_FULL);
    }
}

fn pv_i64(t: i64) -> PROPVARIANT {
    let mut pv = PROPVARIANT::default();
    unsafe {
        let inner = &mut *pv.Anonymous.Anonymous;
        inner.vt = VT_I8;
        inner.Anonymous.hVal = t;
    }
    pv
}

fn pv_u64(pv: &PROPVARIANT) -> Option<u64> {
    unsafe {
        let inner = &*pv.Anonymous.Anonymous;
        (inner.vt == VT_UI8 || inner.vt == VT_I8).then(|| inner.Anonymous.uhVal)
    }
}

fn reader(path: &Path, decode: bool) -> windows_core::Result<IMFSourceReader> {
    unsafe {
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 2)?;
        let attrs = attrs.unwrap();
        if decode {
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
        }
        MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attrs)
    }
}

fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

#[derive(Clone, Debug, Default)]
pub struct Info {
    pub dur: i64,
    pub w: u32,
    pub h: u32,
    pub fps: f32,
    /// Keyframe times (where a lossless cut can start).
    pub keys: Vec<i64>,
    pub frames: usize,
    pub audio: bool,
}

/// Duration, size, frame rate and keyframes (reads the compressed stream; no decoding).
pub fn probe(path: &Path) -> Result<Info, String> {
    start_mf();
    let run = || -> windows_core::Result<Info> {
        unsafe {
            let r = reader(path, false)?;
            let mut info = Info::default();
            if let Ok(pv) = r.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION) {
                info.dur = pv_u64(&pv).unwrap_or(0) as i64;
            }
            let t = r.GetNativeMediaType(VIDEO, 0)?;
            let (w, h) = unpack(t.GetUINT64(&MF_MT_FRAME_SIZE)?);
            info.w = w;
            info.h = h;
            if let Ok(fr) = t.GetUINT64(&MF_MT_FRAME_RATE) {
                let (n, d) = unpack(fr);
                info.fps = n as f32 / d.max(1) as f32;
            }
            info.audio = r.GetNativeMediaType(AUDIO, 0).is_ok();
            r.SetStreamSelection(ALL, false)?;
            r.SetStreamSelection(VIDEO, true)?;
            loop {
                let (mut flags, mut ts, mut s) = (0u32, 0i64, None);
                r.ReadSample(VIDEO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut s))?;
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                let Some(s) = s else { continue };
                info.frames += 1;
                if s.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0 {
                    info.keys.push(ts);
                }
                let d = s.GetSampleDuration().unwrap_or(0);
                info.dur = info.dur.max(ts + d);
            }
            if info.keys.is_empty() {
                info.keys.push(0);
            }
            if info.fps <= 0.0 && info.dur > 0 {
                info.fps = info.frames as f32 * SEC as f32 / info.dur as f32;
            }
            Ok(info)
        }
    };
    run().map_err(|e| format!("Couldn't open the video: {}", e.message()))
}

/// Copies the samples between `start` (a keyframe) and `end` into a new MP4, timestamps rebased to 0.
pub fn remux(src: &Path, dst: &Path, start: i64, end: i64) -> Result<(), String> {
    start_mf();
    let run = || -> windows_core::Result<()> {
        unsafe {
            let r = reader(src, false)?;
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 1)?;
            let attrs = attrs.unwrap();
            attrs.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
            let w = MFCreateSinkWriterFromURL(&HSTRING::from(dst.as_os_str()), None, &attrs)?;
            r.SetStreamSelection(ALL, false)?;
            // Source stream index → sink stream index.
            let mut map: Vec<(u32, u32, bool)> = Vec::new();
            let mut i = 0u32;
            while let Ok(t) = r.GetNativeMediaType(i, 0) {
                let major = t.GetMajorType()?;
                let is_video = major == MFMediaType_Video;
                if (is_video || major == MFMediaType_Audio) && !map.iter().any(|m| m.2 == is_video) {
                    r.SetStreamSelection(i, true)?;
                    let cur = r.GetCurrentMediaType(i)?;
                    let o = w.AddStream(&cur)?;
                    w.SetInputMediaType(o, &cur, None)?;
                    map.push((i, o, is_video));
                }
                i += 1;
            }
            if !map.iter().any(|m| m.2) {
                return Err(windows_core::Error::new(windows_core::HRESULT(-1), "no video stream"));
            }
            w.BeginWriting()?;
            r.SetCurrentPosition(&GUID::zeroed(), &pv_i64(start))?;
            let mut base = start;
            let mut first_video = true;
            let mut done = vec![false; map.len()];
            let mut wrote = 0usize;
            while !done.iter().all(|d| *d) {
                let (mut idx, mut flags, mut ts, mut s) = (0u32, 0u32, 0i64, None);
                r.ReadSample(ANY, 0, Some(&mut idx), Some(&mut flags), Some(&mut ts), Some(&mut s))?;
                let Some(k) = map.iter().position(|m| m.0 == idx) else {
                    if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                        break;
                    }
                    continue;
                };
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    done[k] = true;
                    continue;
                }
                let Some(s) = s else { continue };
                if done[k] {
                    continue;
                }
                if ts >= end {
                    done[k] = true;
                    continue;
                }
                let (_, o, is_video) = map[k];
                if is_video && first_video {
                    first_video = false;
                    // The source may land on an earlier keyframe than asked; start there.
                    base = base.min(ts);
                }
                if ts < base || (!is_video && first_video) {
                    continue;
                }
                s.SetSampleTime(ts - base)?;
                w.WriteSample(o, &s)?;
                wrote += 1;
            }
            if wrote == 0 {
                return Err(windows_core::Error::new(windows_core::HRESULT(-1), "nothing in that range"));
            }
            w.Finalize()
        }
    };
    run().map_err(|e| {
        let _ = std::fs::remove_file(dst);
        format!("Couldn't save the trimmed video: {}", e.message())
    })
}

enum Cmd {
    Seek(i64),
    Play(i64, i64),
}

enum Msg {
    /// Frame time, picture, and whether it's full size (playback sends smaller ones).
    Frame(i64, Image, bool),
    Ended,
    Error(String),
}

/// Playback frames are at most this wide (a 4K frame is 33 MB; the window shows far less).
const PLAY_MAX_W: u32 = 1600;

/// Copies a decoded frame out, keeping every `step`-th pixel (1 = full size).
fn frame_of(s: &IMFSample, w: u32, h: u32, stride: i32, step: u32) -> windows_core::Result<Image> {
    unsafe {
        let buf = s.ConvertToContiguousBuffer()?;
        let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
        buf.Lock(&mut p, None, Some(&mut len))?;
        let data = std::slice::from_raw_parts(p, len as usize);
        let step = step.max(1);
        let (ow, oh) = (w.div_ceil(step), h.div_ceil(step));
        let mut img = Image::new(ow, oh);
        let st = stride.unsigned_abs() as usize;
        for y in 0..oh as usize {
            let srcy = y * step as usize;
            // A negative stride means the rows are stored bottom-up.
            let sy = if stride < 0 { h as usize - 1 - srcy } else { srcy };
            let o = sy * st;
            if o + (w * 4) as usize > data.len() {
                continue;
            }
            let dst = &mut img.px[y * (ow * 4) as usize..(y + 1) * (ow * 4) as usize];
            if step == 1 {
                dst.copy_from_slice(&data[o..o + (w * 4) as usize]);
            } else {
                for (x, d) in dst.chunks_exact_mut(4).enumerate() {
                    let i = o + x * step as usize * 4;
                    d.copy_from_slice(&data[i..i + 4]);
                }
            }
        }
        buf.Unlock()?;
        for a in img.px.iter_mut().skip(3).step_by(4) {
            *a = 255;
        }
        Ok(img)
    }
}

/// Decoder thread: seeks to frames and plays ranges, newest command wins.
fn decoder(path: PathBuf, rx: Receiver<Cmd>, tx: Sender<Msg>, ctx: egui::Context) {
    start_mf();
    let send = |m: Msg| {
        let _ = tx.send(m);
        ctx.request_repaint();
    };
    let setup = || -> windows_core::Result<(IMFSourceReader, u32, u32, i32)> {
        unsafe {
            let r = reader(&path, true)?;
            r.SetStreamSelection(ALL, false)?;
            r.SetStreamSelection(VIDEO, true)?;
            let t = MFCreateMediaType()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            r.SetCurrentMediaType(VIDEO, None, &t)?;
            let cur = r.GetCurrentMediaType(VIDEO)?;
            let (w, h) = unpack(cur.GetUINT64(&MF_MT_FRAME_SIZE)?);
            let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or((w * 4) as i32);
            Ok((r, w, h, stride))
        }
    };
    let (r, w, h, stride) = match setup() {
        Ok(v) => v,
        Err(e) => {
            send(Msg::Error(format!("Couldn't decode the video: {}", e.message())));
            return;
        }
    };
    let read = || -> Option<(i64, i64, Option<IMFSample>)> {
        let (mut flags, mut ts, mut s) = (0u32, 0i64, None);
        unsafe { r.ReadSample(VIDEO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut s)).ok()? };
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            return None;
        }
        let d = s.as_ref().and_then(|s| unsafe { s.GetSampleDuration().ok() }).unwrap_or(0);
        Some((ts, d, s))
    };
    let seek = |t: i64| unsafe {
        let _ = r.SetCurrentPosition(&GUID::zeroed(), &pv_i64(t));
    };
    let mut pending: Option<Cmd> = None;
    loop {
        let cmd = match pending.take() {
            Some(c) => c,
            None => match rx.recv() {
                Ok(c) => c,
                Err(_) => return,
            },
        };
        // Only the newest command matters (scrubbing sends many).
        let mut cmd = cmd;
        while let Ok(c) = rx.try_recv() {
            cmd = c;
        }
        match cmd {
            Cmd::Seek(t) => {
                seek(t);
                let mut last: Option<(i64, IMFSample)> = None;
                while let Some((ts, d, s)) = read() {
                    if let Some(s) = s {
                        last = Some((ts, s));
                        if ts + d.max(1) > t {
                            break;
                        }
                    }
                }
                if let Some((ts, s)) = last {
                    if let Ok(img) = frame_of(&s, w, h, stride, 1) {
                        send(Msg::Frame(ts, img, true));
                    }
                }
            }
            Cmd::Play(from, until) => {
                seek(from);
                let t0 = Instant::now();
                loop {
                    if let Ok(c) = rx.try_recv() {
                        pending = Some(c);
                        break;
                    }
                    let Some((ts, _, s)) = read() else {
                        send(Msg::Ended);
                        break;
                    };
                    if ts >= until {
                        send(Msg::Ended);
                        break;
                    }
                    let Some(s) = s else { continue };
                    if ts < from {
                        continue;
                    }
                    let due = t0 + Duration::from_nanos(((ts - from).max(0) * 100) as u64);
                    let now = Instant::now();
                    if due > now {
                        std::thread::sleep(due - now);
                    } else if now - due > Duration::from_millis(100) {
                        continue; // Behind: skip converting this frame.
                    }
                    if let Ok(img) = frame_of(&s, w, h, stride, w.div_ceil(PLAY_MAX_W)) {
                        send(Msg::Frame(ts, img, false));
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- window

pub fn fmt_time(t: i64) -> String {
    let ms = (t.max(0) / 10_000) as u64;
    let (m, s, cs) = (ms / 60_000, (ms / 1000) % 60, (ms % 1000) / 10);
    format!("{m}:{s:02}.{cs:02}")
}

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    Start,
    End,
    Head,
}

struct Trim {
    path: PathBuf,
    cfg: crate::config::Config,
    info: Option<Info>,
    probe_rx: Option<Receiver<Result<Info, String>>>,
    cmd: Option<Sender<Cmd>>,
    frames: Option<Receiver<Msg>>,
    tex: Option<egui::TextureHandle>,
    frame: Option<(i64, Image)>,
    head: i64,
    start: i64,
    end: i64,
    playing: bool,
    drag: Option<Drag>,
    status: Option<(String, f64)>,
    error: Option<String>,
    ocr_rx: Option<Receiver<Result<crate::ocr::OcrOut, String>>>,
    text: Option<String>,
    save_rx: Option<Receiver<(Result<(), String>, bool, PathBuf)>>,
    demo: Option<(PathBuf, u32)>,
    /// Whether `frame` is full size (playback frames are smaller).
    frame_full: bool,
    /// Size of the last playback frame (test hook report).
    played: Option<(u32, u32)>,
}

pub fn run(path: PathBuf) {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("{} — Trim · Framegrab", path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()))
            .with_inner_size([1100.0, 760.0])
            .with_min_inner_size([760.0, 480.0])
            .with_icon(ui::app_icon()),
        centered: true,
        ..Default::default()
    };
    let _ = eframe::run_native(
        "Framegrab Trim",
        options,
        Box::new(move |cc| {
            ui::install(&cc.egui_ctx);
            Ok(Box::new(Trim::new(path, &cc.egui_ctx)))
        }),
    );
}

impl Trim {
    fn new(path: PathBuf, ctx: &egui::Context) -> Self {
        let mut t = Self {
            path,
            cfg: crate::config::Config::load(),
            info: None,
            probe_rx: None,
            cmd: None,
            frames: None,
            tex: None,
            frame: None,
            head: 0,
            start: 0,
            end: 0,
            playing: false,
            drag: None,
            status: None,
            error: None,
            ocr_rx: None,
            text: None,
            save_rx: None,
            demo: std::env::var_os("FRAMEGRAB_TRIM_DEMO").map(|d| (PathBuf::from(d), 0)),
            frame_full: false,
            played: None,
        };
        t.open(ctx);
        t
    }

    /// (Re)opens the file: probe + decoder.
    fn open(&mut self, ctx: &egui::Context) {
        self.cmd = None; // Ends the old decoder.
        self.info = None;
        self.playing = false;
        let (ptx, prx) = channel();
        let p = self.path.clone();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let _ = ptx.send(probe(&p));
            c.request_repaint();
        });
        self.probe_rx = Some(prx);
        let (ctx_tx, crx) = channel();
        let (ftx, frx) = channel();
        let p = self.path.clone();
        let c = ctx.clone();
        std::thread::spawn(move || decoder(p, crx, ftx, c));
        let _ = ctx_tx.send(Cmd::Seek(0));
        self.cmd = Some(ctx_tx);
        self.frames = Some(frx);
    }

    fn send(&self, c: Cmd) {
        if let Some(tx) = &self.cmd {
            let _ = tx.send(c);
        }
    }

    fn seek(&mut self, t: i64) {
        let dur = self.info.as_ref().map(|i| i.dur).unwrap_or(0);
        self.head = t.clamp(0, dur.max(0));
        self.playing = false;
        self.send(Cmd::Seek(self.head));
    }

    fn snap_start(&self, t: i64) -> i64 {
        let Some(i) = &self.info else { return t };
        i.keys.iter().copied().filter(|k| *k <= t + 1).max().unwrap_or(0)
    }

    fn frame_len(&self) -> i64 {
        self.info.as_ref().map(|i| (SEC as f32 / i.fps.max(1.0)) as i64).unwrap_or(SEC / 30)
    }

    fn trimmed(&self) -> bool {
        self.info.as_ref().is_some_and(|i| self.start > 0 || self.end < i.dur - self.frame_len() / 2)
    }

    fn toggle_play(&mut self) {
        if self.playing {
            // Pausing shows the full-size frame (also what Read text reads).
            self.playing = false;
            self.send(Cmd::Seek(self.head));
            return;
        }
        let from = if self.head >= self.end - self.frame_len() || self.head < self.start { self.start } else { self.head };
        self.playing = true;
        self.send(Cmd::Play(from, self.end));
    }

    fn read_text(&mut self, ctx: &egui::Context, now: f64) {
        let Some((_, img)) = &self.frame else { return };
        if self.ocr_rx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let img = img.clone();
        let cfg = self.cfg.clone();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(crate::ocr::grab_text(&img, &cfg));
            c.request_repaint();
        });
        self.ocr_rx = Some(rx);
        self.status = Some(("Reading text…".into(), now));
    }

    fn save(&mut self, ctx: &egui::Context, now: f64, copy: bool) {
        if self.save_rx.is_some() || self.info.is_none() {
            return;
        }
        let dst = if copy {
            let stem = self.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Recording".into());
            let dir = self.path.parent().map(Path::to_path_buf).unwrap_or_default();
            let Some(p) = rfd::FileDialog::new().set_directory(dir).set_file_name(format!("{stem} (trimmed).mp4")).add_filter("MP4 video", &["mp4"]).save_file() else { return };
            p
        } else {
            self.path.with_extension("trim-tmp.mp4")
        };
        // Saving over the original needs the decoder to let go of the file.
        if !copy {
            self.cmd = None;
            self.frames = None;
            self.playing = false;
        }
        let (tx, rx) = channel();
        let (src, start, end) = (self.path.clone(), self.start, self.end);
        let c = ctx.clone();
        std::thread::spawn(move || {
            let mut r = remux(&src, &dst, start, end);
            if r.is_ok() && !copy {
                // Give the decoder thread a moment to close the file, then replace it.
                let mut tries = 0;
                r = loop {
                    match std::fs::rename(&dst, &src) {
                        Ok(()) => break Ok(()),
                        Err(_) if tries < 20 => {
                            tries += 1;
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        Err(e) => {
                            let _ = std::fs::remove_file(&dst);
                            break Err(format!("Couldn't replace the original: {e}"));
                        }
                    }
                };
            }
            let _ = tx.send((r, copy, if copy { dst } else { src }));
            c.request_repaint();
        });
        self.save_rx = Some(rx);
        self.status = Some(("Saving…".into(), now));
    }

    fn poll(&mut self, ctx: &egui::Context, now: f64) {
        if let Some(rx) = &self.probe_rx {
            if let Ok(r) = rx.try_recv() {
                self.probe_rx = None;
                match r {
                    Ok(i) => {
                        self.start = 0;
                        self.end = i.dur;
                        self.info = Some(i);
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        }
        if let Some(rx) = &self.frames {
            let mut latest = None;
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Frame(t, img, full) => latest = Some((t, img, full)),
                    Msg::Ended => {
                        // Swap the small playback frame for a full-size one.
                        self.playing = false;
                        self.send(Cmd::Seek(self.head));
                    }
                    Msg::Error(e) => self.error = Some(e),
                }
            }
            if let Some((t, img, full)) = latest {
                self.frame_full = full;
                if !full {
                    self.played = Some((img.w, img.h));
                }
                let ci = ui::color_image(&img);
                match &mut self.tex {
                    Some(tex) => tex.set(ci, egui::TextureOptions::LINEAR),
                    None => self.tex = Some(ctx.load_texture("frame", ci, egui::TextureOptions::LINEAR)),
                }
                if self.playing || self.drag.is_none() {
                    self.head = t;
                }
                self.frame = Some((t, img));
            }
        }
        if let Some(rx) = &self.ocr_rx {
            if let Ok(r) = rx.try_recv() {
                self.ocr_rx = None;
                match r {
                    Ok(o) if !o.text.trim().is_empty() => {
                        crate::output::copy_text(&o.text);
                        let n = o.text.lines().count();
                        self.status = Some((format!("Copied {} line{} of text", n, if n == 1 { "" } else { "s" }), now));
                        self.text = Some(o.text);
                    }
                    Ok(_) => self.status = Some(("No text in this frame".into(), now)),
                    Err(e) => self.status = Some((e, now)),
                }
            }
        }
        if let Some(rx) = &self.save_rx {
            if let Ok((r, copy, at)) = rx.try_recv() {
                self.save_rx = None;
                match r {
                    Ok(()) => {
                        let len = fmt_time(self.end - self.start);
                        self.status = Some((if copy { format!("Saved a {len} copy as {}", at.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()) } else { format!("Trimmed to {len}") }, now));
                    }
                    Err(e) => self.status = Some((e, now)),
                }
                if !copy {
                    self.open(ctx);
                }
            }
        }
    }

    fn timeline(&mut self, ui: &mut egui::Ui) {
        let Some(info) = self.info.clone() else {
            ui.add_space(56.0);
            return;
        };
        let dur = info.dur.max(1);
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::click_and_drag());
        let track = egui::Rect::from_min_max(pos2(r.left() + 8.0, r.top() + 10.0), pos2(r.right() - 8.0, r.bottom() - 10.0));
        let x_of = |t: i64| track.left() + track.width() * (t as f32 / dur as f32);
        let t_of = |x: f32| (((x - track.left()) / track.width()).clamp(0.0, 1.0) * dur as f32) as i64;
        let p = ui.painter();
        p.rect_filled(track, CornerRadius::same(6), CARD);
        // Keyframes: where the start can go.
        if info.keys.len() < 3000 {
            for k in &info.keys {
                let x = x_of(*k);
                p.line_segment([pos2(x, track.bottom() - 6.0), pos2(x, track.bottom() - 2.0)], Stroke::new(1.0, FAINT));
            }
        }
        let sel = egui::Rect::from_min_max(pos2(x_of(self.start), track.top()), pos2(x_of(self.end), track.bottom()));
        p.rect_filled(sel, CornerRadius::same(6), ACCENT.gamma_multiply(0.22));
        p.rect_stroke(sel, CornerRadius::same(6), Stroke::new(2.0, ACCENT), egui::StrokeKind::Inside);
        for (x, left) in [(sel.left(), true), (sel.right(), false)] {
            let h = egui::Rect::from_center_size(pos2(if left { x + 4.0 } else { x - 4.0 }, track.center().y), vec2(8.0, track.height()));
            p.rect_filled(h, CornerRadius::same(3), ACCENT);
            p.line_segment([pos2(h.center().x, h.center().y - 6.0), pos2(h.center().x, h.center().y + 6.0)], Stroke::new(1.5, Color32::from_black_alpha(140)));
        }
        let hx = x_of(self.head);
        p.line_segment([pos2(hx, r.top() + 2.0), pos2(hx, r.bottom() - 2.0)], Stroke::new(2.0, TEXT));
        p.circle_filled(pos2(hx, r.top() + 4.0), 4.0, TEXT);

        let pointer = resp.interact_pointer_pos();
        if resp.drag_started() {
            if let Some(pt) = pointer {
                let near = |x: f32| (pt.x - x).abs() < 12.0;
                self.drag = Some(if near(sel.left()) { Drag::Start } else if near(sel.right()) { Drag::End } else { Drag::Head });
                self.playing = false;
            }
        }
        let hover_handle = resp.hover_pos().is_some_and(|pt| (pt.x - sel.left()).abs() < 12.0 || (pt.x - sel.right()).abs() < 12.0);
        if hover_handle || matches!(self.drag, Some(Drag::Start | Drag::End)) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let min_len = self.frame_len() * 2;
        if let (Some(d), Some(pt)) = (self.drag, pointer) {
            if resp.dragged() || resp.drag_started() {
                let t = t_of(pt.x);
                match d {
                    Drag::Start => {
                        let s = self.snap_start(t.min(self.end - min_len));
                        if s != self.start {
                            self.start = s;
                            self.seek(s);
                        }
                    }
                    Drag::End => {
                        let e = t.clamp(self.start + min_len, dur);
                        if e != self.end {
                            self.end = e;
                            self.seek((e - self.frame_len()).max(self.start));
                        }
                    }
                    Drag::Head => self.seek(t),
                }
            }
        }
        if resp.drag_stopped() {
            self.drag = None;
        }
        if resp.clicked() {
            if let Some(pt) = pointer {
                self.seek(t_of(pt.x));
            }
        }
        resp.on_hover_text("Drag the blue edges to trim · I / O set start / end at the playhead · ← → step a frame · Space plays");
    }

    fn keys(&mut self, ctx: &egui::Context, now: f64) {
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        let pressed = |k: egui::Key| ctx.input(|i| i.key_pressed(k));
        if pressed(egui::Key::Space) {
            self.toggle_play();
        }
        let fl = self.frame_len();
        if pressed(egui::Key::ArrowLeft) {
            self.seek(self.head - fl);
        }
        if pressed(egui::Key::ArrowRight) {
            self.seek(self.head + fl + 1);
        }
        if pressed(egui::Key::I) {
            self.start = self.snap_start(self.head.min(self.end - fl * 2));
            self.seek(self.start);
        }
        if pressed(egui::Key::O) {
            self.end = (self.head + fl).max(self.start + fl * 2);
        }
        if pressed(egui::Key::R) {
            self.read_text(ctx, now);
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) && self.trimmed() {
            self.save(ctx, now, false);
        }
    }

    fn demo_step(&mut self, ctx: &egui::Context, now: f64) {
        let Some((dir, n)) = self.demo.clone() else { return };
        ctx.request_repaint();
        let Some(info) = self.info.clone() else { return };
        let mut n = n;
        match n {
            0 => {
                self.start = self.snap_start(info.dur / 3);
                self.end = info.dur * 2 / 3;
                // Play a moment first (checks the smaller playback frames), then pause mid-range.
                self.head = self.start;
                self.toggle_play();
                n = 10;
            }
            10 if self.played.is_some() => {
                self.seek((self.start + self.end) / 2);
                n = 1;
            }
            1 if self.frame_full && self.frame.as_ref().is_some_and(|(t, _)| (*t - self.head).abs() < SEC / 2) && self.drag.is_none() => {
                self.read_text(ctx, now);
                n += 1;
            }
            2 if self.ocr_rx.is_none() => {
                let slot = DEMO_SHOT.clone();
                ctx.request_screenshot(move |img| *slot.lock().unwrap() = Some(img));
                n += 1;
            }
            3 => {
                let shot = DEMO_SHOT.lock().unwrap().take();
                if let Some(img) = shot {
                    let im = Image::from_rgba(img.size[0] as u32, img.size[1] as u32, img.as_raw());
                    let _ = crate::output::write_png(&dir.join("trim-ui.png"), &im, true);
                    let out = dir.join("trimmed.mp4");
                    let _ = std::fs::remove_file(&out);
                    let r = remux(&self.path, &out, self.start, self.end);
                    let probe_out = probe(&out);
                    let rep = format!(
                        "source: {:?}\nplayback frame: {:?}, paused frame: {:?}\nrange: {} - {} (start keyframe {})\nremux: {:?}\ntrimmed: {:?}\nocr: {:?}\n",
                        DebugInfo(&info),
                        self.played,
                        self.frame.as_ref().map(|(_, i)| (i.w, i.h)),
                        fmt_time(self.start),
                        fmt_time(self.end),
                        info.keys.contains(&self.start),
                        r,
                        probe_out.as_ref().map(DebugInfo),
                        self.text
                    );
                    let _ = std::fs::write(dir.join("trim.txt"), rep);
                    if std::env::var_os("FRAMEGRAB_TRIM_DEMO_REPLACE").is_some() {
                        // Also exercise "Save trimmed" (replaces the opened file).
                        self.save(ctx, now, false);
                    } else {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    n += 1;
                }
            }
            4 if self.save_rx.is_none() && self.probe_rx.is_none() => {
                let rep = format!("status: {:?}
reopened: {:?}
", self.status.as_ref().map(|s| &s.0), self.info.as_ref().map(DebugInfo));
                let _ = std::fs::write(dir.join("trim-replace.txt"), rep);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                n += 1;
            }
            _ => {}
        }
        self.demo = Some((dir, n));
    }
}

static DEMO_SHOT: std::sync::LazyLock<std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<egui::ColorImage>>>>> = std::sync::LazyLock::new(Default::default);

struct DebugInfo<'a>(&'a Info);
impl std::fmt::Debug for DebugInfo<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let i = self.0;
        write!(f, "{}x{} {:.1} fps, {} frames, {} keyframes, {}, audio={}", i.w, i.h, i.fps, i.frames, i.keys.len(), fmt_time(i.dur), i.audio)
    }
}

impl eframe::App for Trim {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        self.poll(&ctx, now);
        self.keys(&ctx, now);
        self.demo_step(&ctx, now);
        if self.playing || self.save_rx.is_some() || self.ocr_rx.is_some() || self.probe_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        egui::Panel::bottom("controls").frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(14, 10))).show(ui, |ui| {
            self.timeline(ui);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let ready = self.info.is_some();
                if icon_button(ui, if self.playing { "\u{E39E}" } else { "\u{E3D0}" }, false, "Play / pause the trimmed part  (Space)").clicked() && ready {
                    self.toggle_play();
                }
                separator(ui);
                if let Some(i) = &self.info {
                    ui.label(egui::RichText::new(fmt_time(self.head)).font(egui::FontId::monospace(14.0)).color(TEXT));
                    ui.label(egui::RichText::new(format!("/ {}", fmt_time(i.dur))).font(egui::FontId::monospace(14.0)).color(FAINT));
                    separator(ui);
                    ui.label(egui::RichText::new(format!("Keep {} → {}  ·  {}", fmt_time(self.start), fmt_time(self.end), fmt_time(self.end - self.start))).color(MUTED));
                } else if self.error.is_none() {
                    ui.label(egui::RichText::new("Opening…").color(MUTED));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let can = self.trimmed() && self.save_rx.is_none();
                    let save = primary_button(ui, "\u{EAE0}", "Save trimmed").on_hover_text("Replace the video with the kept part  (Ctrl+S)\nNo re-encoding: instant and lossless. The start lands on the nearest keyframe.");
                    if save.clicked() && can {
                        self.save(&ctx, now, false);
                    }
                    if text_button(ui, "\u{E248}", "Save copy").on_hover_text("Save the kept part as a new file").clicked() && can {
                        self.save(&ctx, now, true);
                    }
                    if text_button(ui, "\u{E6EE}", "Read text").on_hover_text("Read the text in this frame and copy it  (R)").clicked() {
                        self.read_text(&ctx, now);
                    }
                });
            });
            if let Some((msg, t)) = &self.status {
                if now - t < 4.0 {
                    ui.label(egui::RichText::new(msg).small().color(Color32::from_rgb(0x5B, 0xD6, 0x8A)));
                } else {
                    ctx.request_repaint_after(Duration::from_millis(500));
                }
            }
        });

        if self.text.is_some() {
            let mut close = false;
            egui::Panel::right("text").exact_size(320.0).frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::same(12))).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Text in this frame").font(ui::bold(14.0)).color(TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Close").clicked() {
                            close = true;
                        }
                        if ui.small_button("Copy").clicked() {
                            if let Some(t) = &self.text {
                                crate::output::copy_text(t);
                                self.status = Some(("Copied".into(), now));
                            }
                        }
                    });
                });
                ui.add_space(6.0);
                if let Some(t) = &mut self.text {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add(egui::TextEdit::multiline(t).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
                    });
                }
            });
            if close {
                self.text = None;
            }
        }

        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::from_rgb(0x10, 0x10, 0x13)).inner_margin(egui::Margin::same(16))).show(ui, |ui| {
            let avail = ui.available_rect_before_wrap();
            if let Some(e) = &self.error {
                ui.painter().text(avail.center(), egui::Align2::CENTER_CENTER, e, egui::FontId::proportional(15.0), MUTED);
                return;
            }
            if let Some(tex) = &self.tex {
                let s = tex.size_vec2();
                let k = (avail.width() / s.x).min(avail.height() / s.y).min(4.0);
                let r = egui::Rect::from_center_size(avail.center(), s * k);
                ui.painter().image(tex.id(), r, egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                ui.painter().rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, BORDER), egui::StrokeKind::Outside);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(fmt_time(0), "0:00.00");
        assert_eq!(fmt_time(SEC * 75 + SEC / 2), "1:15.50");
    }

    /// FRAMEGRAB_TRIM_FILE=<mp4> cargo test --release trim_live -- --ignored --nocapture
    #[test]
    #[ignore]
    fn trim_live() {
        let p = PathBuf::from(std::env::var("FRAMEGRAB_TRIM_FILE").unwrap());
        let i = probe(&p).unwrap();
        println!("{:?}", DebugInfo(&i));
        let out = p.with_extension("cut.mp4");
        let start = i.keys.iter().copied().find(|k| *k >= i.dur / 3).unwrap_or(0);
        let t = Instant::now();
        remux(&p, &out, start, i.dur * 2 / 3).unwrap();
        println!("remux {:?}", t.elapsed());
        let o = probe(&out).unwrap();
        println!("{:?}", DebugInfo(&o));
        assert!((o.dur - (i.dur * 2 / 3 - start)).abs() < SEC / 5);
    }

    /// Recordings get a keyframe every second (hardware or software encoder).
    #[test]
    #[ignore]
    fn recording_keyframes() {
        let p = std::env::temp_dir().join("framegrab-gop-test.mp4");
        let (w, h, fps) = (640u32, 360u32, 30u32);
        let m = crate::record::mp4::Mp4::new(&p, w, h, fps, false).unwrap();
        let mut nv = Vec::new();
        for f in 0..fps * 4 {
            let mut img = Image::new(w, h);
            for (i, px) in img.px.chunks_exact_mut(4).enumerate() {
                let v = ((i as u32 % w + f * 7) % 255) as u8;
                px.copy_from_slice(&[v, 255 - v, (f * 3) as u8, 255]);
            }
            crate::record::mp4::bgra_to_nv12(&img.px, w as usize, h as usize, &mut nv);
            m.video(&nv, f as i64 * SEC / fps as i64, SEC / fps as i64).unwrap();
        }
        m.finish().unwrap();
        let i = probe(&p).unwrap();
        println!("{:?} keys at {:?}", DebugInfo(&i), i.keys.iter().map(|k| fmt_time(*k)).collect::<Vec<_>>());
        let _ = std::fs::remove_file(&p);
        assert!(i.keys.len() >= 4, "expected a keyframe each second");
    }
}
