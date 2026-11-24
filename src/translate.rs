//! Optional translation of grabbed text (off unless configured): LibreTranslate (self-hosted or
//! public, optional key) or DeepL with your own API key. Runs on the text-grab worker thread.

use crate::config::{TranslateProvider, Translate};

/// Windows' display language as a two-letter code ("en", "de"…), the default target.
pub fn ui_language() -> String {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buf = [0u16; 85];
    let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
    let s = String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize]);
    s.split('-').next().filter(|p| p.len() == 2).unwrap_or("en").to_lowercase()
}

pub fn target(cfg: &Translate) -> String {
    let t = cfg.target.trim().to_lowercase();
    if t.is_empty() { ui_language() } else { t }
}

fn agent() -> ureq::Agent {
    crate::upload::http(20, false)
}

/// Returns (translated text, detected source language if known).
pub fn translate(cfg: &Translate, text: &str) -> Result<(String, Option<String>), String> {
    let to = target(cfg);
    match cfg.provider {
        TranslateProvider::Off => Err("Translation is off".into()),
        TranslateProvider::Libre => {
            let base = cfg.url.trim().trim_end_matches('/');
            if !base.starts_with("http") {
                return Err("Set a LibreTranslate URL in Settings".into());
            }
            let mut body = serde_json::json!({ "q": text, "source": "auto", "target": to, "format": "text" });
            if !cfg.key.trim().is_empty() {
                body["api_key"] = serde_json::Value::String(cfg.key.trim().to_string());
            }
            let mut resp = agent().post(&format!("{base}/translate")).header("User-Agent", "Framegrab").send_json(&body).map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let v: serde_json::Value = resp.body_mut().read_json().map_err(|_| "Unexpected reply from the translator".to_string())?;
            if status != 200 {
                return Err(v["error"].as_str().map(|s| s.to_string()).unwrap_or(format!("Translation failed (HTTP {status})")));
            }
            let out = v["translatedText"].as_str().ok_or("No translation in the reply")?.to_string();
            let src = v["detectedLanguage"]["language"].as_str().map(|s| s.to_string());
            Ok((out, src))
        }
        TranslateProvider::DeepL => {
            let key = cfg.key.trim();
            if key.is_empty() {
                return Err("Add your DeepL API key in Settings".into());
            }
            // Free-plan keys end in ":fx" and use their own host.
            let host = if key.ends_with(":fx") { "https://api-free.deepl.com" } else { "https://api.deepl.com" };
            let body = serde_json::json!({ "text": [text], "target_lang": to.to_uppercase() });
            let mut resp = agent()
                .post(&format!("{host}/v2/translate"))
                .header("User-Agent", "Framegrab")
                .header("Authorization", &format!("DeepL-Auth-Key {key}"))
                .send_json(&body)
                .map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            if status != 200 {
                return Err(format!("Translation failed (HTTP {status})"));
            }
            let v: serde_json::Value = resp.body_mut().read_json().map_err(|_| "Unexpected reply from DeepL".to_string())?;
            let t = &v["translations"][0];
            let out = t["text"].as_str().ok_or("No translation in the reply")?.to_string();
            Ok((out, t["detected_source_language"].as_str().map(|s| s.to_lowercase())))
        }
    }
}
