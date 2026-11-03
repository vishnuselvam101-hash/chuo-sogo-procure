//! Content packs (`packs/manifest.json`): parsing, path safety, variant choice, SHA-256
//! verification, unpacking of runtime archives, and resumable HTTPS downloads.
//!
//! The manifest comes only from the app's own installed resources, never from the webview;
//! the webview can only name a pack id. Every file is checked (size, then SHA-256) before it
//! is recorded as installed, and only recorded files are ever used.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// This build's platform key in the manifest.
pub const PLATFORM: &str = if cfg!(windows) {
    "windows-x64"
} else if cfg!(target_os = "macos") {
    "macos-arm64"
} else {
    "linux-x64"
};

/// Hosts a large pack may be downloaded from (the manifest's official upstream URLs), and the
/// hosts their downloads may redirect to (Hugging Face's own CDN).
const DOWNLOAD_HOSTS: [&str; 2] = ["huggingface.co", "raw.githubusercontent.com"];
const REDIRECT_HOST_SUFFIXES: [&str; 2] = [".huggingface.co", ".hf.co"];

/// Free space kept on the disk after a download (PACKS.md: file size + 2 GB).
pub const DISK_HEADROOM: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    #[serde(default)]
    pub url: Option<String>,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub platform: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variant {
    pub variant: String,
    #[serde(default)]
    pub default: bool,
    pub min_ram_gb: u64,
    pub title_en: String,
    pub title_ja: String,
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pack {
    pub id: String,
    pub version: String,
    pub kind: String,
    pub title_en: String,
    pub title_ja: String,
    pub licence: String,
    #[serde(default)]
    pub licence_url: Option<String>,
    #[serde(default)]
    pub files: Vec<FileEntry>,
    #[serde(default)]
    pub variants: Vec<Variant>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub packs: Vec<Pack>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PackError {
    /// The manifest is malformed or unsafe (bad path, hash or URL).
    Manifest(String),
    UnknownPack,
    UnknownVariant,
    /// A file's size or SHA-256 did not match; the file was deleted.
    Mismatch(String),
    Io(String),
    Network,
    Cancelled,
    NoSpace,
    /// The installer does not contain this core file (wrong platform build).
    NotBundled,
}

impl PackError {
    /// Stable code for the UI (translated there).
    pub fn code(&self) -> &'static str {
        match self {
            PackError::Manifest(_) => "pack_manifest",
            PackError::UnknownPack | PackError::UnknownVariant => "pack_unknown",
            PackError::Mismatch(_) => "pack_mismatch",
            PackError::Io(_) => "pack_io",
            PackError::Network => "pack_network",
            PackError::Cancelled => "pack_cancelled",
            PackError::NoSpace => "pack_no_space",
            PackError::NotBundled => "pack_not_bundled",
        }
    }
}

fn io(e: std::io::Error) -> PackError {
    PackError::Io(e.to_string())
}

/// A manifest path is relative, `/`-separated, made only of plain names, inside its own pack
/// directory (`<pack id>/...`), with no characters that mean something else on Windows.
pub fn safe_rel_path(pack_id: &str, p: &str) -> bool {
    let bad_char = |c: char| {
        c == '\\' || c == ':' || c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*')
    };
    if p.is_empty() || p.len() > 200 || p.chars().any(bad_char) || p.starts_with('/') {
        return false;
    }
    let parts: Vec<&str> = p.split('/').collect();
    if parts.len() < 2 || parts[0] != pack_id {
        return false;
    }
    parts
        .iter()
        .all(|s| !s.is_empty() && *s != "." && *s != ".." && !s.ends_with('.') && !s.ends_with(' '))
        && Path::new(p)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Only https to an allowed upstream host, no credentials, no port.
pub fn allowed_download_url(raw: &str) -> bool {
    reqwest::Url::parse(raw).is_ok_and(|u| {
        u.scheme() == "https"
            && u.username().is_empty()
            && u.password().is_none()
            && u.port().is_none()
            && u.host_str().is_some_and(|h| DOWNLOAD_HOSTS.contains(&h))
    })
}

/// Where a download may be redirected: https on the upstream or its CDN only.
pub fn allowed_redirect(u: &reqwest::Url) -> bool {
    u.scheme() == "https"
        && u.host_str().is_some_and(|h| {
            DOWNLOAD_HOSTS.contains(&h) || REDIRECT_HOST_SUFFIXES.iter().any(|s| h.ends_with(s))
        })
}

fn is_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl Manifest {
    /// Parses and checks the whole manifest; any unsafe entry rejects it.
    pub fn parse(bytes: &[u8]) -> Result<Manifest, PackError> {
        let m: Manifest =
            serde_json::from_slice(bytes).map_err(|e| PackError::Manifest(e.to_string()))?;
        if m.schema != 1 {
            return Err(PackError::Manifest("unsupported schema".into()));
        }
        for p in &m.packs {
            if !is_id(&p.id) || !matches!(p.kind.as_str(), "core" | "large") {
                return Err(PackError::Manifest(format!("bad pack {}", p.id)));
            }
            if p.files.is_empty() == p.variants.is_empty() {
                return Err(PackError::Manifest(format!("{}: files or variants", p.id)));
            }
            for v in &p.variants {
                if !is_id(&v.variant) {
                    return Err(PackError::Manifest(format!("{}: bad variant", p.id)));
                }
            }
            let all = p
                .files
                .iter()
                .chain(p.variants.iter().flat_map(|v| &v.files));
            for f in all {
                if !safe_rel_path(&p.id, &f.path) || !is_sha256(&f.sha256) {
                    return Err(PackError::Manifest(format!("unsafe entry {}", f.path)));
                }
                match (&p.kind[..], &f.url) {
                    ("large", Some(u)) if allowed_download_url(u) => {}
                    ("core", None) => {}
                    _ => return Err(PackError::Manifest(format!("bad url for {}", f.path))),
                }
            }
        }
        Ok(m)
    }

    pub fn pack(&self, id: &str) -> Result<&Pack, PackError> {
        self.packs
            .iter()
            .find(|p| p.id == id)
            .ok_or(PackError::UnknownPack)
    }
}

impl Pack {
    /// The files to install on this platform for `variant` (packs without variants ignore it).
    pub fn files_for(&self, variant: Option<&str>) -> Result<Vec<&FileEntry>, PackError> {
        let files: Vec<&FileEntry> = if self.variants.is_empty() {
            self.files.iter().collect()
        } else {
            let name = variant.ok_or(PackError::UnknownVariant)?;
            let v = self
                .variants
                .iter()
                .find(|v| v.variant == name)
                .ok_or(PackError::UnknownVariant)?;
            v.files.iter().collect()
        };
        Ok(files
            .into_iter()
            .filter(|f| f.platform.as_deref().is_none_or(|p| p == PLATFORM))
            .collect())
    }

    /// PACKS.md: the default variant if the machine has at least its `min_ram_gb`, else the
    /// next variant whose `min_ram_gb` fits, else the smallest.
    pub fn recommended_variant(&self, ram_bytes: u64) -> Option<&str> {
        let gb = ram_bytes / (1024 * 1024 * 1024);
        // Total RAM is reported a little under the marketing size (16 GB shows as ~15.8 GiB).
        let fits = |v: &&Variant| gb + 1 >= v.min_ram_gb;
        let default = self.variants.iter().find(|v| v.default);
        default
            .filter(fits)
            .or_else(|| {
                let mut rest: Vec<&Variant> = self.variants.iter().filter(fits).collect();
                rest.sort_by_key(|v| std::cmp::Reverse(v.min_ram_gb));
                rest.first().copied()
            })
            .or_else(|| self.variants.iter().min_by_key(|v| v.min_ram_gb))
            .map(|v| v.variant.as_str())
    }
}

/// Joins a checked manifest path onto a root.
pub fn resolve(root: &Path, pack_id: &str, rel: &str) -> Result<PathBuf, PackError> {
    if !safe_rel_path(pack_id, rel) {
        return Err(PackError::Manifest(format!("unsafe path {rel}")));
    }
    Ok(rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s)))
}

/// SHA-256 of a file, streamed.
pub fn sha256_file(path: &Path) -> Result<String, PackError> {
    let mut f = std::fs::File::open(path).map_err(io)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(io)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// Size then SHA-256; on a mismatch the file is deleted and the error says so.
pub fn verify_or_delete(path: &Path, f: &FileEntry) -> Result<(), PackError> {
    let size = std::fs::metadata(path).map_err(io)?.len();
    let ok = size == f.size && sha256_file(path)? == f.sha256;
    if ok {
        Ok(())
    } else {
        let _ = std::fs::remove_file(path);
        tracing::warn!(file = %f.path, "pack file failed verification and was deleted");
        Err(PackError::Mismatch(f.path.clone()))
    }
}

/// Record of verified files: `<root>/installed.json`, path -> sha256.
#[derive(Default, Serialize, Deserialize)]
pub struct Installed {
    pub files: BTreeMap<String, String>,
}

impl Installed {
    pub fn load(root: &Path) -> Installed {
        std::fs::read(root.join("installed.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, root: &Path) -> Result<(), PackError> {
        let tmp = root.join("installed.json.tmp");
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| PackError::Io(e.to_string()))?;
        std::fs::write(&tmp, bytes).map_err(io)?;
        std::fs::rename(&tmp, root.join("installed.json")).map_err(io)
    }

    /// Verified earlier with this exact hash, and still there with the right size.
    pub fn has(&self, root: &Path, pack_id: &str, f: &FileEntry) -> bool {
        self.files.get(&f.path) == Some(&f.sha256)
            && resolve(root, pack_id, &f.path)
                .ok()
                .and_then(|p| std::fs::metadata(p).ok())
                .is_some_and(|m| m.len() == f.size)
    }
}

/// Whether every file of `pack`/`variant` is installed and verified.
pub fn is_installed(
    root: &Path,
    installed: &Installed,
    pack: &Pack,
    variant: Option<&str>,
) -> bool {
    pack.files_for(variant)
        .is_ok_and(|fs| !fs.is_empty() && fs.iter().all(|f| installed.has(root, &pack.id, f)))
}

/// Directory a runtime archive is unpacked into: `<root>/<pack>/<platform>/`.
pub fn unpack_dir(root: &Path, pack_id: &str) -> PathBuf {
    root.join(pack_id).join(PLATFORM)
}

fn is_archive(name: &str) -> bool {
    name.ends_with(".zip") || name.ends_with(".tar.gz")
}

/// Unpacks a verified archive into `dest` (replaced), refusing entries that would land
/// outside it and links that point anywhere but a sibling file.
pub fn unpack(archive: &Path, dest: &Path) -> Result<(), PackError> {
    let tmp = dest.with_extension("unpacking");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(io)?;
    let name = archive.to_string_lossy();
    if name.ends_with(".zip") {
        let f = std::fs::File::open(archive).map_err(io)?;
        let mut z = zip::ZipArchive::new(f).map_err(|e| PackError::Io(e.to_string()))?;
        for i in 0..z.len() {
            let mut e = z.by_index(i).map_err(|e| PackError::Io(e.to_string()))?;
            let rel = e
                .enclosed_name()
                .ok_or_else(|| PackError::Manifest(format!("unsafe archive entry {i}")))?;
            if e.is_symlink() {
                return Err(PackError::Manifest("link in zip archive".into()));
            }
            let out = tmp.join(rel);
            if e.is_dir() {
                std::fs::create_dir_all(&out).map_err(io)?;
                continue;
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(io)?;
            }
            let mut w = std::fs::File::create(&out).map_err(io)?;
            std::io::copy(&mut e, &mut w).map_err(io)?;
            #[cfg(unix)]
            if let Some(mode) = e.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o755))
                    .map_err(io)?;
            }
        }
    } else if name.ends_with(".tar.gz") {
        let f = std::fs::File::open(archive).map_err(io)?;
        let mut t = tar::Archive::new(flate2::read::GzDecoder::new(f));
        for entry in t.entries().map_err(io)? {
            let mut e = entry.map_err(io)?;
            let kind = e.header().entry_type();
            if kind.is_symlink() {
                // Library version links (libfoo.so -> libfoo.so.1): a plain sibling name only.
                let target = e.link_name().map_err(io)?.unwrap_or_default();
                let mut c = target.components();
                let plain = matches!(c.next(), Some(Component::Normal(_))) && c.next().is_none();
                if !plain {
                    return Err(PackError::Manifest("unsafe link in archive".into()));
                }
            } else if !(kind.is_file() || kind.is_dir()) {
                return Err(PackError::Manifest("unsupported archive entry".into()));
            }
            // `unpack_in` refuses absolute paths and `..`.
            if !e.unpack_in(&tmp).map_err(io)? {
                return Err(PackError::Manifest("unsafe archive entry".into()));
            }
        }
    } else {
        return Err(PackError::Manifest("not an archive".into()));
    }
    let _ = std::fs::remove_dir_all(dest);
    std::fs::rename(&tmp, dest).map_err(io)
}

/// Finds an executable by file name under `dir` (archives keep it at the top or one folder
/// down: `llama-b11960/llama-server`, `Release/whisper-cli.exe`).
pub fn find_program(dir: &Path, name: &str) -> Option<PathBuf> {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let direct = dir.join(&file);
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let p = e.path().join(&file);
        (e.path().is_dir() && p.is_file()).then_some(p)
    })
}

/// Installs a core pack from the installer's copy (`src` root): copies each file, verifies
/// it, unpacks runtime archives, and records it.
pub fn install_core(pack: &Pack, src: &Path, root: &Path) -> Result<(), PackError> {
    let mut installed = Installed::load(root);
    for f in pack.files_for(None)? {
        let to = resolve(root, &pack.id, &f.path)?;
        if !installed.has(root, &pack.id, f) {
            let from = resolve(src, &pack.id, &f.path)?;
            if !from.is_file() {
                return Err(PackError::NotBundled);
            }
            if let Some(dir) = to.parent() {
                std::fs::create_dir_all(dir).map_err(io)?;
            }
            let part = part_path(&to);
            std::fs::copy(&from, &part).map_err(io)?;
            verify_or_delete(&part, f)?;
            std::fs::rename(&part, &to).map_err(io)?;
        }
        if is_archive(&f.path) && !unpack_dir(root, &pack.id).is_dir() {
            unpack(&to, &unpack_dir(root, &pack.id))?;
        }
        installed.files.insert(f.path.clone(), f.sha256.clone());
    }
    installed.save(root)
}

pub fn part_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

/// Progress of one pack install, shared with the status command.
#[derive(Default)]
pub struct Progress {
    pub done: std::sync::atomic::AtomicU64,
    pub total: std::sync::atomic::AtomicU64,
    pub cancel: std::sync::atomic::AtomicBool,
}

/// Downloads (resumably, with HTTP Range into `<file>.part`) and verifies every file of a
/// large pack, retrying a hash mismatch up to 3 times. A file already in place (an offline
/// copy) is verified instead of downloaded.
pub async fn download_pack(
    http: &reqwest::Client,
    pack: &Pack,
    variant: Option<&str>,
    root: &Path,
    progress: &Progress,
) -> Result<(), PackError> {
    use std::sync::atomic::Ordering::Relaxed;
    let files = pack.files_for(variant)?;
    let mut installed = Installed::load(root);
    let total: u64 = files.iter().map(|f| f.size).sum();
    progress.total.store(total, Relaxed);
    let mut finished = 0u64;
    for f in files {
        let to = resolve(root, &pack.id, &f.path)?;
        if installed.has(root, &pack.id, f) {
            finished += f.size;
            progress.done.store(finished, Relaxed);
            continue;
        }
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let url = f
            .url
            .as_deref()
            .filter(|u| allowed_download_url(u))
            .ok_or(PackError::Manifest("url".into()))?;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let part = part_path(&to);
            if to.is_file() {
                // Placed here by an offline copy (or an earlier run): verify, never trust.
                let check = to.clone();
                let entry = f.clone();
                let r = tokio::task::spawn_blocking(move || verify_or_delete(&check, &entry))
                    .await
                    .map_err(|e| PackError::Io(e.to_string()))?;
                if r.is_ok() {
                    break;
                }
            }
            fetch(http, url, &part, f.size, finished, progress).await?;
            let check = part.clone();
            let entry = f.clone();
            let r = tokio::task::spawn_blocking(move || verify_or_delete(&check, &entry))
                .await
                .map_err(|e| PackError::Io(e.to_string()))?;
            match r {
                Ok(()) => {
                    std::fs::rename(&part, &to).map_err(io)?;
                    break;
                }
                Err(e @ PackError::Mismatch(_)) if attempt >= 3 => return Err(e),
                Err(PackError::Mismatch(_)) => continue,
                Err(e) => return Err(e),
            }
        }
        installed.files.insert(f.path.clone(), f.sha256.clone());
        installed.save(root)?;
        finished += f.size;
        progress.done.store(finished, Relaxed);
    }
    Ok(())
}

/// GET with `Range` from the end of `part`; appends, or restarts if the server ignores it.
async fn fetch(
    http: &reqwest::Client,
    url: &str,
    part: &Path,
    size: u64,
    before: u64,
    progress: &Progress,
) -> Result<(), PackError> {
    use std::sync::atomic::Ordering::Relaxed;
    use tokio::io::AsyncWriteExt;
    let mut have = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    if have > size {
        let _ = std::fs::remove_file(part);
        have = 0;
    }
    if have == size {
        return Ok(());
    }
    let mut res = http
        .get(url)
        .header(reqwest::header::RANGE, format!("bytes={have}-"))
        .send()
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "pack download failed");
            PackError::Network
        })?;
    let append = match res.status().as_u16() {
        206 => true,
        200 => false,
        s => {
            tracing::warn!(status = s, "pack download refused");
            return Err(PackError::Network);
        }
    };
    if !append {
        have = 0;
    }
    let mut out = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(part)
        .await
        .map_err(io)?;
    loop {
        if progress.cancel.load(Relaxed) {
            out.flush().await.map_err(io)?;
            return Err(PackError::Cancelled);
        }
        match res.chunk().await {
            Ok(Some(c)) => {
                have += c.len() as u64;
                if have > size {
                    drop(out);
                    let _ = std::fs::remove_file(part);
                    return Err(PackError::Mismatch(url.into()));
                }
                out.write_all(&c).await.map_err(io)?;
                progress.done.store(before + have, Relaxed);
            }
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(error = %e, "pack download interrupted");
                out.flush().await.map_err(io)?;
                return Err(PackError::Network);
            }
        }
    }
    out.flush().await.map_err(io)?;
    Ok(())
}

/// The download client: no credentials, https-only redirects to the upstream's CDN.
pub fn download_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .read_timeout(std::time::Duration::from_secs(120))
        .user_agent(concat!("ChuoSogoProcure/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.previous().len() >= 10 || !allowed_redirect(a.url()) {
                a.stop()
            } else {
                a.follow()
            }
        }))
        .build()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(b: &[u8]) -> String {
        hex::encode(Sha256::digest(b))
    }

    fn scratch() -> PathBuf {
        // nosemgrep: rust.lang.security.temp-dir.temp-dir
        let d = std::env::temp_dir().join(format!("procure-packs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const MANIFEST: &str = include_str!("../../../../packs/manifest.json");

    #[test]
    fn the_real_manifest_parses_and_every_entry_is_safe() {
        let m = Manifest::parse(MANIFEST.as_bytes()).unwrap();
        let ai = m.pack("ai-model").unwrap();
        assert_eq!(ai.kind, "large");
        assert_eq!(ai.recommended_variant(32 << 30), Some("standard"));
        assert_eq!(ai.recommended_variant(15 << 30), Some("standard"));
        assert_eq!(ai.recommended_variant(8 << 30), Some("light"));
        assert_eq!(ai.recommended_variant(4 << 30), Some("light"));
        let rt = m.pack("ai-runtime").unwrap().files_for(None).unwrap();
        // This platform's archive plus the licence only.
        assert_eq!(rt.len(), 2);
        assert!(
            rt.iter()
                .all(|f| f.platform.as_deref().is_none_or(|p| p == PLATFORM))
        );
        assert!(ai.files_for(None).is_err());
        assert!(matches!(m.pack("nope"), Err(PackError::UnknownPack)));
    }

    #[test]
    fn unsafe_paths_are_rejected() {
        for bad in [
            "",
            "/etc/passwd",
            "ai-model/../x",
            "ai-model/./x",
            "other/x",
            "ai-model",
            "ai-model//x",
            "ai-model\\x",
            "ai-model/C:x",
            "ai-model/x.",
            "ai-model/a|b",
            "ai-model/x\n",
        ] {
            assert!(!safe_rel_path("ai-model", bad), "{bad:?}");
        }
        assert!(safe_rel_path("fonts", "fonts/NotoSansJP[wght].ttf"));
        assert!(resolve(Path::new("/r"), "ai-model", "ai-model/../../x").is_err());
    }

    #[test]
    fn manifests_with_unsafe_entries_or_foreign_urls_are_refused() {
        let base = |path: &str, url: &str| {
            format!(
                r#"{{"schema":1,"packs":[{{"id":"ai-model","version":"1","kind":"large","title_en":"a","title_ja":"a","licence":"MIT","licence_url":"https://x","files":[{{"path":"{path}","url":{url},"sha256":"{}","size":1}}]}}]}}"#,
                "a".repeat(64)
            )
        };
        let good = base("ai-model/m.gguf", r#""https://huggingface.co/x/m.gguf""#);
        assert!(Manifest::parse(good.as_bytes()).is_ok());
        for bad in [
            base("ai-model/../m", r#""https://huggingface.co/x""#),
            base("ai-model/m", r#""http://huggingface.co/x""#),
            base("ai-model/m", r#""https://evil.test/x""#),
            base("ai-model/m", r#""https://huggingface.co:444/x""#),
            base("ai-model/m", r#""https://u:p@huggingface.co/x""#),
            base("ai-model/m", "null"),
        ] {
            assert!(Manifest::parse(bad.as_bytes()).is_err(), "{bad}");
        }
        let short = good.replace(&"a".repeat(64), "abc");
        assert!(Manifest::parse(short.as_bytes()).is_err());
    }

    #[test]
    fn redirects_only_to_the_upstream_cdn_over_https() {
        let ok = |s: &str| allowed_redirect(&reqwest::Url::parse(s).unwrap());
        assert!(ok("https://cas-bridge.xethub.hf.co/x"));
        assert!(ok("https://cdn-lfs.huggingface.co/x"));
        assert!(!ok("http://cdn-lfs.huggingface.co/x"));
        assert!(!ok("https://huggingface.co.evil.test/x"));
        assert!(!ok("https://evilhf.co/x"));
    }

    #[test]
    fn a_sha_mismatch_deletes_the_file() {
        let d = scratch();
        let p = d.join("f");
        std::fs::write(&p, b"hello").unwrap();
        let mut f = FileEntry {
            path: "x/f".into(),
            url: None,
            sha256: sha(b"hello"),
            size: 5,
            platform: None,
        };
        assert_eq!(verify_or_delete(&p, &f), Ok(()));
        f.sha256 = sha(b"other");
        assert!(matches!(
            verify_or_delete(&p, &f),
            Err(PackError::Mismatch(_))
        ));
        assert!(!p.exists());
        std::fs::write(&p, b"hello!").unwrap();
        f.sha256 = sha(b"hello");
        assert!(matches!(
            verify_or_delete(&p, &f),
            Err(PackError::Mismatch(_))
        ));
        assert!(!p.exists());
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn core_install_copies_verifies_records_and_refuses_tampered_sources() {
        let d = scratch();
        let (src, root) = (d.join("src"), d.join("root"));
        std::fs::create_dir_all(src.join("ocr-data")).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(src.join("ocr-data/jpn.traineddata"), b"data").unwrap();
        let mut pack = Pack {
            id: "ocr-data".into(),
            version: "1".into(),
            kind: "core".into(),
            title_en: String::new(),
            title_ja: String::new(),
            licence: String::new(),
            licence_url: None,
            files: vec![FileEntry {
                path: "ocr-data/jpn.traineddata".into(),
                url: None,
                sha256: sha(b"data"),
                size: 4,
                platform: None,
            }],
            variants: vec![],
        };
        install_core(&pack, &src, &root).unwrap();
        assert!(is_installed(&root, &Installed::load(&root), &pack, None));
        // A changed source with the old hash is refused and nothing new is recorded.
        std::fs::remove_file(root.join("installed.json")).unwrap();
        std::fs::write(src.join("ocr-data/jpn.traineddata"), b"evil").unwrap();
        assert!(matches!(
            install_core(&pack, &src, &root),
            Err(PackError::Mismatch(_))
        ));
        assert!(!is_installed(&root, &Installed::load(&root), &pack, None));
        pack.files[0].path = "ocr-data/missing".into();
        assert_eq!(install_core(&pack, &src, &root), Err(PackError::NotBundled));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn zip_entries_outside_the_destination_are_refused() {
        let d = scratch();
        let a = d.join("a.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&a).unwrap());
            z.start_file("../escape.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut z, b"x").unwrap();
            z.finish().unwrap();
        }
        assert!(unpack(&a, &d.join("out")).is_err());
        assert!(!d.join("escape.txt").exists());
        let b = d.join("b.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&b).unwrap());
            z.start_file(
                "Release/whisper-cli.exe",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            std::io::Write::write_all(&mut z, b"x").unwrap();
            z.finish().unwrap();
        }
        unpack(&b, &d.join("out")).unwrap();
        assert!(d.join("out/Release/whisper-cli.exe").is_file());
        std::fs::remove_dir_all(d).unwrap();
    }
}
