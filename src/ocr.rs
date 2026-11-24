//! Text recognition with the OCR engine built into Windows (Windows.Media.Ocr).

use crate::capture::Image;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows_core::HSTRING;

fn engine(lang: &str) -> windows_core::Result<OcrEngine> {
    if !lang.trim().is_empty() {
        if let Ok(l) = Language::CreateLanguage(&HSTRING::from(lang.trim())) {
            if OcrEngine::IsLanguageSupported(&l).unwrap_or(false) {
                return OcrEngine::TryCreateFromLanguage(&l);
            }
        }
    }
    OcrEngine::TryCreateFromUserProfileLanguages()
}

/// Installed OCR languages as (tag, display name).
pub fn languages() -> Vec<(String, String)> {
    let mut v = Vec::new();
    if let Ok(list) = OcrEngine::AvailableRecognizerLanguages() {
        for i in 0..list.Size().unwrap_or(0) {
            if let Ok(l) = list.GetAt(i) {
                v.push((l.LanguageTag().map(|s| s.to_string()).unwrap_or_default(), l.DisplayName().map(|s| s.to_string()).unwrap_or_default()));
            }
        }
    }
    v
}

/// Upscales small captures 2× (the engine reads small UI text much better that way).
fn prepare(img: &Image, max: u32) -> Image {
    let mut out = img.clone();
    if img.w * 2 <= max && img.h * 2 <= max && (img.w as u64 * img.h as u64) < 2_500_000 {
        let (w, h) = (img.w * 2, img.h * 2);
        let mut up = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                // Bilinear sample at the source pixel centre.
                let fx = ((x as f32 + 0.5) / 2.0 - 0.5).max(0.0);
                let fy = ((y as f32 + 0.5) / 2.0 - 0.5).max(0.0);
                let (x0, y0) = (fx as u32, fy as u32);
                let (x1, y1) = ((x0 + 1).min(img.w - 1), (y0 + 1).min(img.h - 1));
                let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                let p = |xx: u32, yy: u32, c: usize| img.px[((yy * img.w + xx) * 4) as usize + c] as f32;
                let o = ((y * w + x) * 4) as usize;
                for c in 0..3 {
                    let top = p(x0, y0, c) * (1.0 - tx) + p(x1, y0, c) * tx;
                    let bot = p(x0, y1, c) * (1.0 - tx) + p(x1, y1, c) * tx;
                    up.px[o + c] = (top * (1.0 - ty) + bot * ty) as u8;
                }
                up.px[o + 3] = 255;
            }
        }
        out = up;
    } else if img.w > max || img.h > max {
        out = img.fit(max, max);
    }
    out
}

#[derive(Clone, Debug)]
pub struct Word {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub words: Vec<Word>,
}

impl Line {
    fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }
    fn top(&self) -> f32 {
        self.words.iter().map(|w| w.y).fold(f32::MAX, f32::min)
    }
    fn bottom(&self) -> f32 {
        self.words.iter().map(|w| w.y + w.h).fold(f32::MIN, f32::max)
    }
    fn left(&self) -> f32 {
        self.words.iter().map(|w| w.x).fold(f32::MAX, f32::min)
    }
    fn height(&self) -> f32 {
        (self.bottom() - self.top()).max(1.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Text,
    Table,
    Qr,
    Link,
    Email,
    Code,
}

#[derive(Clone, Debug)]
pub struct OcrOut {
    pub text: String,
    pub kind: Kind,
    /// Extra status, e.g. "Translated from German" or why translation didn't happen.
    pub note: Option<String>,
}

/// Recognizes text with word positions. Run off the UI thread.
pub fn recognize_lines(img: &Image, lang: &str) -> Result<Vec<Line>, String> {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    let run = || -> windows_core::Result<Vec<Line>> {
        let eng = engine(lang)?;
        let max = OcrEngine::MaxImageDimension().unwrap_or(10000);
        let src_w = img.w;
        let img = prepare(img, max);
        // Word boxes come back in the prepared image's pixels; report them in the caller's.
        let k = src_w as f32 / img.w.max(1) as f32;
        let w = DataWriter::new()?;
        w.WriteBytes(&img.px)?;
        let buf = w.DetachBuffer()?;
        let sb = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(&buf, BitmapPixelFormat::Bgra8, img.w as i32, img.h as i32, BitmapAlphaMode::Premultiplied)?;
        let res = eng.RecognizeAsync(&sb)?.join()?;
        let lines = res.Lines()?;
        let mut out = Vec::new();
        for i in 0..lines.Size()? {
            let l = lines.GetAt(i)?;
            let ws = l.Words()?;
            let mut words = Vec::new();
            for j in 0..ws.Size()? {
                let wd = ws.GetAt(j)?;
                let r = wd.BoundingRect()?;
                words.push(Word { text: wd.Text()?.to_string(), x: r.X * k, y: r.Y * k, w: r.Width * k, h: r.Height * k });
            }
            if !words.is_empty() {
                out.push(Line { words });
            }
        }
        Ok(out)
    };
    match run() {
        Ok(v) => Ok(v),
        Err(e) if e.code().0 == 0 => Err("No OCR language is installed (Settings › Time & language › Language)".into()),
        Err(e) => Err(format!("Text recognition failed: {}", e.message())),
    }
}

/// What smart redaction hides.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Secret {
    Email,
    Number,
    Key,
}

const PHONE_CHARS: &str = "+-().";

/// The address inside a word ("From:jane@x.com" → "jane@x.com"), as a char range.
fn email_span(t: &str) -> Option<(usize, usize)> {
    let chars: Vec<char> = t.chars().collect();
    let at = chars.iter().position(|c| *c == '@')?;
    let local = |c: &char| c.is_alphanumeric() || "._%+-".contains(*c);
    let mut a = at;
    while a > 0 && local(&chars[a - 1]) {
        a -= 1;
    }
    let mut b = at + 1;
    while b < chars.len() && (chars[b].is_alphanumeric() || chars[b] == '.' || chars[b] == '-') {
        b += 1;
    }
    while b > at + 1 && chars[b - 1] == '.' {
        b -= 1;
    }
    let host: String = chars[at + 1..b].iter().collect();
    (a < at && host.contains('.') && !host.starts_with('.')).then_some((a, b))
}

#[cfg(test)]
fn is_email(t: &str) -> bool {
    email_span(t).is_some()
}

/// The key inside a word ("KEY=sk-…" → "sk-…"), as a char range.
fn key_span(t: &str) -> Option<(usize, usize)> {
    let chars: Vec<char> = t.chars().collect();
    let mut a = chars.iter().rposition(|c| *c == '=' || *c == ':').map(|i| i + 1).unwrap_or(0);
    // Keep trailing '=' padding (base64) with the value.
    if a > 0 && a == chars.len() {
        a = 0;
    }
    let trim = |c: char| matches!(c, '"' | '\'' | ',' | ';' | '(' | ')' | '[' | ']' | '<' | '>');
    while a < chars.len() && trim(chars[a]) {
        a += 1;
    }
    let mut b = chars.len();
    while b > a && trim(chars[b - 1]) {
        b -= 1;
    }
    let v: String = chars[a..b].iter().collect();
    is_key(&v).then_some((a, b))
}

/// API keys, tokens, passwords-looking strings: long, mixed letters and digits, or a known prefix.
fn is_key(t: &str) -> bool {
    let t = t.trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | ';' | '(' | ')' | '[' | ']' | '<' | '>'));
    // "KEY=value" / "token: value": judge the value.
    let t = t.rsplit(['=', ':']).next().unwrap_or(t);
    let prefixes = ["sk-", "sk_live_", "sk_test_", "pk_live_", "rk_live_", "ghp_", "gho_", "ghs_", "github_pat_", "glpat-", "xoxb-", "xoxp-", "AKIA", "ASIA", "AIza", "eyJ", "hf_", "npm_"];
    let ok_chars = t.chars().all(|c| c.is_ascii_alphanumeric() || "-_./+=".contains(c));
    if !ok_chars {
        return false;
    }
    if t.len() >= 16 && prefixes.iter().any(|p| t.starts_with(p)) {
        return true;
    }
    if t.len() < 20 || t.contains("://") || t.starts_with("www.") {
        return false;
    }
    let letters = t.chars().filter(|c| c.is_ascii_alphabetic()).count();
    let digits = t.chars().filter(|c| c.is_ascii_digit()).count();
    let upper = t.chars().any(|c| c.is_ascii_uppercase());
    let lower = t.chars().any(|c| c.is_ascii_lowercase());
    // Long hex (hashes, secrets) or a random-looking mix. Plain words and paths don't qualify.
    let hex = t.len() >= 32 && t.chars().all(|c| c.is_ascii_hexdigit());
    hex || (letters >= 4 && digits >= 3 && (upper && lower || digits * 4 >= t.len()) && !t.contains('/'))
}

/// Phone, card and account numbers: a run of words made only of digits and phone punctuation,
/// 9-19 digits in total (dates and times have fewer). Returns word index ranges.
fn number_runs(words: &[Word]) -> Vec<(usize, usize)> {
    let numberish = |t: &str| t.chars().any(|c| c.is_ascii_digit()) && t.chars().all(|c| c.is_ascii_digit() || PHONE_CHARS.contains(c));
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        if !numberish(&words[i].text) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j + 1 < words.len() && numberish(&words[j + 1].text) {
            j += 1;
        }
        let digits: usize = words[i..=j].iter().map(|w| w.text.chars().filter(|c| c.is_ascii_digit()).count()).sum();
        if (9..=19).contains(&digits) {
            out.push((i, j));
        }
        i = j + 1;
    }
    out
}

/// Boxes (x, y, w, h, what) around emails, phone/card numbers and keys, padded a little.
pub fn find_secrets(lines: &[Line]) -> Vec<(f32, f32, f32, f32, Secret)> {
    let mut out = Vec::new();
    let pad = |x: f32, y: f32, w: f32, h: f32, s: Secret| {
        let p = (h * 0.2).max(2.0);
        (x - p, y - p, w + p * 2.0, h + p * 2.0, s)
    };
    for l in lines {
        let mut taken = vec![false; l.words.len()];
        for (i, wd) in l.words.iter().enumerate() {
            let what = email_span(&wd.text).map(|r| (r, Secret::Email)).or_else(|| key_span(&wd.text).map(|r| (r, Secret::Key)));
            if let Some(((a, b), s)) = what {
                taken[i] = true;
                // Cover just that part of the word (assumes even character widths, plus a margin).
                let n = wd.text.chars().count().max(1) as f32;
                let (x0, x1) = (wd.x + wd.w * (a as f32 - 0.5).max(0.0) / n, wd.x + wd.w * (b as f32 + 0.5).min(n) / n);
                out.push(pad(x0, wd.y, x1 - x0, wd.h, s));
            }
        }
        for (a, b) in number_runs(&l.words) {
            if taken[a..=b].iter().any(|t| *t) {
                continue;
            }
            let ws = &l.words[a..=b];
            let x0 = ws.iter().map(|w| w.x).fold(f32::MAX, f32::min);
            let y0 = ws.iter().map(|w| w.y).fold(f32::MAX, f32::min);
            let x1 = ws.iter().map(|w| w.x + w.w).fold(f32::MIN, f32::max);
            let y1 = ws.iter().map(|w| w.y + w.h).fold(f32::MIN, f32::max);
            out.push(pad(x0, y0, x1 - x0, y1 - y0, Secret::Number));
        }
    }
    out
}

fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return 1.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// Lines as they appear, joined into paragraphs, or all on one line.
pub fn layout(lines: &[Line], mode: crate::config::TextLayout) -> String {
    use crate::config::TextLayout;
    match mode {
        TextLayout::Lines => lines.iter().map(|l| l.text()).collect::<Vec<_>>().join("\n"),
        TextLayout::SingleLine => lines.iter().map(|l| l.text()).collect::<Vec<_>>().join(" "),
        TextLayout::Paragraphs => {
            let h = median(lines.iter().map(|l| l.height()).collect());
            let mut out = String::new();
            for (i, l) in lines.iter().enumerate() {
                let t = l.text();
                if i == 0 {
                    out += &t;
                    continue;
                }
                let p = &lines[i - 1];
                let gap = l.top() - p.bottom();
                let new_para = gap > h * 0.75 || gap < -h * 0.5 || (l.left() - p.left()).abs() > h * 2.5;
                if new_para {
                    out += "\n";
                    out += &t;
                } else if out.ends_with('-') && t.chars().next().is_some_and(|c| c.is_lowercase()) {
                    out.pop();
                    out += &t;
                } else {
                    out += " ";
                    out += &t;
                }
            }
            out
        }
    }
}

/// Detects a grid of columns; returns tab-separated rows (pastes as cells in Excel/Sheets).
pub fn table(lines: &[Line]) -> Option<String> {
    if lines.len() < 2 {
        return None;
    }
    let h = median(lines.iter().map(|l| l.height()).collect());
    // Rows: words from all lines whose vertical centres line up.
    let mut words: Vec<Word> = lines.iter().flat_map(|l| l.words.clone()).collect();
    words.sort_by(|a, b| (a.y + a.h / 2.0).partial_cmp(&(b.y + b.h / 2.0)).unwrap());
    let mut rows: Vec<Vec<Word>> = Vec::new();
    for w in words {
        let cy = w.y + w.h / 2.0;
        match rows.last_mut() {
            Some(r) if (cy - (r[0].y + r[0].h / 2.0)).abs() < h * 0.5 => r.push(w),
            _ => rows.push(vec![w]),
        }
    }
    if rows.len() < 2 {
        return None;
    }
    // Cells: split a row where the horizontal gap is much wider than a space.
    let mut grid: Vec<Vec<(f32, String)>> = Vec::new();
    for r in &mut rows {
        r.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());
        let mut cells: Vec<(f32, String)> = Vec::new();
        let mut last_right = f32::MIN;
        for w in r.iter() {
            if cells.is_empty() || w.x - last_right > h * 1.4 {
                cells.push((w.x, w.text.clone()));
            } else {
                let c = cells.last_mut().unwrap();
                c.1.push(' ');
                c.1 += &w.text;
            }
            last_right = w.x + w.w;
        }
        grid.push(cells);
    }
    // A table: most rows share the same column count (≥2) and their columns line up.
    let mut counts = std::collections::HashMap::new();
    for r in &grid {
        *counts.entry(r.len()).or_insert(0) += 1;
    }
    let (&k, &n) = counts.iter().max_by_key(|(k, n)| (**n, **k))?;
    if k < 2 || (n as f32) < grid.len() as f32 * 0.6 || n < 2 {
        return None;
    }
    let full: Vec<&Vec<(f32, String)>> = grid.iter().filter(|r| r.len() == k).collect();
    for col in 0..k {
        let xs: Vec<f32> = full.iter().map(|r| r[col].0).collect();
        let m = median(xs.clone());
        let aligned = xs.iter().filter(|x| (**x - m).abs() < h * 3.0).count();
        if (aligned as f32) < full.len() as f32 * 0.7 {
            return None;
        }
    }
    Some(grid.iter().map(|r| r.iter().map(|c| c.1.clone()).collect::<Vec<_>>().join("\t")).collect::<Vec<_>>().join("\n"))
}

/// Decodes a QR code; returns (content, fraction of the image it covers).
pub fn read_qr(img: &Image) -> Option<(String, f32)> {
    let (w, h) = (img.w as usize, img.h as usize);
    let luma = |x: usize, y: usize| {
        let i = (y * w + x) * 4;
        ((img.px[i] as u32 * 29 + img.px[i + 1] as u32 * 150 + img.px[i + 2] as u32 * 77) >> 8) as u8
    };
    let mut prep = rqrr::PreparedImage::prepare_from_greyscale(w, h, luma);
    for g in prep.detect_grids() {
        if let Ok((_, content)) = g.decode() {
            let xs = g.bounds.iter().map(|p| p.x as f32);
            let ys = g.bounds.iter().map(|p| p.y as f32);
            let (x0, x1) = xs.clone().fold((f32::MAX, f32::MIN), |a, v| (a.0.min(v), a.1.max(v)));
            let (y0, y1) = ys.fold((f32::MAX, f32::MIN), |a, v| (a.0.min(v), a.1.max(v)));
            let frac = ((x1 - x0) * (y1 - y0)) / (w as f32 * h as f32).max(1.0);
            return Some((content, frac));
        }
    }
    None
}

const URL_CHARS: &str = "-._~:/?#[]@!$&'()*+,;=%";

/// If the whole text is a single link, email address or code (OCR may split it with spaces),
/// returns just that item.
pub fn single_item(text: &str) -> Option<(Kind, String)> {
    let tokens = text.split_whitespace().count();
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.is_empty() || tokens > 3 {
        return None;
    }
    let lower = compact.to_lowercase();
    let url_chars = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric() || URL_CHARS.contains(c));
    if (lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("www.")) && compact.len() > 8 && compact.contains('.') && url_chars(&compact) {
        return Some((Kind::Link, compact));
    }
    if tokens <= 2 && compact.matches('@').count() == 1 {
        let (local, domain) = compact.split_once('@')?;
        let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "._%+-".contains(c));
        let tld = domain.rsplit('.').next().unwrap_or("");
        if ok(local) && ok(domain) && domain.contains('.') && tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()) {
            return Some((Kind::Email, compact));
        }
    }
    let digits: String = compact.chars().filter(|c| *c != '-').collect();
    if (4..=8).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit()) {
        return Some((Kind::Code, digits));
    }
    // A bare domain with a path, like github.com/user/repo.
    if tokens == 1 && url_chars(&compact) && compact.contains('/') {
        let host = compact.split('/').next().unwrap_or("");
        let tld = host.rsplit('.').next().unwrap_or("");
        if host.contains('.') && tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()) {
            return Some((Kind::Link, compact));
        }
    }
    None
}

fn language_name(code: &str) -> String {
    let names = [("en", "English"), ("de", "German"), ("fr", "French"), ("es", "Spanish"), ("it", "Italian"), ("pt", "Portuguese"), ("nl", "Dutch"), ("pl", "Polish"), ("ru", "Russian"), ("uk", "Ukrainian"), ("ja", "Japanese"), ("ko", "Korean"), ("zh", "Chinese"), ("ar", "Arabic"), ("tr", "Turkish"), ("sv", "Swedish"), ("da", "Danish"), ("fi", "Finnish"), ("no", "Norwegian"), ("nb", "Norwegian"), ("cs", "Czech"), ("el", "Greek"), ("hu", "Hungarian"), ("ro", "Romanian"), ("he", "Hebrew"), ("hi", "Hindi"), ("id", "Indonesian"), ("th", "Thai"), ("vi", "Vietnamese")];
    let c = code.to_lowercase();
    let c = c.split('-').next().unwrap_or("");
    names.iter().find(|n| n.0 == c).map(|n| n.1.to_string()).unwrap_or(code.to_uppercase())
}

/// Text grab per the settings: QR code, table (TSV), a lone link/code/email, or text in the
/// chosen layout (translated if that's set up).
pub fn grab_text(img: &Image, cfg: &crate::config::Config) -> Result<OcrOut, String> {
    let qr = if cfg.read_qr { read_qr(img) } else { None };
    if let Some((q, frac)) = &qr {
        if *frac > 0.15 {
            return Ok(OcrOut { text: q.clone(), kind: Kind::Qr, note: None });
        }
    }
    let lines = recognize_lines(img, &cfg.ocr_language)?;
    if lines.is_empty() {
        if let Some((q, _)) = qr {
            return Ok(OcrOut { text: q, kind: Kind::Qr, note: None });
        }
        return Ok(OcrOut { text: String::new(), kind: Kind::Text, note: None });
    }
    if cfg.detect_tables {
        if let Some(t) = table(&lines) {
            return Ok(OcrOut { text: t, kind: Kind::Table, note: None });
        }
    }
    let text = layout(&lines, cfg.text_layout);
    if cfg.smart_copy {
        if let Some((kind, item)) = single_item(&text) {
            return Ok(OcrOut { text: item, kind, note: None });
        }
    }
    Ok(finish_text(text, cfg))
}

/// Translates plain text when translation is set up; otherwise returns it as is.
pub fn finish_text(text: String, cfg: &crate::config::Config) -> OcrOut {
    if cfg.translate.provider == crate::config::TranslateProvider::Off || text.trim().is_empty() {
        return OcrOut { text, kind: Kind::Text, note: None };
    }
    let target = crate::translate::target(&cfg.translate);
    match crate::translate::translate(&cfg.translate, &text) {
        Ok((t, src)) => {
            let same = src.as_deref().is_some_and(|s| s.split('-').next() == Some(target.as_str())) || t.trim() == text.trim();
            if same {
                OcrOut { text, kind: Kind::Text, note: None }
            } else {
                let from = src.map(|s| format!(" from {}", language_name(&s))).unwrap_or_default();
                OcrOut { text: t, kind: Kind::Text, note: Some(format!("Translated{from}")) }
            }
        }
        Err(e) => OcrOut { text, kind: Kind::Text, note: Some(format!("Not translated: {e}")) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn line(ws: &[(&str, f32, f32)]) -> Line {
        Line { words: ws.iter().map(|(t, x, y)| Word { text: t.to_string(), x: *x, y: *y, w: t.len() as f32 * 8.0, h: 16.0 }).collect() }
    }
    #[test]
    fn paragraphs_join_and_split() {
        let lines = vec![line(&[("Hello", 0.0, 0.0), ("wor-", 50.0, 0.0)]), line(&[("ld", 0.0, 20.0), ("again", 30.0, 20.0)]), line(&[("New", 0.0, 70.0)])];
        assert_eq!(layout(&lines, crate::config::TextLayout::Paragraphs), "Hello world again\nNew");
        assert_eq!(layout(&lines, crate::config::TextLayout::SingleLine), "Hello wor- ld again New");
    }
    fn wline(words: &[&str]) -> Line {
        Line { words: words.iter().enumerate().map(|(i, t)| Word { text: t.to_string(), x: i as f32 * 100.0, y: 10.0, w: 90.0, h: 20.0 }).collect() }
    }

    #[test]
    fn finds_secrets() {
        let kinds = |ws: &[&str]| find_secrets(&[wline(ws)]).into_iter().map(|s| s.4).collect::<Vec<_>>();
        assert_eq!(kinds(&["Mail", "me:", "jane.doe@example.com", "today"]), vec![Secret::Email]);
        assert!(is_email("<jane@example.co.uk>,") && !is_email("@handle") && !is_email("a@b"));
        // Only the address part of "From:jane@x.com" is covered.
        let s = find_secrets(&[wline(&["From:jane@example.com"])]);
        assert!(s[0].0 > 15.0 && s[0].0 < 30.0, "{:?}", s[0]);
        let s = find_secrets(&[wline(&["KEY=ghp_16C7e42F292c6912E7710c838347Ae178B4a"])]);
        assert!(s[0].0 > 0.0 && s[0].0 < 15.0, "{:?}", s[0]);
        assert_eq!(kinds(&["Call", "+44", "20", "7946", "0958"]), vec![Secret::Number]);
        assert_eq!(kinds(&["Card", "4111", "1111", "1111", "1111"]), vec![Secret::Number]);
        assert_eq!(kinds(&["OPENAI_KEY=sk-proj-Ab12Cd34Ef56Gh78"]), vec![Secret::Key]);
        assert_eq!(kinds(&["token", "ghp_16C7e42F292c6912E7710c838347Ae178B4a"]), vec![Secret::Key]);
        assert_eq!(kinds(&["sha", "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"]), vec![Secret::Key]);
        // Ordinary text, dates, times, versions, URLs and paths stay visible.
        assert!(kinds(&["Meeting", "on", "2026-11-24", "at", "10:30"]).is_empty());
        assert!(kinds(&["Version", "1.2.3", "released", "with", "42", "fixes"]).is_empty());
        assert!(kinds(&["https://github.com/JustSad42/framegrab/releases/latest"]).is_empty());
        assert!(kinds(&["internationalization", "Characteristically"]).is_empty());
        // The box covers all words of a split number.
        let s = find_secrets(&[wline(&["+1", "555", "010", "9999"])]);
        assert_eq!(s.len(), 1);
        assert!(s[0].0 < 0.0 && s[0].0 + s[0].2 > 390.0);
    }

    #[test]
    fn single_items() {
        assert_eq!(single_item("https://exa mple.com/a?b=1"), Some((Kind::Link, "https://example.com/a?b=1".into())));
        assert_eq!(single_item("github.com/JustSad42/framegrab"), Some((Kind::Link, "github.com/JustSad42/framegrab".into())));
        assert_eq!(single_item("name.surname@mail.co.uk"), Some((Kind::Email, "name.surname@mail.co.uk".into())));
        assert_eq!(single_item("482 913"), Some((Kind::Code, "482913".into())));
        assert_eq!(single_item("Your code is 482913"), None);
        assert_eq!(single_item("Hello world"), None);
        assert_eq!(single_item("3.14"), None);
    }
    #[test]
    fn reads_qr() {
        let code = qrcode::QrCode::new(b"https://example.com/framegrab").unwrap();
        let w = code.width();
        let scale = 8;
        let q = 4;
        let n = ((w + q * 2) * scale) as u32;
        let mut img = Image::new(n, n);
        for y in 0..n as usize {
            for x in 0..n as usize {
                let (cx, cy) = ((x / scale) as isize - q as isize, (y / scale) as isize - q as isize);
                let dark = cx >= 0 && cy >= 0 && (cx as usize) < w && (cy as usize) < w && code[(cx as usize, cy as usize)] == qrcode::Color::Dark;
                let v = if dark { 0 } else { 255 };
                let i = (y * n as usize + x) * 4;
                img.px[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        let (text, frac) = read_qr(&img).expect("qr");
        assert_eq!(text, "https://example.com/framegrab");
        assert!(frac > 0.3);
    }
    #[test]
    fn detects_table() {
        let lines = vec![
            line(&[("Name", 0.0, 0.0), ("Price", 200.0, 0.0)]),
            line(&[("Apple", 0.0, 24.0), ("1.20", 200.0, 24.0)]),
            line(&[("Pear", 0.0, 48.0), ("0.90", 200.0, 48.0)]),
        ];
        assert_eq!(table(&lines).as_deref(), Some("Name\tPrice\nApple\t1.20\nPear\t0.90"));
        let prose = vec![line(&[("just", 0.0, 0.0), ("words", 40.0, 0.0)]), line(&[("more", 0.0, 20.0), ("text", 40.0, 20.0)])];
        assert_eq!(table(&prose), None);
    }
}
#[cfg(test)]
mod live {
    /// FRAMEGRAB_REDACT_IMG=<png> cargo test --release redact_live -- --ignored --nocapture
    #[test]
    #[ignore]
    fn redact_live() {
        let p = std::env::var("FRAMEGRAB_REDACT_IMG").expect("FRAMEGRAB_REDACT_IMG");
        let img = crate::output::read_png(std::path::Path::new(&p)).unwrap();
        let lines = super::recognize_lines(&img, "").unwrap();
        for l in &lines {
            println!("line: {}", l.words.iter().map(|w| format!("{}[{:.0},{:.0} {:.0}x{:.0}]", w.text, w.x, w.y, w.w, w.h)).collect::<Vec<_>>().join(" "));
        }
        for s in super::find_secrets(&lines) {
            println!("secret: {:?} at {:.0},{:.0} {:.0}x{:.0}", s.4, s.0, s.1, s.2, s.3);
        }
    }
}
