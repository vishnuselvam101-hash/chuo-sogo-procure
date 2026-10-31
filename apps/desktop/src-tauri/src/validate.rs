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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(method("TRACE").is_err());
        assert!(is_supported_language("ta"));
        assert!(!is_supported_language("xx"));
    }
}
