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

/// The only API namespace this build can reach: the business app talks to `/api/v1/`,
/// the admin console (feature `admin`) only to `/api/admin/v1/`.
#[cfg(not(feature = "admin"))]
pub const API_PREFIX: &str = "/api/v1/";
#[cfg(feature = "admin")]
pub const API_PREFIX: &str = "/api/admin/v1/";

/// Only relative paths under `prefix` on the configured server; no traversal,
/// no scheme or authority smuggling, no fragments.
fn api_path_in(prefix: &str, p: &str) -> Result<(), String> {
    let ok = p.starts_with(prefix)
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
/// same origin as `base` and still under [`API_PREFIX`]. Catches encoded dot-segments
/// (`%2e%2e`) that a plain string check would miss.
pub fn api_url(base: &str, path: &str) -> Result<reqwest::Url, String> {
    api_url_in(API_PREFIX, base, path)
}

fn api_url_in(prefix: &str, base: &str, path: &str) -> Result<reqwest::Url, String> {
    api_path_in(prefix, path)?;
    let base = reqwest::Url::parse(base).map_err(|_| "invalid_url".to_string())?;
    let url = base.join(path).map_err(|_| "invalid_path".to_string())?;
    let lower = url.path().to_ascii_lowercase();
    if url.origin() != base.origin()
        || !url.path().starts_with(prefix)
        || lower.contains("%2e")
        || lower.contains("%2f")
    {
        return Err("invalid_path".into());
    }
    Ok(url)
}

/// A 6-digit authenticator code (admin console).
#[cfg_attr(not(feature = "admin"), allow(dead_code))]
pub fn totp_code(raw: &str) -> Result<String, String> {
    let code: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    if code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit()) {
        Ok(code)
    } else {
        Err("invalid_code".into())
    }
}

/// Log text from the webview: bounded and free of control characters, so it cannot
/// forge extra log lines or terminal escapes.
pub fn log_text(raw: &str) -> String {
    raw.chars()
        .take(2000)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// A listing media content path (`.../listings/<id>/media/<id>/content` or the catalogue
/// equivalent); the prefix and traversal checks are done by `api_url`.
pub fn listing_media_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let n = parts.len();
    n >= 6
        && parts[n - 1] == "content"
        && parts[n - 3] == "media"
        && matches!(parts[n - 5], "listings" | "catalogue")
        && uuid::Uuid::parse_str(parts[n - 2]).is_ok()
        && uuid::Uuid::parse_str(parts[n - 4]).is_ok()
}

/// The image type, if the declared type is JPEG/PNG/WebP and the bytes really are that.
pub fn image_type(declared: &str, b: &[u8]) -> Option<&'static str> {
    match declared {
        "image/jpeg" if b.starts_with(&[0xFF, 0xD8, 0xFF]) => Some("image/jpeg"),
        "image/png" if b.starts_with(b"\x89PNG\r\n\x1a\n") => Some("image/png"),
        "image/webp" if b.len() > 12 && b.starts_with(b"RIFF") && &b[8..12] == b"WEBP" => {
            Some("image/webp")
        }
        _ => None,
    }
}

/// `<number>.pdf` for an issued invoice or credit note (numbers are A–Z, 0–9 and '-').
pub fn invoice_file_name(number: &str) -> Option<String> {
    let ok = (1..=40).contains(&number.len())
        && number
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-')
        && !number.starts_with('-');
    ok.then(|| format!("{number}.pdf"))
}

#[cfg_attr(feature = "admin", allow(dead_code))]
/// A file name safe to create in the user's Downloads folder: no path components, no
/// control or reserved characters, not hidden, at most 150 characters. `None` if nothing
/// usable remains.
pub fn download_name(raw: &str) -> Option<String> {
    let base = raw.rsplit(['/', '\\']).next()?;
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .take(150)
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim().to_string();
    // Windows reserved device names.
    let stem = cleaned.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem[3..].chars().all(|c| c.is_ascii_digit()));
    if cleaned.is_empty() || reserved {
        None
    } else {
        Some(cleaned)
    }
}

#[cfg_attr(feature = "admin", allow(dead_code))]
/// File types the server accepts as attachments; nothing else is ever written to disk.
pub fn is_attachment_type(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| {
        matches!(
            ext.to_ascii_lowercase().as_str(),
            "pdf" | "png" | "jpg" | "jpeg" | "webp" | "txt" | "csv" | "xlsx" | "docx"
        )
    })
}

/// A kept supplier file (電子帳簿保存法 record): what attachments allow, plus saved emails,
/// FAX TIFF scans and old Excel files. Saved to Downloads, never opened by the app.
pub fn is_record_type(name: &str) -> bool {
    is_attachment_type(name)
        || name.rsplit_once('.').is_some_and(|(_, ext)| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "eml" | "tif" | "tiff" | "xls"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_types_add_mail_and_fax_files() {
        assert!(
            is_record_type("reply.eml") && is_record_type("fax.TIFF") && is_record_type("q.xls")
        );
        assert!(is_record_type("q.pdf"));
        assert!(!is_record_type("run.exe") && !is_record_type("noext"));
    }

    const V1: &str = "/api/v1/";
    const ADMIN: &str = "/api/admin/v1/";

    #[test]
    fn only_attachment_types_are_saved() {
        assert!(is_attachment_type("図面.PDF"));
        assert!(is_attachment_type("a.xlsx"));
        for bad in [
            "run.bat",
            "x.lnk",
            "x.desktop",
            "x.scr",
            "x.exe",
            "x.sh",
            "noext",
            "x.pdf.exe",
        ] {
            assert!(!is_attachment_type(bad), "{bad}");
        }
    }

    #[test]
    fn download_names_cannot_escape_the_folder() {
        assert_eq!(download_name("../../.bashrc").as_deref(), Some("bashrc"));
        assert_eq!(
            download_name("C:\\Windows\\x.pdf").as_deref(),
            Some("x.pdf")
        );
        assert_eq!(download_name("見積書.pdf").as_deref(), Some("見積書.pdf"));
        assert_eq!(download_name("a\u{0}:b.pdf").as_deref(), Some("a__b.pdf"));
        assert_eq!(download_name("CON.txt"), None);
        assert_eq!(download_name("..."), None);
        assert_eq!(download_name("dir/"), None);
        assert_eq!(
            download_name(&"x".repeat(400)).map(|n| n.chars().count()),
            Some(150)
        );
    }

    #[test]
    fn api_urls_stay_on_origin_and_prefix() {
        let base = "https://procure.example.jp";
        assert_eq!(
            api_url_in(V1, base, "/api/v1/me").unwrap().as_str(),
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
            assert!(api_url_in(V1, base, bad).is_err(), "{bad}");
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
        assert!(api_path_in(V1, "/api/v1/me").is_ok());
        assert!(
            api_path_in(
                V1,
                "/api/v1/orgs/0192f0aa-1111-7222-8333-444455556666/audit?limit=50"
            )
            .is_ok()
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
            assert!(api_path_in(V1, bad).is_err(), "{bad}");
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

    #[test]
    fn each_build_reaches_only_its_own_namespace() {
        let base = "https://procure.example.jp";
        assert!(api_url_in(ADMIN, base, "/api/admin/v1/orgs").is_ok());
        assert!(api_url_in(ADMIN, base, "/api/v1/me").is_err());
        assert!(api_url_in(V1, base, "/api/admin/v1/orgs").is_err());
        assert!(api_url_in(ADMIN, base, "/api/admin/v1/%2e%2e/%2e%2e/v1/me").is_err());
        assert!(api_url(base, &format!("{API_PREFIX}me")).is_ok());
    }

    #[test]
    fn only_listing_images_are_displayed() {
        let id = "0192f0aa-1111-7222-8333-444455556666";
        assert!(listing_media_path(&format!(
            "/api/v1/orgs/{id}/listings/{id}/media/{id}/content"
        )));
        assert!(listing_media_path(&format!(
            "/api/v1/orgs/{id}/catalogue/{id}/media/{id}/content"
        )));
        assert!(listing_media_path(&format!(
            "/api/admin/v1/listings/{id}/media/{id}/content"
        )));
        assert!(!listing_media_path(&format!(
            "/api/v1/orgs/{id}/attachments/{id}/content"
        )));
        assert!(!listing_media_path(&format!(
            "/api/v1/orgs/{id}/listings/{id}/media/x/content"
        )));
        assert_eq!(
            image_type("image/png", b"\x89PNG\r\n\x1a\nrest"),
            Some("image/png")
        );
        assert_eq!(image_type("image/png", b"<svg onload=alert(1)>"), None);
        assert_eq!(image_type("image/svg+xml", b"<svg/>"), None);
        assert_eq!(image_type("text/html", b"\xFF\xD8\xFF"), None);
    }

    #[test]
    fn invoice_file_names() {
        assert_eq!(
            invoice_file_name("CS-2026-00001").as_deref(),
            Some("CS-2026-00001.pdf")
        );
        for bad in ["", "../x", "CS/1", "cs-1", "-rf", "A.B", &"A".repeat(41)] {
            assert!(invoice_file_name(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn totp_codes() {
        assert_eq!(totp_code("123 456"), Ok("123456".into()));
        for bad in ["12345", "1234567", "12345a", "", "１２３４５６"] {
            assert!(totp_code(bad).is_err(), "{bad}");
        }
    }
}

/// `YYYY-MM` (a month for CSV exports).
pub fn is_month(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 7
        && b[4] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || c.is_ascii_digit())
        && matches!(
            &s[5..],
            "01" | "02" | "03" | "04" | "05" | "06" | "07" | "08" | "09" | "10" | "11" | "12"
        )
}

#[cfg(test)]
mod month_tests {
    #[test]
    fn months() {
        assert!(super::is_month("2026-11"));
        for bad in ["2026-13", "2026-1", "2026/11", "20261-1", "2026-11&x=1", ""] {
            assert!(!super::is_month(bad), "{bad}");
        }
    }
}
