//! The resident process: hidden host window (no tray icon), hotkey commands and the capture flows.
//! Launching Framegrab again opens Settings; Quit and Pause live there.

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
    /// Processed capture is on the clipboard; show the card.
    Ready { raw: Image, thumb: Image, size: (u32, u32), path: Option<PathBuf>, status: String },
    Saved(PathBuf),
    Uploaded(Result<String, String>),
    Ocr(Result<crate::ocr::OcrOut, String>),
    RecDone(Result<(PathBuf, Option<Image>), String>),
    Msg(String),
}

/// Where a capture came from (file names, window processing, flash).
#[derive(Default, Clone)]
pub struct Meta {
    pub mode: &'static str,
    /// Clicked window: re-capture it with PrintWindow / transparent corners.
    pub window: Option<isize>,
    /// Window the capture is "of" (app name + title).
    pub app_window: Option<isize>,
    pub rect: Option<Rect>,
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
    /// Where the last capture was on screen (a pin appears right there).
    last_rect: Option<Rect>,
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
const TIMER_TRIM: usize = 12;
const TIMER_GAME: usize = 13;
const TIMER_PREWARM: usize = 14;

/// After a capture: once everything is closed, drop the cached image and hand memory back.
fn schedule_trim() {
    // Wait a while: captures in a burst stay at full speed, and an idle PC still gets the
    // memory back (paging it in again costs ~30 ms on the next capture).
    schedule_trim_in(30_000);
}

fn schedule_trim_in(ms: u32) {
    if let Some(h) = with(|a| a.host) {
        unsafe {
            SetTimer(Some(h), TIMER_TRIM, ms, None);
        }
    }
}

fn trim_tick(h: HWND) {
    if !is_idle() {
        return;
    }
    unsafe {
        let _ = KillTimer(Some(h), TIMER_TRIM);
    }
    with(|a| a.last = None);
    overlay::release_memory();
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

pub fn run(background: bool) {
    let t0 = std::time::Instant::now();
    unsafe {
        // OLE (not just COM) so the toolbar card can start drag-and-drop.
        let _ = windows::Win32::System::Ole::OleInitialize(None);
    }
    win::register_class(host_class(), Some(wndproc));
    let host = win::create(host_class(), "Framegrab", WS_OVERLAPPED, WS_EX_TOOLWINDOW, 0, 0, 0, 0, None);
    overlay::init(host);
    toast::init();
    bar::init();
    crate::feedback::init();
    unsafe {
        SetTimer(Some(host), TIMER_GAME, 2000, None);
    }
    let cfg = Config::load();
    let (jobs_tx, jobs_rx) = channel();
    APP.with(|a| *a.borrow_mut() = Some(App { host, cfg: cfg.clone(), busy: Busy::Idle, jobs_tx, jobs_rx, last: None, last_rect: None }));
    apply_config(&cfg);
    write_status();
    if !config::test_mode() || std::env::var_os("FRAMEGRAB_HOOK").is_some() {
        hook::start(host);
    }
    crate::updater::spawn_auto(host, cfg.auto_update);
    log(&format!("started in {} ms", t0.elapsed().as_millis()));
    // Build the overlay windows a moment later so start-up (and sign-in) isn't held up.
    unsafe {
        SetTimer(Some(host), TIMER_PREWARM, 1500, None);
    }
    if !background && !config::test_mode() {
        notify(host, "Framegrab is running", "Press Print Screen to capture. Open Framegrab again for Settings.");
    }
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Lets Settings show whether hotkeys are paused (a session switch, not saved).
fn write_status() {
    let _ = std::fs::write(config::data_dir().join("status"), if hook::PAUSED.load(Relaxed) { "paused" } else { "running" });
}

fn apply_config(cfg: &Config) {
    hook::set_bindings(cfg);
    if !cfg.pause_in_fullscreen {
        hook::GAME.store(false, Relaxed);
    }
    crate::autostart::apply(cfg.start_with_windows);
}

/// Every 2 s: pause hotkeys while an exclusive full-screen game is in front (if enabled).
fn game_tick() {
    if !with(|a| a.cfg.pause_in_fullscreen).unwrap_or(false) {
        return;
    }
    let state = unsafe { SHQueryUserNotificationState() };
    let game = state.is_ok_and(|s| s == QUNS_RUNNING_D3D_FULL_SCREEN);
    hook::GAME.store(game, Relaxed);
}

/// The "choose what to capture" popup (bindable to a hotkey, e.g. Print Screen).
fn capture_menu(host: HWND) {
    unsafe {
        let m = CreatePopupMenu().unwrap_or_default();
        let hk = with(|a| a.cfg.hotkeys.clone()).unwrap_or_default();
        let items = [
            (1usize, "&Snip", hk.snip),
            (2, "&Full screen", hk.screen),
            (4, "Active &window", hk.window),
            (3, "&All monitors", hk.all),
            (5, "Grab &text", hk.text),
            (6, "Scrolling &capture", hk.scroll),
            (7, "&Record screen", hk.record),
        ];
        for (id, label, k) in items {
            let s = if k.is_set() { format!("{label}\t{}", k.label()) } else { label.to_string() };
            let w = win::wide(&s);
            let _ = AppendMenuW(m, MF_STRING, id, PCWSTR(w.as_ptr()));
        }
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        win::force_foreground(host);
        let id = TrackPopupMenu(m, TPM_RETURNCMD | TPM_LEFTBUTTON, p.x, p.y, None, host, None);
        let _ = DestroyMenu(m);
        if let Some(c) = Cmd::from(id.0 as u32).filter(|c| *c != Cmd::Menu) {
            std::thread::sleep(std::time::Duration::from_millis(120));
            command(c);
        }
    }
}

pub fn log(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(config::data_dir().join("framegrab.log")) {
        let _ = writeln!(f, "{} {msg}", config::stamp());
    }
}

/// Messages show on the toolbar card (there is no tray icon).
fn notify(host: HWND, title: &str, text: &str) {
    log(&format!("{title}: {text}"));
    toast::show(host, toast::Content::Text { text: text.to_string(), status: title.to_string() }, None, 6);
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

/// Entry point for hotkeys, the capture menu and test triggers.
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
        Cmd::Menu => {
            let host = with(|a| a.host).unwrap_or_default();
            capture_menu(host);
        }
        Cmd::Screen => {
            let shot = Shot::take(cfg.capture_cursor);
            let (cx, cy) = capture::cursor_pos();
            if let Some(i) = shot.monitor_at(cx, cy).or(Some(0)).filter(|&i| i < shot.imgs.len()) {
                let fg = unsafe { GetForegroundWindow() };
                let meta = Meta { mode: "Screen", app_window: (!fg.0.is_null()).then_some(fg.0 as isize), rect: Some(shot.mons[i].rect), ..Default::default() };
                deliver(shot.imgs[i].clone(), meta);
            }
        }
        Cmd::AllScreens => {
            let shot = Shot::take(cfg.capture_cursor);
            let r = capture::virtual_rect(&shot.mons);
            deliver(shot.crop(r), Meta { mode: "Screens", rect: Some(r), ..Default::default() });
        }
        Cmd::Window => {
            let fg = unsafe { GetForegroundWindow() };
            let shot = Shot::take(cfg.capture_cursor);
            if let Some(r) = capture::foreground_rect() {
                let vr = capture::virtual_rect(&shot.mons);
                if let Some(r) = r.intersect(&vr) {
                    let h = (!fg.0.is_null()).then_some(fg.0 as isize);
                    deliver(shot.crop(r), Meta { mode: "Window", window: h, app_window: h, rect: Some(r) });
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
    schedule_trim();
    let Some(done) = overlay::take_result() else {
        with(|a| a.busy = Busy::Idle);
        return;
    };
    with(|a| a.busy = Busy::Idle);
    let Some(rect) = done.rect else {
        log("overlay closed without a selection");
        return;
    };
    log(&format!("selected {:?} {rect:?} window={}", done.mode, done.window.is_some()));
    match done.mode {
        Mode::Region | Mode::Screen => {
            let meta = Meta {
                mode: if done.window.is_some() { "Window" } else if done.mode == Mode::Screen { "Screen" } else { "Snip" },
                window: done.window,
                app_window: done.under,
                rect: Some(rect),
            };
            deliver(done.shot.crop(rect), meta)
        }
        Mode::Text => {
            let img = done.shot.crop(rect);
            run_ocr(img);
        }
        Mode::Scroll => scroll_start(rect, done.shot),
        Mode::Record => rec_setup(rect),
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

/// The capture pipeline. Everything optional (window re-capture, transparent corners, beautify,
/// format, upload) comes from Settings and runs on a worker thread, so nothing waits on the user.
fn deliver(img: Image, meta: Meta) {
    schedule_trim();
    let Some((cfg, host)) = with(|a| {
        a.last_rect = meta.rect;
        (a.cfg.clone(), a.host)
    }) else {
        return;
    };
    if cfg.sound {
        crate::feedback::sound();
    }
    if cfg.flash {
        if let Some(r) = meta.rect {
            crate::feedback::flash(r);
        }
    }
    // Nothing to process: copy right away on this thread (fastest path).
    let window_work = meta.window.is_some() && (cfg.window_full_content || cfg.window_transparent);
    let copied_now = cfg.copy_to_clipboard && !window_work && !cfg.beautify;
    if copied_now {
        output::copy_image(&img);
    }
    let (app, title) = meta.app_window.map(|h| (crate::winfo::app_name(h), crate::winfo::title(h))).unwrap_or_default();
    let tx = jobs_tx();
    std::thread::spawn(move || {
        let mut raw = img;
        if let Some(h) = meta.window {
            if cfg.window_full_content {
                if let Some(r) = meta.rect {
                    let t = std::time::Instant::now();
                    let got = crate::winfo::print_window(h, r);
                    log(&format!("  window content {} in {} ms", if got.is_some() { "captured" } else { "unavailable, kept screen pixels" }, t.elapsed().as_millis()));
                    if let Some(w) = got {
                        raw = w;
                    }
                }
            }
            if cfg.window_transparent && !crate::winfo::is_maximized(h) {
                let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(HWND(h as _)) }.max(96);
                crate::winfo::round_corners(&mut raw, 8.0 * dpi as f32 / 96.0);
            }
        }
        let fin = if cfg.beautify { crate::process::beautify(&raw, &crate::process::Beautify::from(&cfg)) } else { raw.clone() };
        if cfg.copy_to_clipboard && !copied_now {
            output::copy_image(&fin);
        }
        let ext = cfg.format.ext();
        let info = config::NameInfo { app, title, mode: meta.mode.to_string(), w: fin.w, h: fin.h };
        let path = cfg.save_to_disk.then(|| config::template_path(&cfg.shots_dir(), &cfg.name_template, &info, cfg.app_subfolders, ext));
        let status = match (cfg.copy_to_clipboard, cfg.save_to_disk) {
            (true, true) => "Copied and saved",
            (true, false) => "Copied to clipboard",
            (false, true) => "Saved",
            (false, false) => "Captured",
        };
        if let Some(tx) = &tx {
            tx.send(Job::Ready { raw: raw.clone(), thumb: fin.fit(700, 400), size: (fin.w, fin.h), path: path.clone(), status: status.into() });
        }
        let mut bytes = None;
        if let Some(p) = &path {
            match output::write_image(p, &fin, cfg.format, cfg.quality) {
                Ok(b) => {
                    bytes = Some(b);
                    if let Some(tx) = &tx {
                        tx.send(Job::Saved(p.clone()));
                    }
                    if cfg.open_editor {
                        let ps = p.to_string_lossy().to_string();
                        if ext == "png" && !cfg.beautify {
                            spawn_self(&["--edit", &ps]);
                        } else if let Some(t) = temp_png(&raw) {
                            spawn_self(&["--edit", &t.to_string_lossy(), "--out", &ps]);
                        }
                    }
                }
                Err(e) => log(&format!("save failed: {e}")),
            }
        }
        let saved = bytes.is_some();
        if cfg.upload.provider != config::UploadProvider::Off && cfg.upload.auto {
            let b = bytes.or_else(|| output::encode(&fin, cfg.format, cfg.quality).ok());
            if let (Some(b), Some(tx)) = (b, &tx) {
                let name = path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| format!("screenshot.{ext}"));
                tx.send(Job::Uploaded(crate::upload::upload(&cfg.upload, &b, &name, ext)));
            }
        }
        crate::history::add(&fin, path.as_deref().filter(|_| saved));
    });
    let _ = host;
}

fn temp_png(img: &Image) -> Option<PathBuf> {
    let p = std::env::temp_dir().join(format!("Framegrab edit {}.png", config::stamp()));
    output::write_png(&p, img, true).ok().map(|_| p)
}

fn run_ocr(img: Image) {
    let cfg = with(|a| a.cfg.clone()).unwrap_or_default();
    let tx = jobs_tx();
    std::thread::spawn(move || {
        let r = crate::ocr::grab_text(&img, &cfg);
        if let Some(tx) = tx {
            tx.send(Job::Ocr(r));
        }
    });
}

/// Opens the editor on the last capture. Non-PNG / beautified files are edited from a clean
/// PNG copy and written back to the real file in its format.
fn edit_last() {
    let Some((img, path)) = with(|a| a.last.clone()).flatten() else { return };
    let cfg = with(|a| a.cfg.clone()).unwrap_or_default();
    let plain_png = path.as_ref().is_some_and(|p| p.exists() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"))) && !cfg.beautify;
    if plain_png {
        spawn_self(&["--edit", &path.unwrap().to_string_lossy()]);
        return;
    }
    let Some(t) = temp_png(&img) else { return };
    match path {
        Some(p) => spawn_self(&["--edit", &t.to_string_lossy(), "--out", &p.to_string_lossy()]),
        None => spawn_self(&["--edit", &t.to_string_lossy()]),
    }
}

/// File for the last capture (saved one, else a temp PNG) — for drag-out.
pub fn last_file() -> Option<PathBuf> {
    if let Some(p) = toast::path().filter(|p| p.exists()) {
        return Some(p);
    }
    let (img, _) = with(|a| a.last.clone()).flatten()?;
    temp_png(&img)
}

fn upload_last() {
    let Some(p) = toast::path().filter(|p| p.exists()) else {
        toast::set_status("Save is off — nothing to upload");
        return;
    };
    let cfg = with(|a| a.cfg.clone()).unwrap_or_default();
    let tx = jobs_tx();
    toast::set_status("Uploading…");
    std::thread::spawn(move || {
        let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "png".into());
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let r = std::fs::read(&p).map_err(|e| e.to_string()).and_then(|b| crate::upload::upload(&cfg.upload, &b, &name, &ext));
        if let Some(tx) = tx {
            tx.send(Job::Uploaded(r));
        }
    });
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
            if let Some(t) = LAST_TEXT.with(|t| t.borrow().clone()).filter(|_| with(|a| a.last.is_none()).unwrap_or(true)) {
                output::copy_text(&t);
            } else if let Some(p) = toast::path().filter(|p| p.exists()) {
                // Copy what was saved (includes beautify/transparency).
                match output::read_png(&p) {
                    Ok(img) => {
                        output::copy_image(&img);
                    }
                    Err(_) => {
                        if let Some((img, _)) = with(|a| a.last.clone()).flatten() {
                            output::copy_image(&img);
                        }
                    }
                }
            } else if let Some((img, _)) = with(|a| a.last.clone()).flatten() {
                output::copy_image(&img);
            }
            toast::dismiss();
        }
        Action::Pin => {
            if let Some((img, _)) = with(|a| a.last.clone()).flatten() {
                let at = with(|a| a.last_rect).flatten();
                toast::dismiss();
                crate::pin::pin(img, at);
            }
        }
        Action::Trim => {
            if let Some(p) = toast::path() {
                spawn_self(&["--trim", &p.to_string_lossy()]);
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
        Action::Upload => upload_last(),
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
        deliver(st.finish(), Meta { mode: "Scroll", ..Default::default() });
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
            Job::Ready { raw, thumb, size, path, status } => {
                with(|a| a.last = Some((raw, path.clone())));
                if cfg.show_toolbar && !cfg.open_editor {
                    let upload = cfg.upload.provider != config::UploadProvider::Off && !cfg.upload.auto;
                    toast::show(host, Content::Image { thumb, size, status, upload }, path, cfg.toolbar_seconds);
                }
            }
            Job::Saved(_) => {}
            Job::Uploaded(Ok(url)) => {
                if cfg.upload.copy_link {
                    output::copy_text(&url);
                    toast::set_status("Uploaded — link copied");
                } else {
                    toast::set_status("Uploaded");
                }
                log(&format!("uploaded: {url}"));
            }
            Job::Uploaded(Err(e)) => {
                toast::set_status("Upload failed");
                notify(host, "Upload failed", &e);
            }
            Job::Msg(m) => notify(host, "Framegrab", &m),
            Job::Ocr(Ok(out)) => {
                let t = out.text.trim().to_string();
                if t.is_empty() {
                    toast::show(host, Content::Text { text: "No text found in that area.".into(), status: "Nothing to copy".into() }, None, cfg.toolbar_seconds.max(3));
                } else {
                    output::copy_text(&t);
                    LAST_TEXT.with(|l| *l.borrow_mut() = Some(t.clone()));
                    with(|a| a.last = None);
                    let n = t.chars().count();
                    let status = match out.kind {
                        crate::ocr::Kind::Qr => "QR code copied".to_string(),
                        crate::ocr::Kind::Table => format!("Table copied ({} rows)", t.lines().count()),
                        crate::ocr::Kind::Link => "Link copied".to_string(),
                        crate::ocr::Kind::Email => "Email address copied".to_string(),
                        crate::ocr::Kind::Code => "Code copied".to_string(),
                        crate::ocr::Kind::Text => format!("Copied {n} characters"),
                    };
                    let status = match &out.note {
                        Some(note) => format!("{status} · {note}"),
                        None => status,
                    };
                    toast::show(host, Content::Text { text: t, status }, None, cfg.toolbar_seconds);
                }
            }
            Job::Ocr(Err(e)) => toast::show(host, Content::Text { text: e, status: "Couldn't read text".into() }, None, cfg.toolbar_seconds.max(4)),
            Job::RecDone(res) => {
                schedule_trim();
                with(|a| {
                    if matches!(a.busy, Busy::Finishing) {
                        a.busy = Busy::Idle;
                    }
                });
                match res {
                    Ok((path, thumb)) => {
                        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                        let detail = if size > 10 << 20 { format!("{:.0} MB", size as f64 / 1048576.0) } else { format!("{:.1} MB", size as f64 / 1048576.0) };
                        toast::show(host, Content::Video { thumb, status: "Recording saved".into(), detail, trim: path.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")) }, Some(path), cfg.toolbar_seconds.max(5));
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
        "pin" => {
            // pin <history id>
            let img = rest.parse::<u64>().ok().and_then(crate::history::find).and_then(|e| output::read_png(std::path::Path::new(&e.image)).ok());
            if let Some(img) = img {
                crate::pin::pin(img, None);
            }
        }
        "history-remove" => {
            if let Ok(id) = rest.parse::<u64>() {
                crate::history::remove(id);
            }
        }
        "history-clear" => crate::history::clear(),
        "pins-close" => crate::pin::close_all(),
        "pin-dump" => {
            // pin-dump <dir> [zoom factor]
            let (dir, f) = rest.rsplit_once(' ').and_then(|(d, f)| f.parse::<f32>().ok().map(|f| (d, f))).unwrap_or((rest, 1.0));
            let r = crate::pin::dump(std::path::Path::new(dir), f);
            let _ = std::fs::write(std::path::Path::new(dir).join("pins.txt"), r);
        }
        "pin-last" => toast_action(Action::Pin),
        "pause" => {
            hook::PAUSED.store(rest != "off", Relaxed);
            write_status();
        }
        "sim" => overlay::sim(rest),
        "key" => {
            if let Ok(vk) = rest.parse::<u32>() {
                route_key(vk);
            }
        }
        "gfx-selftest" => {
            // Draws through the real window path (shared GPU device + swap chain) in a window
            // parked far off-screen, then reads it back with PrintWindow.
            unsafe extern "system" fn plain(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
                unsafe { DefWindowProcW(h, m, w, l) }
            }
            win::register_class("FramegrabSelftest", Some(plain));
            let h = win::create("FramegrabSelftest", "fg-selftest", WS_POPUP, WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, -20000, -20000, 320, 160, None);
            unsafe {
                let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
            }
            let mut rep = String::new();
            if let Some(c) = crate::gfx::Canvas::new(h, 320, 160) {
                for _ in 0..2 {
                    c.begin();
                    c.clear(0xFF202028);
                    c.round(20.0, 20.0, 120.0, 60.0, 10.0, 0xFF6E8BFF);
                    c.text("Framegrab", 20.0, 90.0, 280.0, 40.0, 24.0, 600, 0xFFFFFFFF, crate::gfx::Align::Left);
                    rep += &format!("end ok={} ", c.end());
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
                let img = crate::winfo::print_window(h.0 as isize, Rect::new(-20000, -20000, 320, 160));
                rep += &format!("readback={}", img.is_some());
                if let Some(img) = img {
                    let _ = output::write_png(&std::path::Path::new(rest).join("gfx-selftest.png"), &img, true);
                    let px = |x: u32, y: u32| { let i = ((y * img.w + x) * 4) as usize; (img.px[i + 2], img.px[i + 1], img.px[i]) };
                    rep += &format!(" bg={:?} box={:?}", px(5, 5), px(80, 50));
                }
            } else {
                rep += "no canvas";
            }
            unsafe {
                let _ = DestroyWindow(h);
            }
            let _ = std::fs::write(std::path::Path::new(rest).join("gfx-selftest.txt"), rep);
        }
        "state" => {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(rest) {
                let _ = writeln!(f, "{}", overlay::debug_state());
            }
        }
        "dump" => {
            let files = overlay::dump(std::path::Path::new(rest));
            let _ = std::fs::write(std::path::Path::new(rest).join("dump.txt"), files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n"));
        }
        "dxgi-compare" => {
            let dir = std::path::PathBuf::from(rest);
            let mons = capture::monitors();
            let t = std::time::Instant::now();
            let gdi: Vec<Image> = mons.iter().map(|m| capture::grab(m.rect, false)).collect();
            let tg = t.elapsed().as_millis();
            let t = std::time::Instant::now();
            let dx = crate::dxgi::grab_monitors(&mons);
            let td = t.elapsed().as_millis();
            let t = std::time::Instant::now();
            let dx2 = crate::dxgi::grab_monitors(&mons);
            let td2 = t.elapsed().as_millis();
            let mut rep = format!("gdi {tg} ms, dxgi {td} ms, dxgi again {td2} ms, ok={}\n", dx2.is_some());
            if let Some(dx) = dx {
                for (i, (a, b)) in gdi.iter().zip(&dx).enumerate() {
                    let same = a.w == b.w && a.h == b.h;
                    let diff = if same { a.px.chunks_exact(4).zip(b.px.chunks_exact(4)).filter(|(p, q)| p[..3] != q[..3]).count() } else { usize::MAX };
                    rep += &format!("mon {i} {:?} gdi {}x{} dxgi {}x{} differing px {}\n", mons[i].rect, a.w, a.h, b.w, b.h, diff);
                    let _ = output::write_png(&dir.join(format!("dx-{i}.png")), &b.fit(960, 960), true);
                }
            }
            let _ = std::fs::write(dir.join("dxgi.txt"), rep);
        }
        "deliver-test" => {
            // Runs the pipeline on an existing PNG.
            if let Ok(img) = output::read_png(std::path::Path::new(rest)) {
                deliver(img, Meta { mode: "Snip", ..Default::default() });
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
                    TIMER_TRIM => trim_tick(h),
                    TIMER_GAME => game_tick(),
                    TIMER_PREWARM => {
                        let _ = KillTimer(Some(h), TIMER_PREWARM);
                        overlay::prewarm();
                        schedule_trim_in(3000);
                    }
                    crate::updater::TIMER_ID => crate::updater::on_timer(h),
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
