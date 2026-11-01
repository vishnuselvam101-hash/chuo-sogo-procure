//! Validation of everything the webview asks the native layer to do.

pub const SUPPORTED_LANGUAGES: [&str; 6] = ["ja", "en", "ta", "hi", "zh-Hans", "ko"];

pub fn is_supported_language(lang: &str) -> bool {
    SUPPORTED_LANGUAGES.contains(&lang)
}

/// https anywhere; plain http only to this machine (local development).
pub fn server_url(raw: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| "invalid_url".to_string())?;
    let host = url.host_str().unwrap_or("");
    let loopback = matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1");
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        _ => return Err("insecure_server".into()),
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("invalid_url".into());
    }
    if url.path() != "/" && !url.path().is_empty() {
        return Err("invalid_url".into());
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

pub fn method(m: &str) -> Result<reqwest::Method, String> {
    match m {
        "GET" => Ok(reqwest::Method::GET),
        "POST" => Ok(reqwest::Method::POST),
        "PUT" => Ok(reqwest::Method::PUT),
        "PATCH" => Ok(reqwest::Method::PATCH),
        "DELETE" => Ok(reqwest::Method::DELETE),
        _ => Err("invalid_method".into()),
    }
}

/// Only relative paths under /api/v1/ on the configured server; no traversal,
/// no scheme or authority smuggling, no fragments.
pub fn api_path(p: &str) -> Result<(), String> {
    let ok = p.starts_with("/api/v1/")
        && p.len() <= 512
        && !p.contains("..")
        && !p.contains("//")
        && !p.contains('\\')
        && !p.contains('#')
        && p.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_.?=&%".contains(&b));
    if ok {
        Ok(())
    } else {
        Err("invalid_path".into())
    }
}

/// Full request URL for `path` on `base`, checked **after** URL normalisation:
/// same origin as `base` and still under `/api/v1/`. Catches encoded dot-segments
/// (`%2e%2e`) that a plain string check would miss.
pub fn api_url(base: &str, path: &str) -> Result<reqwest::Url, String> {
    api_path(path)?;
    let base = reqwest::Url::parse(base).map_err(|_| "invalid_url".to_string())?;
    let url = base.join(path).map_err(|_| "invalid_path".to_string())?;
    let lower = url.path().to_ascii_lowercase();
    if url.origin() != base.origin()
        || !url.path().starts_with("/api/v1/")
        || lower.contains("%2e")
        || lower.contains("%2f")
    {
        return Err("invalid_path".into());
    }
    Ok(url)
}

/// Log text from the webview: bounded and free of control characters, so it cannot
/// forge extra log lines or terminal escapes.
pub fn log_text(raw: &str) -> String {
    raw.chars()
        .take(2000)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_urls_stay_on_origin_and_prefix() {
        let base = "https://procure.example.jp";
        assert_eq!(
            api_url(base, "/api/v1/me").unwrap().as_str(),
            "https://procure.example.jp/api/v1/me"
        );
        for bad in [
            "/api/v1/%2e%2e/%2e%2e/health",
            "/api/v1/%2E%2e/x",
            "/api/v1/.%2e/x",
            "/api/v1/%2e%2E/admin",
            "/api/v1/..%2fadmin",
            "/api/v1/a%2F..%2F..%2Fhealth",
        ] {
            assert!(api_url(base, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn log_text_strips_control_characters() {
        let cleaned = log_text("ok\n2026-10-31T00:00:00Z ERROR fake line\r\u{1b}[31m");
        assert!(!cleaned.chars().any(char::is_control), "{cleaned:?}");
        assert_eq!(log_text(&"x".repeat(5000)).len(), 2000);
    }

    #[test]
    fn server_urls() {
        assert_eq!(
            server_url("https://procure.example.jp/"),
            Ok("https://procure.example.jp".into())
        );
        assert_eq!(
            server_url("http://127.0.0.1:8088"),
            Ok("http://127.0.0.1:8088".into())
        );
        assert_eq!(
            server_url("http://procure.example.jp"),
            Err("insecure_server".into())
        );
        assert_eq!(
            server_url("http://127.0.0.1.evil.com"),
            Err("insecure_server".into())
        );
        assert_eq!(
            server_url("https://user:pw@x.jp"),
            Err("invalid_url".into())
        );
        assert_eq!(server_url("https://x.jp/other"), Err("invalid_url".into()));
        assert_eq!(
            server_url("file:///etc/passwd"),
            Err("insecure_server".into())
        );
        assert!(server_url("not a url").is_err());
    }

    #[test]
    fn api_paths() {
        assert!(api_path("/api/v1/me").is_ok());
        assert!(
            api_path("/api/v1/orgs/0192f0aa-1111-7222-8333-444455556666/audit?limit=50").is_ok()
        );
        for bad in [
            "/health",
            "/api/v1/../../admin",
            "//evil.com/api/v1/me",
            "/api/v1//x",
            "https://evil.com/api/v1/me",
            "/api/v1/me#x",
            "/api/v1/me\r\nX-Injected: 1",
            "/api/v1/ me",
        ] {
            assert!(api_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn methods_and_languages() {
        assert!(method("GET").is_ok());
        assert!(method("PUT").is_ok());
        assert!(method("TRACE").is_err());
        assert!(method("put").is_err());
        assert!(is_supported_language("ta"));
        assert!(!is_supported_language("xx"));
    }
}
