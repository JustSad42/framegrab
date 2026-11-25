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

In the overlay: `1`–`5` or `Tab` switch mode, arrow keys nudge the cursor by a pixel (Shift: 10), `Enter` takes what's highlighted, `F` takes the whole monitor, `C` shows the magnifier and colour readout (off by default), `Esc` or right-click cancels. Selections snap to nearby edges of buttons, panels and windows as you drag; hold `Alt` to place freely (both can be switched off in Settings).

## What happens after a capture

Everything below is set once in Settings and then happens automatically:

- Copy to the clipboard and/or save to `Pictures\Screenshots` as PNG, JPG, WebP (lossless) or AVIF, with a name template (`{date} {time} {app} {title} {mode} {w} {h}`) and optional per-app folders.
- Window captures (click a window) grab the window's own pixels even if it's covered, with transparent rounded corners. Games and apps that Windows stretches for display scaling (e.g. League of Legends at 150%) are captured at full screen resolution, not as a small picture in a black frame.
- Beautify: gradient background, padding, rounded corners and a shadow.
- Optional upload to ImgBB (your API key), Imgur (your Client ID) or any ShareX-style endpoint, with the link copied. Off by default; every capture can upload automatically, or only when you press **Upload** on the card or in the editor (`Ctrl+U`, uploads your edits).
- Optional shutter sound and screen flash.

A small card offers **Edit**, **Text**, copy, **pin**, upload, show in folder and delete, and you can drag the thumbnail straight into Discord, a browser or Explorer.

- **History** (first page of Settings): the last 20 captures, kept even when saving is off. Click to copy, drag one out, pin, edit or remove.
- **Pin to screen**: an always-on-top copy to work from. Drag to move, wheel to zoom, `0` for actual size, `Ctrl+C` to copy, right-click for more, double-click or `Esc` to close.

Text grab keeps line breaks, joins paragraphs or puts everything on one line; tables are copied tab-separated (they paste as cells) and QR codes are decoded. When the text is just a link, email address or code, only that is copied. Translation can be set up once (LibreTranslate or DeepL with your key; off by default) so grabbed text in another language is copied in yours.

The editor has arrows, curved arrows, rectangles, ellipses, lines, pen, highlighter, text, callout bubbles, numbered steps, spotlight, pixelate, crop and an eyedropper, with undo/redo and a beautify switch. Arrow tips snap to the nearest edge (`Alt` to place freely). **Smart redact** (`X`) reads the image and pixelates email addresses, phone/card numbers and API keys or tokens; `Ctrl+Z` undoes it. **Done** (or Enter) saves in your chosen format and copies the result.

## Recording

MP4 (H.264 + AAC through Media Foundation, hardware encoder when available; 60 fps by default) or GIF. A 1080p60 recording where every frame changes encodes at ~280 fps on an RTX 4080 machine. System sound and microphone can be switched on the recording bar; the last choice is remembered. Recordings go to `Videos\Screen Recordings`. The bar and frame never appear in the video.

**Trim** (on the card after recording, or Settings › Recording › Open video… for any MP4): drag the start and end, play it back, and save. Nothing is re-encoded, so saving is instant and lossless; the start lands on a keyframe (recordings have one every second). **Read text** reads the paused frame and copies it.

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

Test copies never touch the real install: set `FRAMEGRAB_TEST=1` (separate settings in `%APPDATA%\Framegrab-test`, no keyboard hook unless `FRAMEGRAB_HOOK=1`), `FRAMEGRAB_HEADLESS=1` (overlay, card and bar render off-screen) and `FRAMEGRAB_OUT=<dir>` (where captures go) and `FRAMEGRAB_FAKE_CLIPBOARD=<dir>` (clipboard output goes to files instead). Drive it with `framegrab --trigger region|screen|text|…`, `--trigger sim move x y`, `--trigger dump <dir>` (renders the overlay to PNGs), `--trigger record x y w h`, `--trigger rec-button <id>`, `--trigger pin-dump <dir>`. `FRAMEGRAB_EDITOR_DEMO=<dir>` (+ `FRAMEGRAB_EDITOR_REDACT=1`) and `FRAMEGRAB_TRIM_DEMO=<dir>` make the editor / trim window run a scripted pass, save a screenshot and close.

Icons: [Phosphor](https://phosphoricons.com) (MIT).
