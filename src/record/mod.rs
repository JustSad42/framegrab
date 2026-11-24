//! Screen recording: a capture thread grabs the area at a fixed rate, an encoder thread writes
//! MP4 (H.264/AAC via Media Foundation) or GIF, and an audio thread feeds system sound / mic.

pub mod audio;
pub mod mp4;

use crate::capture::{self, Image, Rect};
use crate::config::RecFormat;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Wall clock that stops while paused.
pub struct Clock {
    start: Instant,
    st: Mutex<(Duration, Option<Instant>)>,
}

impl Clock {
    pub fn new() -> Self {
        Self { start: Instant::now(), st: Mutex::new((Duration::ZERO, None)) }
    }
    pub fn elapsed(&self) -> Duration {
        let st = self.st.lock().unwrap();
        let now = st.1.unwrap_or_else(Instant::now);
        now.duration_since(self.start).saturating_sub(st.0)
    }
    pub fn paused(&self) -> bool {
        self.st.lock().unwrap().1.is_some()
    }
    pub fn set_paused(&self, p: bool) {
        let mut st = self.st.lock().unwrap();
        match (p, st.1) {
            (true, None) => st.1 = Some(Instant::now()),
            (false, Some(t)) => {
                st.0 += t.elapsed();
                st.1 = None;
            }
            _ => {}
        }
    }
}

pub struct Opts {
    pub area: Rect,
    pub fps: u32,
    pub gif_fps: u32,
    pub format: RecFormat,
    pub system_audio: bool,
    pub mic: bool,
    pub cursor: bool,
    pub path: PathBuf,
}

enum Msg {
    Video(Image, Duration),
    Audio(Vec<i16>, u64),
}

pub struct Recorder {
    pub clock: Arc<Clock>,
    stop: Arc<AtomicBool>,
    pub frames: Arc<AtomicU64>,
    pub path: PathBuf,
    pub format: RecFormat,
    threads: Vec<std::thread::JoinHandle<()>>,
    encoder: Option<std::thread::JoinHandle<Result<Option<Image>, String>>>,
    pub audio_error: Arc<Mutex<Option<String>>>,
}

/// GIFs are scaled down to keep files and encode times reasonable.
const GIF_MAX_W: u32 = 960;

impl Recorder {
    pub fn start(o: Opts) -> Result<Recorder, String> {
        // H.264 needs even dimensions.
        let mut area = o.area;
        area.w &= !1;
        area.h &= !1;
        if area.w < 16 || area.h < 16 {
            return Err("The area is too small to record".into());
        }
        let clock = Arc::new(Clock::new());
        let stop = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(AtomicU64::new(0));
        let audio_error = Arc::new(Mutex::new(None));
        let (tx, rx) = sync_channel::<Msg>(6);
        let fps = if o.format == RecFormat::Gif { o.gif_fps.clamp(5, 30) } else { o.fps.clamp(10, 60) };
        let with_audio = o.format == RecFormat::Mp4 && (o.system_audio || o.mic);

        // Encoder (opens the file now so errors show up before recording starts).
        let (ready_tx, ready_rx) = sync_channel::<Result<(), String>>(1);
        let path = o.path.clone();
        let format = o.format;
        let encoder = std::thread::Builder::new()
            .name("encoder".into())
            .spawn(move || match format {
                RecFormat::Mp4 => encode_mp4(rx, &path, area, fps, with_audio, ready_tx),
                RecFormat::Gif => encode_gif(rx, &path, area, fps, ready_tx),
            })
            .map_err(|e| e.to_string())?;
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err("The video encoder did not start".into()),
        }

        let mut threads = Vec::new();
        {
            let (clock, stop, frames, tx) = (clock.clone(), stop.clone(), frames.clone(), tx.clone());
            let cursor = o.cursor;
            threads.push(
                std::thread::Builder::new()
                    .name("rec-capture".into())
                    .spawn(move || capture_loop(area, fps, cursor, clock, stop, frames, tx))
                    .map_err(|e| e.to_string())?,
            );
        }
        if with_audio {
            let (clock, stop, tx, err) = (clock.clone(), stop.clone(), tx.clone(), audio_error.clone());
            let (sys, mic) = (o.system_audio, o.mic);
            threads.push(
                std::thread::Builder::new()
                    .name("rec-audio".into())
                    .spawn(move || {
                        let r = audio::run(sys, mic, clock, stop, |pcm, at| {
                            let _ = tx.send(Msg::Audio(pcm, at));
                        });
                        if let Err(e) = r {
                            *err.lock().unwrap() = Some(e);
                        }
                    })
                    .map_err(|e| e.to_string())?,
            );
        }
        drop(tx);
        Ok(Recorder { clock, stop, frames, path: o.path, format: o.format, threads, encoder: Some(encoder), audio_error })
    }

    /// Stops and finalizes the file (blocks; run off the UI thread). Returns a thumbnail.
    pub fn finish(mut self) -> Result<(PathBuf, Option<Image>), String> {
        self.clock.set_paused(false);
        self.stop.store(true, Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        let thumb = self.encoder.take().map(|e| e.join().unwrap_or(Err("encoder crashed".into()))).unwrap_or(Ok(None))?;
        Ok((self.path.clone(), thumb))
    }
}

fn capture_loop(area: Rect, fps: u32, cursor: bool, clock: Arc<Clock>, stop: Arc<AtomicBool>, frames: Arc<AtomicU64>, tx: SyncSender<Msg>) {
    unsafe {
        let _ = windows::Win32::Media::timeBeginPeriod(1);
    }
    let step = Duration::from_secs_f64(1.0 / fps as f64);
    let mut next = Instant::now();
    let mut last: Option<Duration> = None;
    // GPU duplication when the area sits on one monitor; GDI otherwise or if it fails.
    let mut dup = crate::dxgi::open_for(area);
    let mut buf = Image::new(area.w as u32, area.h as u32);
    let mut have_frame = false;
    let mut last_cursor = (i32::MIN, i32::MIN);
    let mut last_sent = Instant::now();
    while !stop.load(Relaxed) {
        if clock.paused() {
            std::thread::sleep(Duration::from_millis(15));
            next = Instant::now();
            continue;
        }
        let t = clock.elapsed();
        // Keep timestamps strictly increasing by at least one frame.
        if last.is_some_and(|l| t < l + step / 2) {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        let img = match dup.as_mut() {
            Some(d) => {
                let wait = next.saturating_duration_since(Instant::now()).as_millis().min(100) as u32;
                match d.acquire(wait, area) {
                    Ok(new) => {
                        // Nothing changed on screen (and the pointer didn't move): skip the frame;
                        // the previous one simply lasts longer. Keep one frame a second for seeking.
                        let cur = capture::cursor_pos();
                        let pointer_moved = cursor && cur != last_cursor;
                        if !new && have_frame && !pointer_moved && last_sent.elapsed() < Duration::from_secs(1) {
                            next += step;
                            let now = Instant::now();
                            if next > now {
                                std::thread::sleep(next - now);
                            } else {
                                next = now;
                            }
                            continue;
                        }
                        last_cursor = cur;
                        if new || !have_frame {
                            have_frame = d.read(area, &mut buf) || have_frame;
                        }
                    }
                    Err(_) => {
                        // Lost (mode change, secure desktop): reopen, GDI for this frame.
                        dup = crate::dxgi::open_for(area);
                        have_frame = false;
                    }
                }
                if have_frame {
                    let mut f = buf.clone();
                    if cursor {
                        capture::overlay_cursor(&mut f, area.x, area.y);
                    }
                    f
                } else {
                    capture::grab(area, cursor)
                }
            }
            None => capture::grab(area, cursor),
        };
        if tx.try_send(Msg::Video(img, t)).is_ok() {
            last_sent = Instant::now();
            frames.fetch_add(1, Relaxed);
            last = Some(t);
        }
        next += step;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    unsafe {
        let _ = windows::Win32::Media::timeEndPeriod(1);
    }
}

fn encode_mp4(rx: Receiver<Msg>, path: &std::path::Path, area: Rect, fps: u32, audio: bool, ready: SyncSender<Result<(), String>>) -> Result<Option<Image>, String> {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    let mp4 = match mp4::Mp4::new(path, area.w as u32, area.h as u32, fps, audio) {
        Ok(m) => {
            let _ = ready.send(Ok(()));
            m
        }
        Err(e) => {
            let _ = ready.send(Err(format!("Could not start the video encoder: {}", e.message())));
            return Err(e.message());
        }
    };
    let mut nv12 = Vec::new();
    let mut thumb: Option<Image> = None;
    let mut pending: Option<(Image, Duration)> = None;
    let frame = Duration::from_secs_f64(1.0 / fps as f64);
    let mut err = None;
    let mut flush = |img: &Image, t: Duration, next: Option<Duration>, nv12: &mut Vec<u8>| {
        mp4::bgra_to_nv12(&img.px, img.w as usize, img.h as usize, nv12);
        let dur = next.map(|n| n.saturating_sub(t)).unwrap_or(frame).max(Duration::from_millis(1));
        mp4.video(nv12, (t.as_nanos() / 100) as i64, (dur.as_nanos() / 100) as i64)
    };
    for m in rx {
        match m {
            Msg::Video(img, t) => {
                if thumb.is_none() {
                    thumb = Some(img.fit(640, 360));
                }
                if let Some((p, pt)) = pending.take() {
                    if let Err(e) = flush(&p, pt, Some(t), &mut nv12) {
                        err.get_or_insert(e.message());
                    }
                }
                pending = Some((img, t));
            }
            Msg::Audio(pcm, at) => {
                if let Err(e) = mp4.audio(&pcm, at) {
                    err.get_or_insert(e.message());
                }
            }
        }
    }
    if let Some((p, pt)) = pending.take() {
        let _ = flush(&p, pt, None, &mut nv12);
    }
    drop(flush);
    mp4.finish().map_err(|e| e.message())?;
    if let Some(e) = err {
        return Err(format!("Recording had errors: {e}"));
    }
    Ok(thumb)
}

fn encode_gif(rx: Receiver<Msg>, path: &std::path::Path, area: Rect, fps: u32, ready: SyncSender<Result<(), String>>) -> Result<Option<Image>, String> {
    let scale = (GIF_MAX_W as f64 / area.w as f64).min(1.0);
    let (gw, gh) = (((area.w as f64 * scale) as u32).max(2) as u16, ((area.h as f64 * scale) as u32).max(2) as u16);
    let file = match std::fs::File::create(path) {
        Ok(f) => {
            let _ = ready.send(Ok(()));
            f
        }
        Err(e) => {
            let _ = ready.send(Err(format!("Could not create the GIF: {e}")));
            return Err(e.to_string());
        }
    };
    let mut enc = gif::Encoder::new(std::io::BufWriter::new(file), gw, gh, &[]).map_err(|e| e.to_string())?;
    let _ = enc.set_repeat(gif::Repeat::Infinite);
    // Quantize frames on worker threads, write them in order.
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
    let (jtx, jrx) = std::sync::mpsc::channel::<(u64, Image, u16)>();
    let jrx = Arc::new(Mutex::new(jrx));
    let (ftx, frx) = std::sync::mpsc::channel::<(u64, gif::Frame<'static>)>();
    let mut pool = Vec::new();
    for _ in 0..workers {
        let (jrx, ftx) = (jrx.clone(), ftx.clone());
        pool.push(std::thread::spawn(move || {
            loop {
                let job = jrx.lock().unwrap().recv();
                let Ok((i, img, delay)) = job else { break };
                let mut rgba = img.to_rgba();
                let mut f = gif::Frame::from_rgba_speed(img.w as u16, img.h as u16, &mut rgba, 10);
                f.delay = delay;
                let _ = ftx.send((i, f));
            }
        }));
    }
    drop(ftx);
    let writer = std::thread::spawn(move || -> Result<(), String> {
        let mut next = 0u64;
        let mut buf = std::collections::BTreeMap::new();
        for (i, f) in frx {
            buf.insert(i, f);
            while let Some(f) = buf.remove(&next) {
                enc.write_frame(&f).map_err(|e| e.to_string())?;
                next += 1;
            }
        }
        Ok(())
    });
    let mut thumb = None;
    let mut idx = 0u64;
    let mut prev: Option<(Image, Duration)> = None;
    let send = |img: Image, cs: u16, idx: &mut u64| {
        let small = if scale < 1.0 { img.fit(gw as u32, gh as u32) } else { img };
        let _ = jtx.send((*idx, small, cs.max(2)));
        *idx += 1;
    };
    for m in rx {
        if let Msg::Video(img, t) = m {
            if thumb.is_none() {
                thumb = Some(img.fit(640, 360));
            }
            if let Some((p, pt)) = prev.take() {
                let cs = ((t.saturating_sub(pt)).as_millis() / 10) as u16;
                send(p, cs, &mut idx);
            }
            prev = Some((img, t));
        }
    }
    if let Some((p, _)) = prev.take() {
        send(p, (100 / fps) as u16, &mut idx);
    }
    drop(send);
    drop(jtx);
    for p in pool {
        let _ = p.join();
    }
    writer.join().map_err(|_| "GIF writer crashed".to_string())??;
    Ok(thumb)
}
