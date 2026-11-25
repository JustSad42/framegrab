//! Optional uploads (off unless configured): ImgBB with your API key, Imgur with your own Client ID, or any
//! ShareX-style endpoint that accepts a multipart file upload.

use crate::config::{Upload, UploadProvider};

/// HTTP client using Windows' own TLS (SChannel), so no TLS library is bundled in the exe.
pub fn http(timeout_secs: u64, status_as_error: bool) -> ureq::Agent {
    use ureq::tls::{TlsConfig, TlsProvider};
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(timeout_secs)))
        .http_status_as_error(status_as_error)
        .tls_config(TlsConfig::builder().provider(TlsProvider::NativeTls).build())
        .build()
        .into()
}

fn multipart(field: &str, filename: &str, mime: &str, bytes: &[u8], extra: &[(&str, &str)]) -> (String, Vec<u8>) {
    let boundary = format!("----framegrab{:016x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7));
    let mut body = Vec::with_capacity(bytes.len() + 512);
    for (k, v) in extra {
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
    }
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{filename}\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// Follows a dot path like `data.link` or `files.0.url` into a JSON value.
pub fn json_path(v: &serde_json::Value, path: &str) -> Option<String> {
    let mut cur = v;
    for part in path.split('.').filter(|p| !p.is_empty()) {
        cur = match part.parse::<usize>() {
            Ok(i) if cur.is_array() => cur.get(i)?,
            _ => cur.get(part)?,
        };
    }
    cur.as_str().map(|s| s.to_string())
}

pub fn mime(ext: &str) -> &'static str {
    match ext {
        "jpg" => "image/jpeg",
        "webp" => "image/webp",
        "avif" => "image/avif",
        _ => "image/png",
    }
}

/// Uploads and returns the link. Runs on a worker thread.
pub fn upload(cfg: &Upload, bytes: &[u8], filename: &str, ext: &str) -> Result<String, String> {
    let agent = http(90, false);
    let mut extra: Vec<(&str, &str)> = Vec::new();
    let (url, field, header, path) = match cfg.provider {
        UploadProvider::Off => return Err("Uploads are off".into()),
        UploadProvider::ImgBB => {
            let key = cfg.imgbb_key.trim();
            if key.is_empty() {
                return Err("Add your ImgBB API key in Settings".into());
            }
            // The key goes in the form body, not the URL, so it doesn't end up in logs.
            extra.push(("key", key));
            ("https://api.imgbb.com/1/upload".to_string(), "image".to_string(), String::new(), "data.url".to_string())
        }
        UploadProvider::Imgur => {
            if cfg.imgur_client_id.trim().is_empty() {
                return Err("Add your Imgur Client ID in Settings".into());
            }
            ("https://api.imgur.com/3/image".to_string(), "image".to_string(), format!("Authorization: Client-ID {}", cfg.imgur_client_id.trim()), "data.link".to_string())
        }
        UploadProvider::Custom => {
            if !cfg.custom_url.trim().starts_with("http") {
                return Err("Set an upload URL in Settings".into());
            }
            let f = if cfg.custom_field.trim().is_empty() { "file".to_string() } else { cfg.custom_field.trim().to_string() };
            (cfg.custom_url.trim().to_string(), f, cfg.custom_header.trim().to_string(), cfg.custom_response_path.trim().to_string())
        }
    };
    let (ctype, body) = multipart(&field, filename, mime(ext), bytes, &extra);
    let mut req = agent.post(&url).header("User-Agent", "Framegrab").header("Content-Type", &ctype);
    if let Some((k, v)) = header.split_once(':') {
        req = req.header(k.trim(), v.trim());
    }
    let mut resp = req.send(&body[..]).map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().with_config().limit(1 << 20).read_to_string().map_err(|e| e.to_string())?;
    if !(200..300).contains(&status) {
        // Most hosts explain themselves in JSON (e.g. ImgBB: {"error":{"message":"Invalid API v1 key."}}).
        let why = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| json_path(&v, "error.message").or_else(|| json_path(&v, "data.error")));
        return Err(match why {
            Some(m) => format!("Upload failed: {m}"),
            None => format!("Upload failed (HTTP {status})"),
        });
    }
    if path.is_empty() {
        let link = text.trim().to_string();
        return if link.starts_with("http") { Ok(link) } else { Err("The server didn't return a link".into()) };
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|_| "The server didn't return JSON".to_string())?;
    json_path(&v, &path).ok_or_else(|| format!("No link at \"{path}\" in the response"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn json_paths() {
        let v: serde_json::Value = serde_json::from_str(r#"{"data":{"link":"https://i/x.png"},"files":[{"url":"https://a"}]}"#).unwrap();
        assert_eq!(super::json_path(&v, "data.link").as_deref(), Some("https://i/x.png"));
        assert_eq!(super::json_path(&v, "files.0.url").as_deref(), Some("https://a"));
        assert_eq!(super::json_path(&v, "nope"), None);
    }

    #[test]
    fn multipart_has_key_field() {
        let (ct, body) = super::multipart("image", "a.png", "image/png", b"PNG", &[("key", "abc")]);
        let b = String::from_utf8_lossy(&body);
        let boundary = ct.split("boundary=").nth(1).unwrap();
        assert!(b.starts_with(&format!("--{boundary}\r\nContent-Disposition: form-data; name=\"key\"\r\n\r\nabc\r\n")));
        assert!(b.contains("name=\"image\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\nPNG\r\n"));
        assert!(b.ends_with(&format!("--{boundary}--\r\n")));
    }
}

#[cfg(test)]
mod net_tests {
    /// Network check of the Windows TLS client: `cargo test --release https_works -- --ignored`.
    #[test]
    #[ignore]
    fn https_works() {
        let mut r = super::http(15, false).get("https://api.github.com/zen").header("User-Agent", "Framegrab").call().expect("request");
        assert_eq!(r.status().as_u16(), 200);
        assert!(!r.body_mut().read_to_string().unwrap().is_empty());
    }

    /// ImgBB rejects a made-up key with a readable reason: `cargo test --release imgbb_bad_key -- --ignored`.
    #[test]
    #[ignore]
    fn imgbb_bad_key() {
        let cfg = crate::config::Upload { provider: crate::config::UploadProvider::ImgBB, imgbb_key: "framegrab-test-invalid-key".into(), ..Default::default() };
        let png = crate::output::encode_png(&crate::capture::Image::new(1, 1), true).unwrap();
        let e = super::upload(&cfg, &png, "test.png", "png").unwrap_err();
        println!("{e}");
        assert!(e.starts_with("Upload failed"), "{e}");
    }
}
