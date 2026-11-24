# Framegrab

A fast replacement for the Windows Snipping Tool: screenshots, text grab (OCR), scrolling capture and screen recording, all from the Print Screen key. Native Rust, runs quietly in the tray.

## Shortcuts

| Keys | Does |
| --- | --- |
| **Print Screen** · **Win+Shift+S** | Snip: drag an area, or click a window |
| Shift+Print Screen | Monitor under the mouse |
| Alt+Print Screen | Active window |
| Win+Print Screen | All monitors |
| Ctrl+Print Screen | Grab text and copy it |
| Ctrl+Alt+Print Screen | Scrolling capture |
| Ctrl+Shift+Print Screen | Record the screen (press again to stop) |

In the overlay: `1`–`5` or `Tab` switch mode, arrow keys nudge the cursor by a pixel (Shift: 10), `Enter` takes what's highlighted, `F` takes the whole monitor, `Esc` or right-click cancels.

## What happens after a capture

The image is copied to the clipboard and saved to `Pictures\Screenshots` (named like Windows does, e.g. `Screenshot 2026-11-24 153012.png`). A small card offers **Edit**, **Text**, copy, show in folder and delete.

The editor has arrows, rectangles, ellipses, lines, pen, highlighter, text, numbered steps, pixelate and crop, with undo/redo. **Done** (or Enter) saves over the capture and copies the result.

## Recording

MP4 (H.264 + AAC through Media Foundation, hardware encoder when available) or GIF. System sound and microphone can be switched on the recording bar; the last choice is remembered. Recordings go to `Videos\Screen Recordings`. The bar and frame never appear in the video.

## Install

```powershell
.\install.ps1
```

Installs to `%LOCALAPPDATA%\Programs\Framegrab`, adds a Start menu entry, and (by default) starts with Windows. Installed copies update from GitHub releases; an update is only installed when its SHA-256 checksum matches.

## Build & test

```powershell
cargo test
cargo build --release
```

Test copies never touch the real install: set `FRAMEGRAB_TEST=1` (separate settings in `%APPDATA%\Framegrab-test`, no keyboard hook unless `FRAMEGRAB_HOOK=1`), `FRAMEGRAB_HEADLESS=1` (overlay, card and bar render off-screen) and `FRAMEGRAB_OUT=<dir>` (where captures go). Drive it with `framegrab --trigger region|screen|text|…`, `--trigger sim move x y`, `--trigger dump <dir>` (renders the overlay to PNGs), `--trigger record x y w h`, `--trigger rec-button <id>`.

Icons: [Phosphor](https://phosphoricons.com) (MIT).
