//! The resident process: hidden host window, tray icon, hotkey commands and the capture flows.

use crate::bar::{self, Item, Style};
use crate::capture::{self, Image, Rect, Shot};
use crate::config::{self, Config, RecFormat};
use crate::gfx::{col, icon};
use crate::hook::{self, Cmd};
use crate::overlay::{self, Mode};
use crate::toast::{self, Action, Content};
use crate::{output, record, scroll, win};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::atomic::Ordering::Relaxed;
use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::PCWSTR;

pub fn host_class() -> &'static str {
    if config::test_mode() { "FramegrabHostTest" } else { "FramegrabHost" }
}

enum Job {
    Saved(PathBuf),
    Ocr(Result<String, String>),
    RecDone(Result<(PathBuf, Option<Image>), String>),
    Msg(String),
}

/// Rec-bar button ids.
mod rb {
    pub const START: u32 = 1;
    pub const STOP: u32 = 2;
    pub const PAUSE: u32 = 3;
    pub const MIC: u32 = 4;
    pub const SYS: u32 = 5;
    pub const FORMAT: u32 = 6;
    pub const CANCEL: u32 = 7;
    pub const TIMER: u32 = 8;
    pub const SCROLL_STOP: u32 = 10;
    pub const SCROLL_INFO: u32 = 11;
}

enum Busy {
    Idle,
    Overlay,
    /// Area picked, waiting for Start.
    RecSetup(Rect),
    Recording(record::Recorder, Rect),
    Finishing,
    Scrolling(ScrollState),
}

struct ScrollState {
    area: Rect,
    st: scroll::Stitcher,
    notches: i32,
    since_scroll: u32,
    ticks: u32,
}

struct App {
    host: HWND,
    cfg: Config,
    busy: Busy,
    jobs_tx: Sender<Job>,
    jobs_rx: Receiver<Job>,
    /// Last capture shown in the toolbar (for Copy / Text / Edit without re-reading the file).
    last: Option<(Image, Option<PathBuf>)>,
    taskbar_created: u32,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut a| a.as_mut().map(f)))
}

/// Nothing in progress (safe to restart for an update).
pub fn is_idle() -> bool {
    with(|a| matches!(a.busy, Busy::Idle)).unwrap_or(false) && !bar::visible() && toast::path().is_none() && !overlay::active()
}

const TIMER_SCROLL: usize = 10;
const TIMER_REC: usize = 11;

pub fn run(background: bool) {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
    win::register_class(host_class(), Some(wndproc));
    let host = win::create(host_class(), "Framegrab", WS_OVERLAPPED, WS_EX_TOOLWINDOW, 0, 0, 0, 0, None);
    overlay::init(host);
    toast::init();
    bar::init();
    let cfg = Config::load();
    let (jobs_tx, jobs_rx) = channel();
    let taskbar_created = unsafe { RegisterWindowMessageW(PCWSTR(win::wide("TaskbarCreated").as_ptr())) };
    APP.with(|a| *a.borrow_mut() = Some(App { host, cfg: cfg.clone(), busy: Busy::Idle, jobs_tx, jobs_rx, last: None, taskbar_created }));
    apply_config(&cfg);
    tray_add(host);
    if !config::test_mode() || std::env::var_os("FRAMEGRAB_HOOK").is_some() {
        hook::start(host);
    }
    crate::updater::spawn_auto(host, cfg.auto_update);
    if !background && !config::test_mode() {
        notify(host, "Framegrab is running", "Press Print Screen or Win+Shift+S to capture.");
    }
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    tray_remove(host);
}

fn apply_config(cfg: &Config) {
    hook::PRTSC.store(cfg.hook_prtsc, Relaxed);
    hook::WIN_SHIFT_S.store(cfg.hook_win_shift_s, Relaxed);
    crate::autostart::apply(cfg.start_with_windows);
}

pub fn log(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(config::data_dir().join("framegrab.log")) {
        let _ = writeln!(f, "{} {msg}", config::stamp());
    }
}

fn notify(host: HWND, title: &str, text: &str) {
    log(&format!("{title}: {text}"));
    let mut nid = nid(host);
    nid.uFlags = NIF_INFO;
    copy_w(&mut nid.szInfoTitle, title);
    copy_w(&mut nid.szInfo, text);
    nid.dwInfoFlags = NIIF_NONE;
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

fn copy_w<const N: usize>(dst: &mut [u16; N], s: &str) {
    for (i, c) in s.encode_utf16().take(N - 1).enumerate() {
        dst[i] = c;
    }
}

fn nid(host: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: host, uID: 1, ..Default::default() }
}

fn tray_add(host: HWND) {
    let mut n = nid(host);
    n.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    n.uCallbackMessage = win::WM_TRAY;
    n.hIcon = unsafe { LoadIconW(Some(win::hinstance()), PCWSTR(1 as _)).or_else(|_| LoadIconW(None, IDI_APPLICATION)).unwrap_or_default() };
    copy_w(&mut n.szTip, if config::test_mode() { "Framegrab (TEST COPY)" } else { "Framegrab — Print Screen to capture" });
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &n);
    }
}

fn tray_remove(host: HWND) {
    let n = nid(host);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &n);
    }
}

fn tray_menu(host: HWND) {
    unsafe {
        let m = CreatePopupMenu().unwrap_or_default();
        let add = |id: usize, s: &str| {
            let w = win::wide(s);
            let _ = AppendMenuW(m, MF_STRING, id, PCWSTR(w.as_ptr()));
        };
        let busy_rec = with(|a| matches!(a.busy, Busy::Recording(..))).unwrap_or(false);
        if busy_rec {
            add(100, "Stop recording\tPrtSc");
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        }
        add(1, "Snip\tPrtSc");
        add(2, "Full screen\tShift+PrtSc");
        add(4, "Active window\tAlt+PrtSc");
        add(5, "Grab text\tCtrl+PrtSc");
        add(6, "Scrolling capture\tCtrl+Alt+PrtSc");
        add(7, "Record screen\tCtrl+Shift+PrtSc");
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        add(20, "Open screenshots folder");
        add(21, "Open recordings folder");
        add(22, "Edit an image…");
        add(23, "Settings");
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        add(99, "Quit Framegrab");
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let _ = SetForegroundWindow(host);
        let id = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, p.x, p.y, None, host, None);
        let _ = DestroyMenu(m);
        match id.0 as usize {
            0 => {}
            100 => command(Cmd::Record),
            v @ 1..=7 => {
                if let Some(c) = Cmd::from(v as u32) {
                    // Let the menu disappear before freezing the screen.
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    command(c);
                }
            }
            20 => open_folder(with(|a| a.cfg.shots_dir())),
            21 => open_folder(with(|a| a.cfg.recordings_dir())),
            22 => {
                if let Some(p) = rfd::FileDialog::new().add_filter("Images", &["png"]).pick_file() {
                    spawn_self(&["--edit", &p.to_string_lossy()]);
                }
            }
            23 => spawn_self(&["--settings"]),
            99 => quit(),
            _ => {}
        }
    }
}

fn open_folder(p: Option<PathBuf>) {
    if let Some(p) = p {
        let _ = std::process::Command::new("explorer").arg(p).spawn();
    }
}

fn reveal(p: &std::path::Path) {
    let _ = std::process::Command::new("explorer").arg(format!("/select,{}", p.display())).spawn();
}

pub fn spawn_self(args: &[&str]) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).args(args).spawn();
    }
}

fn quit() {
    let busy = with(|a| std::mem::replace(&mut a.busy, Busy::Idle));
    if let Some(Busy::Recording(r, _)) = busy {
        let _ = r.finish();
    }
    bar::hide();
    toast::hide();
    unsafe { PostQuitMessage(0) };
}

/// Entry point for hotkeys, tray items and test triggers.
pub fn command(cmd: Cmd) {
    let Some((busy_kind, cfg)) = with(|a| {
        let k = match &a.busy {
            Busy::Idle => 0,
            Busy::Overlay => 1,
            Busy::RecSetup(_) => 2,
            Busy::Recording(..) => 3,
            Busy::Finishing => 4,
            Busy::Scrolling(_) => 5,
        };
        (k, a.cfg.clone())
    }) else {
        return;
    };
    match busy_kind {
        1 => {
            // Pressing the hotkey again while selecting cancels.
            overlay::cancel();
            return;
        }
        2 if cmd == Cmd::Record || cmd == Cmd::Region => {
            rec_button(rb::START);
            return;
        }
        2 => {
            rec_button(rb::CANCEL);
        }
        3 => {
            rec_button(rb::STOP);
            return;
        }
        4 => return,
        5 => {
            scroll_finish();
            return;
        }
        _ => {}
    }
    toast::hide();
    match cmd {
        Cmd::Region => start_overlay(Mode::Region, &cfg),
        Cmd::Text => start_overlay(Mode::Text, &cfg),
        Cmd::Scroll => start_overlay(Mode::Scroll, &cfg),
        Cmd::Record => start_overlay(Mode::Record, &cfg),
        Cmd::Screen => {
            let shot = Shot::take(cfg.capture_cursor);
            let (cx, cy) = capture::cursor_pos();
            if let Some(i) = shot.monitor_at(cx, cy).or(Some(0)).filter(|&i| i < shot.imgs.len()) {
                deliver(shot.imgs[i].clone());
            }
        }
        Cmd::AllScreens => {
            let shot = Shot::take(cfg.capture_cursor);
            let r = capture::virtual_rect(&shot.mons);
            deliver(shot.crop(r));
        }
        Cmd::Window => {
            let shot = Shot::take(cfg.capture_cursor);
            if let Some(r) = capture::foreground_rect() {
                let vr = capture::virtual_rect(&shot.mons);
                if let Some(r) = r.intersect(&vr) {
                    deliver(shot.crop(r));
                }
            }
        }
    }
}

fn start_overlay(mode: Mode, cfg: &Config) {
    with(|a| a.busy = Busy::Overlay);
    hook::ROUTE.store(true, Relaxed);
    let t = std::time::Instant::now();
    overlay::show(mode, cfg);
    if config::test_mode() {
        log(&format!("overlay ready in {} ms", t.elapsed().as_millis()));
    }
}

fn overlay_done() {
    hook::ROUTE.store(false, Relaxed);
    let Some(done) = overlay::take_result() else {
        with(|a| a.busy = Busy::Idle);
        return;
    };
    with(|a| a.busy = Busy::Idle);
    let Some(rect) = done.rect else { return };
    match done.mode {
        Mode::Region | Mode::Screen => deliver(done.shot.crop(rect)),
        Mode::Text => {
            let img = done.shot.crop(rect);
            run_ocr(img);
        }
        Mode::Scroll => scroll_start(rect, done.shot),
        Mode::Record => rec_setup(rect),
    }
}

fn job(j: Job) {
    if let Some((tx, host)) = with(|a| (a.jobs_tx.clone(), a.host)) {
        let _ = tx.send(j);
        win::post(host, win::WM_JOB, 0, 0);
    }
}

/// A job sender + host handle that can move to worker threads.
#[derive(Clone)]
struct JobTx(Sender<Job>, isize);
impl JobTx {
    fn send(&self, j: Job) {
        let _ = self.0.send(j);
        win::post(HWND(self.1 as _), win::WM_JOB, 0, 0);
    }
}

fn jobs_tx() -> Option<JobTx> {
    with(|a| JobTx(a.jobs_tx.clone(), a.host.0 as isize))
}

/// Clipboard + save + toolbar for a finished image.
fn deliver(img: Image) {
    let Some((cfg, host)) = with(|a| (a.cfg.clone(), a.host)) else { return };
    if cfg.copy_to_clipboard {
        output::copy_image(&img);
    }
    let path = cfg.save_to_disk.then(|| config::unique_path(&cfg.shots_dir(), "Screenshot", "png"));
    let status = match (cfg.copy_to_clipboard, cfg.save_to_disk) {
        (true, true) => "Copied and saved",
        (true, false) => "Copied to clipboard",
        (false, true) => "Saved to Screenshots",
        (false, false) => "Captured",
    };
    with(|a| a.last = Some((img.clone(), path.clone())));
    if let Some(p) = path.clone() {
        let save = img.clone();
        let tx = jobs_tx();
        let open = cfg.open_editor;
        std::thread::spawn(move || {
            if output::write_png(&p, &save, false).is_ok() {
                if let Some(tx) = tx {
                    tx.send(Job::Saved(p.clone()));
                }
                if open {
                    spawn_self(&["--edit", &p.to_string_lossy()]);
                }
            }
        });
    } else if cfg.open_editor {
        edit_last();
        return;
    }
    if cfg.show_toolbar && !cfg.open_editor {
        let thumb = img.fit(700, 400);
        toast::show(host, Content::Image { thumb, size: (img.w, img.h), status: status.into() }, path, cfg.toolbar_seconds);
    }
}

fn run_ocr(img: Image) {
    let lang = with(|a| a.cfg.ocr_language.clone()).unwrap_or_default();
    let tx = jobs_tx();
    std::thread::spawn(move || {
        let r = crate::ocr::recognize(&img, &lang);
        if let Some(tx) = tx {
            tx.send(Job::Ocr(r));
        }
    });
}

fn edit_last() {
    let Some((img, path)) = with(|a| a.last.clone()).flatten() else { return };
    let p = match path.filter(|p| p.exists()) {
        Some(p) => p,
        None => {
            let p = std::env::temp_dir().join(format!("Framegrab {}.png", config::stamp()));
            if output::write_png(&p, &img, true).is_err() {
                return;
            }
            p
        }
    };
    spawn_self(&["--edit", &p.to_string_lossy()]);
}

fn toast_action(a: Action) {
    match a {
        Action::Edit => {
            toast::dismiss();
            edit_last();
        }
        Action::Text => {
            toast::dismiss();
            if let Some((img, _)) = with(|a| a.last.clone()).flatten() {
                run_ocr(img);
            }
        }
        Action::Copy => {
            if let Some((img, _)) = with(|a| a.last.clone()).flatten() {
                output::copy_image(&img);
            } else if let Some(t) = LAST_TEXT.with(|t| t.borrow().clone()) {
                output::copy_text(&t);
            }
            toast::dismiss();
        }
        Action::Folder => {
            if let Some(p) = toast::path() {
                reveal(&p);
            } else {
                open_folder(with(|a| a.cfg.shots_dir()));
            }
            toast::dismiss();
        }
        Action::Delete => {
            if let Some(p) = toast::path() {
                let _ = trash(&p);
            }
            with(|a| a.last = None);
            toast::dismiss();
        }
        Action::Close => toast::dismiss(),
    }
}

thread_local! {
    static LAST_TEXT: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Moves a file to the Recycle Bin.
fn trash(p: &std::path::Path) -> Result<(), String> {
    let mut from: Vec<u16> = p.as_os_str().encode_wide_vec();
    from.push(0);
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
        ..Default::default()
    };
    let r = unsafe { SHFileOperationW(&mut op) };
    if r == 0 { Ok(()) } else { Err(format!("delete failed ({r})")) }
}

trait WideVec {
    fn encode_wide_vec(&self) -> Vec<u16>;
}
impl WideVec for std::ffi::OsStr {
    fn encode_wide_vec(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().collect()
    }
}

// ---------- scrolling capture ----------

fn scroll_items(height: u32, frames: u32) -> Vec<Item> {
    vec![
        Item { id: rb::SCROLL_INFO, icon: "", label: format!("Scrolling…  {} px  ·  {} frames", height, frames), style: Style::Label, tip: "" },
        Item { id: rb::SCROLL_STOP, icon: icon::CHECK, label: "Done".into(), style: Style::On, tip: "" },
        Item { id: rb::CANCEL, icon: icon::X, label: String::new(), style: Style::Danger, tip: "" },
    ]
}

fn scroll_start(area: Rect, shot: Shot) {
    let first = shot.crop(area);
    let host = with(|a| a.host).unwrap_or_default();
    bar::show(host, area, col::ACCENT, scroll_items(first.h, 1));
    with(|a| a.busy = Busy::Scrolling(ScrollState { area, st: scroll::Stitcher::new(first), notches: 1, since_scroll: 0, ticks: 0 }));
    hook::ROUTE.store(true, Relaxed);
    scroll::wheel(area, -1);
    unsafe {
        SetTimer(Some(host), TIMER_SCROLL, 70, None);
    }
}

/// Called every 70 ms: waits for the page to settle after a wheel step, then grabs and stitches.
fn scroll_tick() {
    let mut finish = false;
    let mut update = None;
    with(|a| {
        let Busy::Scrolling(s) = &mut a.busy else { return };
        s.ticks += 1;
        s.since_scroll += 1;
        // Smooth scrolling animates for ~200 ms.
        if s.since_scroll < 4 {
            return;
        }
        let frame = capture::grab(s.area, false);
        let before = s.st.out.h;
        let moved = s.st.push(frame);
        if moved {
            let gained = s.st.out.h - before;
            // Aim for ~half the area per step.
            if gained > 0 && (gained as i32) < s.area.h / 4 && s.notches < 6 {
                s.notches += 1;
            } else if gained as i32 > s.area.h * 3 / 4 && s.notches > 1 {
                s.notches -= 1;
            }
            update = Some((s.st.out.h, s.st.frames));
        }
        if s.st.stalls >= 2 || s.st.out.h >= scroll::MAX_HEIGHT || s.ticks > 2000 {
            finish = true;
            return;
        }
        scroll::wheel(s.area, -s.notches);
        s.since_scroll = 0;
    });
    if let Some((h, f)) = update {
        bar::update(scroll_items(h, f));
    }
    if finish {
        scroll_finish();
    }
}

fn scroll_finish() {
    let host = with(|a| a.host).unwrap_or_default();
    unsafe {
        let _ = KillTimer(Some(host), TIMER_SCROLL);
    }
    hook::ROUTE.store(false, Relaxed);
    bar::hide();
    let st = with(|a| match std::mem::replace(&mut a.busy, Busy::Idle) {
        Busy::Scrolling(s) => Some(s.st),
        other => {
            a.busy = other;
            None
        }
    })
    .flatten();
    if let Some(st) = st {
        deliver(st.finish());
    }
}

// ---------- recording ----------

fn rec_items(cfg: &Config, recording: Option<(&str, bool)>) -> Vec<Item> {
    let mut v = Vec::new();
    let mp4 = cfg.rec_format == RecFormat::Mp4;
    match recording {
        None => {
            v.push(Item { id: rb::START, icon: icon::RECORD, label: "Start".into(), style: Style::Primary, tip: "" });
            v.push(Item { id: rb::FORMAT, icon: "", label: if mp4 { "MP4".into() } else { "GIF".into() }, style: Style::Normal, tip: "" });
        }
        Some((time, paused)) => {
            v.push(Item { id: rb::TIMER, icon: "", label: format!("● {time}"), style: Style::Label, tip: "" });
            v.push(Item { id: rb::PAUSE, icon: if paused { icon::PLAY } else { icon::PAUSE }, label: String::new(), style: if paused { Style::On } else { Style::Normal }, tip: "" });
            v.push(Item { id: rb::STOP, icon: icon::STOP, label: "Stop".into(), style: Style::Primary, tip: "" });
        }
    }
    if mp4 {
        v.push(Item { id: rb::SYS, icon: if cfg.rec_system_audio { icon::SPEAKER } else { icon::SPEAKER_X }, label: String::new(), style: if cfg.rec_system_audio { Style::On } else { Style::Normal }, tip: "System sound" });
        v.push(Item { id: rb::MIC, icon: if cfg.rec_mic { icon::MIC } else { icon::MIC_SLASH }, label: String::new(), style: if cfg.rec_mic { Style::On } else { Style::Normal }, tip: "Microphone" });
    }
    if recording.is_none() {
        v.push(Item { id: rb::CANCEL, icon: icon::X, label: String::new(), style: Style::Danger, tip: "" });
    }
    v
}

fn rec_setup(area: Rect) {
    let Some((host, cfg)) = with(|a| (a.host, a.cfg.clone())) else { return };
    with(|a| a.busy = Busy::RecSetup(area));
    bar::show(host, area, col::REC, rec_items(&cfg, None));
}

fn fmt_time(d: std::time::Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn rec_button(id: u32) {
    let Some((host, mut cfg)) = with(|a| (a.host, a.cfg.clone())) else { return };
    match id {
        rb::MIC | rb::SYS | rb::FORMAT => {
            let recording = with(|a| matches!(a.busy, Busy::Recording(..))).unwrap_or(false);
            if recording {
                return; // audio sources are fixed once recording starts
            }
            match id {
                rb::MIC => cfg.rec_mic = !cfg.rec_mic,
                rb::SYS => cfg.rec_system_audio = !cfg.rec_system_audio,
                _ => cfg.rec_format = if cfg.rec_format == RecFormat::Mp4 { RecFormat::Gif } else { RecFormat::Mp4 },
            }
            cfg.save();
            with(|a| a.cfg = cfg.clone());
            let area = with(|a| if let Busy::RecSetup(r) = a.busy { Some(r) } else { None }).flatten();
            if let Some(area) = area {
                // Item count can change (GIF has no audio), so rebuild the bar.
                bar::show(host, area, col::REC, rec_items(&cfg, None));
            }
        }
        rb::CANCEL => {
            bar::hide();
            with(|a| {
                if matches!(a.busy, Busy::RecSetup(_) | Busy::Scrolling(_)) {
                    a.busy = Busy::Idle;
                }
            });
            unsafe {
                let _ = KillTimer(Some(host), TIMER_SCROLL);
            }
            hook::ROUTE.store(false, Relaxed);
        }
        rb::SCROLL_STOP => scroll_finish(),
        rb::START => {
            let Some(area) = with(|a| if let Busy::RecSetup(r) = a.busy { Some(r) } else { None }).flatten() else { return };
            let ext = if cfg.rec_format == RecFormat::Mp4 { "mp4" } else { "gif" };
            let path = config::unique_path(&cfg.recordings_dir(), "Screen Recording", ext);
            let opts = record::Opts {
                area,
                fps: cfg.rec_fps,
                gif_fps: cfg.gif_fps,
                format: cfg.rec_format,
                system_audio: cfg.rec_system_audio,
                mic: cfg.rec_mic,
                cursor: cfg.rec_cursor,
                path,
            };
            match record::Recorder::start(opts) {
                Ok(r) => {
                    with(|a| a.busy = Busy::Recording(r, area));
                    bar::show(host, area, col::REC, rec_items(&cfg, Some(("00:00", false))));
                    unsafe {
                        SetTimer(Some(host), TIMER_REC, 250, None);
                    }
                }
                Err(e) => {
                    bar::hide();
                    with(|a| a.busy = Busy::Idle);
                    notify(host, "Recording failed", &e);
                }
            }
        }
        rb::PAUSE => {
            let paused = with(|a| {
                if let Busy::Recording(r, _) = &a.busy {
                    let p = !r.clock.paused();
                    r.clock.set_paused(p);
                    Some((p, fmt_time(r.clock.elapsed())))
                } else {
                    None
                }
            })
            .flatten();
            if let Some((p, t)) = paused {
                bar::update(rec_items(&cfg, Some((&t, p))));
            }
        }
        rb::STOP => {
            unsafe {
                let _ = KillTimer(Some(host), TIMER_REC);
            }
            bar::hide();
            let rec = with(|a| match std::mem::replace(&mut a.busy, Busy::Finishing) {
                Busy::Recording(r, _) => Some(r),
                other => {
                    a.busy = other;
                    None
                }
            })
            .flatten();
            if let Some(r) = rec {
                let tx = jobs_tx();
                let aerr = r.audio_error.clone();
                std::thread::spawn(move || {
                    let res = r.finish();
                    if let Some(tx) = tx {
                        if let Some(e) = aerr.lock().unwrap().take() {
                            tx.send(Job::Msg(format!("Audio was not recorded ({e})")));
                        }
                        tx.send(Job::RecDone(res));
                    }
                });
            } else {
                with(|a| a.busy = Busy::Idle);
            }
        }
        _ => {}
    }
}

fn rec_tick() {
    let info = with(|a| if let Busy::Recording(r, _) = &a.busy { Some((fmt_time(r.clock.elapsed()), r.clock.paused(), a.cfg.clone())) } else { None }).flatten();
    if let Some((t, p, cfg)) = info {
        bar::update(rec_items(&cfg, Some((&t, p))));
    }
}

fn handle_jobs() {
    loop {
        let Some(j) = with(|a| a.jobs_rx.try_recv().ok()).flatten() else { break };
        let Some((host, cfg)) = with(|a| (a.host, a.cfg.clone())) else { return };
        match j {
            Job::Saved(_) => {}
            Job::Msg(m) => notify(host, "Framegrab", &m),
            Job::Ocr(Ok(text)) => {
                let t = text.trim().to_string();
                if t.is_empty() {
                    toast::show(host, Content::Text { text: "No text found in that area.".into(), status: "Nothing to copy".into() }, None, cfg.toolbar_seconds.max(3));
                } else {
                    output::copy_text(&t);
                    LAST_TEXT.with(|l| *l.borrow_mut() = Some(t.clone()));
                    with(|a| a.last = None);
                    let n = t.chars().count();
                    toast::show(host, Content::Text { text: t, status: format!("Copied {n} characters") }, None, cfg.toolbar_seconds);
                }
            }
            Job::Ocr(Err(e)) => toast::show(host, Content::Text { text: e, status: "Couldn't read text".into() }, None, cfg.toolbar_seconds.max(4)),
            Job::RecDone(res) => {
                with(|a| {
                    if matches!(a.busy, Busy::Finishing) {
                        a.busy = Busy::Idle;
                    }
                });
                match res {
                    Ok((path, thumb)) => {
                        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                        let detail = if size > 10 << 20 { format!("{:.0} MB", size as f64 / 1048576.0) } else { format!("{:.1} MB", size as f64 / 1048576.0) };
                        toast::show(host, Content::Video { thumb, status: "Recording saved".into(), detail }, Some(path), cfg.toolbar_seconds.max(5));
                    }
                    Err(e) => notify(host, "Recording failed", &e),
                }
            }
        }
    }
}

/// WM_COPYDATA commands from `framegrab --trigger …` and the settings window.
fn copydata(s: &str) {
    let mut it = s.splitn(2, ' ');
    let head = it.next().unwrap_or("");
    let rest = it.next().unwrap_or("").trim();
    match head {
        "reload" => {
            let cfg = Config::load();
            apply_config(&cfg);
            with(|a| a.cfg = cfg);
        }
        "quit" => quit(),
        "sim" => overlay::sim(rest),
        "key" => {
            if let Ok(vk) = rest.parse::<u32>() {
                route_key(vk);
            }
        }
        "dump" => {
            let files = overlay::dump(std::path::Path::new(rest));
            let _ = std::fs::write(std::path::Path::new(rest).join("dump.txt"), files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n"));
        }
        "deliver-test" => {
            // Shows the toolbar for an existing PNG.
            if let Ok(img) = output::read_png(std::path::Path::new(rest)) {
                deliver(img);
            }
        }
        "ocr-file" => {
            if let Ok(img) = output::read_png(std::path::Path::new(rest)) {
                run_ocr(img);
            }
        }
        "scroll" => {
            // scroll x y w h — start a scrolling capture over an area directly.
            let v: Vec<i32> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            if v.len() == 4 {
                scroll_start(Rect::new(v[0], v[1], v[2], v[3]), Shot::take(false));
            }
        }
        "record" => {
            let v: Vec<i32> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            if v.len() == 4 {
                rec_setup(Rect::new(v[0], v[1], v[2], v[3]));
            }
        }
        "rec-button" => {
            if let Ok(id) = rest.parse::<u32>() {
                rec_button(id);
            }
        }
        "toast" => {
            if let Ok(id) = rest.parse::<usize>() {
                if let Some(a) = Action::from(id) {
                    toast_action(a);
                }
            }
        }
        _ => {
            if let Some(c) = Cmd::parse(head) {
                command(c);
            }
        }
    }
}

fn route_key(vk: u32) {
    if overlay::active() {
        overlay::key(vk);
        return;
    }
    let state = with(|a| match a.busy {
        Busy::Scrolling(_) => 1,
        Busy::RecSetup(_) => 2,
        _ => 0,
    })
    .unwrap_or(0);
    let esc = vk == 0x1B;
    let enter = vk == 0x0D || vk == 0x20;
    match state {
        1 if esc || enter => scroll_finish(),
        2 if esc => rec_button(rb::CANCEL),
        2 if enter => rec_button(rb::START),
        _ => hook::ROUTE.store(false, Relaxed),
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let tbc = with(|a| a.taskbar_created).unwrap_or(0);
        if msg == tbc && tbc != 0 {
            tray_add(h);
            return LRESULT(0);
        }
        match msg {
            win::WM_CMD => {
                if let Some(c) = Cmd::from(wp.0 as u32) {
                    command(c);
                }
                LRESULT(0)
            }
            win::WM_OVERLAY_DONE => {
                overlay_done();
                LRESULT(0)
            }
            win::WM_KEYROUTE => {
                route_key(wp.0 as u32);
                LRESULT(0)
            }
            win::WM_JOB => {
                handle_jobs();
                LRESULT(0)
            }
            win::WM_TOAST => {
                if let Some(a) = Action::from(wp.0) {
                    toast_action(a);
                }
                LRESULT(0)
            }
            win::WM_REC => {
                rec_button(wp.0 as u32);
                LRESULT(0)
            }
            WM_TIMER => {
                match wp.0 {
                    TIMER_SCROLL => scroll_tick(),
                    TIMER_REC => rec_tick(),
                    crate::updater::TIMER_ID => crate::updater::on_timer(h),
                    _ => {}
                }
                LRESULT(0)
            }
            win::WM_TRAY => {
                match (lp.0 as u32) & 0xFFFF {
                    WM_LBUTTONUP => command(Cmd::Region),
                    WM_RBUTTONUP | WM_CONTEXTMENU => tray_menu(h),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_COPYDATA => {
                let cds = &*(lp.0 as *const COPYDATASTRUCT);
                if cds.dwData == 0x4647 && !cds.lpData.is_null() {
                    let bytes = std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize);
                    let s = String::from_utf8_lossy(bytes).to_string();
                    // Run after returning so the sender isn't blocked by long work.
                    PENDING.with(|p| p.borrow_mut().push(s));
                    win::post(h, WM_APP + 50, 0, 0);
                }
                LRESULT(1)
            }
            m if m == WM_APP + 50 => {
                let items: Vec<String> = PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()));
                for s in items {
                    copydata(&s);
                }
                LRESULT(0)
            }
            WM_DISPLAYCHANGE | WM_DPICHANGED => {
                overlay::displays_changed();
                LRESULT(0)
            }
            WM_ENDSESSION => {
                quit();
                LRESULT(0)
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

thread_local! {
    static PENDING: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Sends a command string to the running copy. Returns false when none is running.
pub fn send_to_running(cmd: &str) -> bool {
    unsafe {
        let cls = win::wide(host_class());
        let Ok(h) = FindWindowW(PCWSTR(cls.as_ptr()), None) else { return false };
        if h.0.is_null() {
            return false;
        }
        let cds = COPYDATASTRUCT { dwData: 0x4647, cbData: cmd.len() as u32, lpData: cmd.as_ptr() as _ };
        let _ = SendMessageW(h, WM_COPYDATA, None, Some(LPARAM(&cds as *const _ as isize)));
        true
    }
}
