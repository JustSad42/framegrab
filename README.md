# Framegrab

A fast replacement for the Windows Snipping Tool: screenshots, text grab (OCR), scrolling capture and screen recording, all from the Print Screen key. Native Rust, runs in the background with no tray icon (about 0.5 MB working set and no CPU while idle). Open Framegrab again from the Start menu to get Settings.

## Shortcuts

All shortcuts can be changed in Settings (click a shortcut, press the new keys). Print Screen can also open a menu that asks what to capture. Hotkeys pause automatically in full-screen games, or by hand in Settings.

| Keys | Does |
| --- | --- |
| **Print Screen** · **Win+Shift+S** | Snip: drag an area, or click a window |
| Shift+Print Screen | Monitor under the mouse |
| Alt+Print Screen | Active window |
| Win+Print Screen | All monitors |
| Ctrl+Print Screen | Grab text and copy it |
| Ctrl+Alt+Print Screen | Scrolling capture |
| Ctrl+Shift+Print Screen | Record the screen (press again to stop) |

In the overlay: `1`–`5` or `Tab` switch mode, arrow keys nudge the cursor by a pixel (Shift: 10), `Enter` takes what's highlighted, `F` takes the whole monitor, `C` shows the magnifier and colour readout (off by default), `Esc` or right-click cancels.

## What happens after a capture

Everything below is set once in Settings and then happens automatically:

- Copy to the clipboard and/or save to `Pictures\Screenshots` as PNG, JPG, WebP (lossless) or AVIF, with a name template (`{date} {time} {app} {title} {mode} {w} {h}`) and optional per-app folders.
- Window captures (click a window) grab the window's own pixels even if it's covered, with transparent rounded corners.
- Beautify: gradient background, padding, rounded corners and a shadow.
- Optional upload to Imgur (your Client ID) or any ShareX-style endpoint, with the link copied. Off by default.
- Optional shutter sound and screen flash.

A small card offers **Edit**, **Text**, copy, upload, show in folder and delete, and you can drag the thumbnail straight into Discord, a browser or Explorer.

Text grab keeps line breaks, joins paragraphs or puts everything on one line; tables are copied tab-separated (they paste as cells) and QR codes are decoded.

The editor has arrows, curved arrows, rectangles, ellipses, lines, pen, highlighter, text, callout bubbles, numbered steps, spotlight, pixelate, crop and an eyedropper, with undo/redo and a beautify switch. **Done** (or Enter) saves in your chosen format and copies the result.

## Recording

MP4 (H.264 + AAC through Media Foundation, hardware encoder when available; 60 fps by default) or GIF. A 1080p60 recording where every frame changes encodes at ~280 fps on an RTX 4080 machine. System sound and microphone can be switched on the recording bar; the last choice is remembered. Recordings go to `Videos\Screen Recordings`. The bar and frame never appear in the video.

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

Test copies never touch the real install: set `FRAMEGRAB_TEST=1` (separate settings in `%APPDATA%\Framegrab-test`, no keyboard hook unless `FRAMEGRAB_HOOK=1`), `FRAMEGRAB_HEADLESS=1` (overlay, card and bar render off-screen) and `FRAMEGRAB_OUT=<dir>` (where captures go) and `FRAMEGRAB_FAKE_CLIPBOARD=<dir>` (clipboard output goes to files instead). Drive it with `framegrab --trigger region|screen|text|…`, `--trigger sim move x y`, `--trigger dump <dir>` (renders the overlay to PNGs), `--trigger record x y w h`, `--trigger rec-button <id>`.

Icons: [Phosphor](https://phosphoricons.com) (MIT).
