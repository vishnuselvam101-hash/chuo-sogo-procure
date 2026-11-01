//! Attachment bytes on local disk, content-addressed by SHA-256
//! (`<dir>/<first two hex chars>/<sha256 hex>`). The database row is the only way to reach a
//! file; nothing here is served directly. Files are written owner-only (0600).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;

pub struct Storage {
    dir: PathBuf,
}

/// Allowed attachment types: decided by the file extension, confirmed by the content.
/// Nothing is ever rendered by the client; downloads are always saved, not opened.
pub fn content_type_for(file_name: &str, bytes: &[u8]) -> Option<&'static str> {
    let ext = Path::new(file_name)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    let starts = |magic: &[u8]| bytes.starts_with(magic);
    let text_ok = || !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok();
    match ext.as_str() {
        "pdf" if starts(b"%PDF-") => Some("application/pdf"),
        "png" if starts(b"\x89PNG\r\n\x1a\n") => Some("image/png"),
        "jpg" | "jpeg" if starts(&[0xFF, 0xD8, 0xFF]) => Some("image/jpeg"),
        "webp" if bytes.len() > 12 && starts(b"RIFF") && &bytes[8..12] == b"WEBP" => {
            Some("image/webp")
        }
        "txt" if text_ok() => Some("text/plain"),
        "csv" if text_ok() => Some("text/csv"),
        "xlsx" if starts(b"PK\x03\x04") => {
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
        }
        "docx" if starts(b"PK\x03\x04") => {
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        }
        _ => None,
    }
}

/// A display name safe to store and to suggest on download: no path components, no control
/// or reserved characters, at most 200 characters, keeps the extension.
pub fn clean_file_name(raw: &str) -> Option<String> {
    let base = raw.rsplit(['/', '\\']).next()?.trim();
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim_start_matches('.').trim().to_string();
    if cleaned.is_empty() || cleaned.chars().count() > 200 {
        return None;
    }
    Some(cleaned)
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

impl Storage {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Storage { dir: dir.into() }
    }

    fn path(&self, hash: &[u8; 32]) -> PathBuf {
        let hex = hex::encode(hash);
        self.dir.join(&hex[..2]).join(hex)
    }

    /// Stores `bytes` under their hash (idempotent) and returns the hash.
    pub async fn put(&self, bytes: &[u8]) -> std::io::Result<[u8; 32]> {
        let hash = sha256(bytes);
        let path = self.path(&hash);
        if tokio::fs::try_exists(&path).await? {
            return Ok(hash);
        }
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("storage path without parent"))?;
        tokio::fs::create_dir_all(parent).await?;
        // Write to a unique temporary name, then rename: readers never see partial files.
        let tmp = parent.join(format!(".{}.tmp", uuid::Uuid::now_v7()));
        let mut opts = tokio::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        opts.mode(0o600);
        let mut file = opts.open(&tmp).await?;
        tokio::io::AsyncWriteExt::write_all(&mut file, bytes).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&tmp, &path).await?;
        Ok(hash)
    }

    /// Reads a stored file and checks it still matches its hash.
    pub async fn get(&self, hash: &[u8; 32]) -> std::io::Result<Vec<u8>> {
        let bytes = tokio::fs::read(self.path(hash)).await?;
        if sha256(&bytes) != *hash {
            return Err(std::io::Error::other("stored file does not match its hash"));
        }
        Ok(bytes)
    }
}

/// Refuses an upload of `adding` bytes that would take the organisation over its storage quota
/// (request attachments plus listing photos and documents). Runs in the caller's tenant
/// transaction, so it counts exactly what that organisation holds.
pub async fn check_org_quota(
    state: &crate::AppState,
    tx: &mut crate::db::Tx,
    org_id: uuid::Uuid,
    adding: usize,
) -> Result<(), crate::error::ApiError> {
    let Some(quota) = state.org_storage_quota else {
        return Ok(());
    };
    let used: i64 = sqlx::query_scalar(
        "SELECT (SELECT coalesce(sum(size_bytes), 0) FROM attachments
                 WHERE org_id = $1 AND removed_at IS NULL)::bigint
              + (SELECT coalesce(sum(size_bytes), 0) FROM listing_media WHERE org_id = $1)::bigint",
    )
    .bind(org_id)
    .fetch_one(&mut **tx)
    .await?;
    let adding = i64::try_from(adding).unwrap_or(i64::MAX);
    if used.saturating_add(adding) > quota {
        return Err(crate::error::ApiError::Validation("storage_quota"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_is_decided_by_extension_and_confirmed_by_content() {
        assert_eq!(
            content_type_for("a.PDF", b"%PDF-1.7 ..."),
            Some("application/pdf")
        );
        // An HTML file renamed to .pdf is refused.
        assert_eq!(content_type_for("a.pdf", b"<html><script>"), None);
        assert_eq!(content_type_for("a.html", b"<html>"), None);
        assert_eq!(content_type_for("a.svg", b"<svg/>"), None);
        assert_eq!(content_type_for("noext", b"%PDF-"), None);
        assert_eq!(
            content_type_for("list.csv", "a,b\n1,2".as_bytes()),
            Some("text/csv")
        );
        assert_eq!(content_type_for("bin.txt", b"a\0b"), None);
        assert_eq!(
            content_type_for("p.jpg", &[0xFF, 0xD8, 0xFF, 0xE0]),
            Some("image/jpeg")
        );
    }

    #[test]
    fn file_names_lose_paths_and_reserved_characters() {
        assert_eq!(
            clean_file_name("../../etc/passwd").as_deref(),
            Some("passwd")
        );
        assert_eq!(
            clean_file_name("C:\\Users\\x\\見積書.pdf").as_deref(),
            Some("見積書.pdf")
        );
        assert_eq!(
            clean_file_name("a\u{0}b<>.pdf").as_deref(),
            Some("a_b__.pdf")
        );
        assert_eq!(
            clean_file_name("..hidden.pdf").as_deref(),
            Some("hidden.pdf")
        );
        assert_eq!(clean_file_name("   "), None);
        assert_eq!(clean_file_name("dir/"), None);
        assert!(clean_file_name(&"a".repeat(201)).is_none());
    }

    #[tokio::test]
    async fn round_trip_is_content_addressed_and_verified() {
        // Unique random name per run; test-only scratch directory.
        // nosemgrep: rust.lang.security.temp-dir.temp-dir
        let dir = std::env::temp_dir().join(format!("procure-storage-{}", uuid::Uuid::now_v7()));
        let s = Storage::new(&dir);
        let h1 = s.put(b"%PDF-hello").await.unwrap();
        let h2 = s.put(b"%PDF-hello").await.unwrap();
        assert_eq!(h1, h2);
        assert_eq!(s.get(&h1).await.unwrap(), b"%PDF-hello");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(s.path(&h1)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // Tampering is detected on read.
        std::fs::write(s.path(&h1), b"changed").unwrap();
        assert!(s.get(&h1).await.is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
