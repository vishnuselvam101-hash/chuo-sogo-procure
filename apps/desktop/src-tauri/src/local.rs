//! Standalone mode ("use on this PC only"): the app runs its own PostgreSQL, procure-server
//! and (when the packs are installed) llama-server, all bound to 127.0.0.1, as child
//! processes it starts and stops itself. Nothing listens on the network.
//!
//! Layout. Installed, read-only (`<resources>/full/`): `bin/procure-server[.exe]`,
//! `pgsql/` (portable PostgreSQL), `packs/manifest.json` and the core pack files.
//! Per user (`app_local_data_dir`): `pgdata/`, `packs/`, `logs/`, `storage/`, `outbox/`,
//! `work/`, `standalone.json` (ports, setup state) and `secrets.json` (generated database
//! passwords; owner-only file permissions on Unix, the per-user profile folder on Windows).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::packs::{self, Manifest, PackError};

/// Hard Windows flag: start console programs without a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Name the app gives the local model in API calls.
const LOCAL_MODEL: &str = "procure-local";
/// The local platform operator account (needed to create the factory organisation).
const ROOT_EMAIL: &str = "platform@procure.localhost";

#[derive(Clone)]
pub struct Paths {
    pub bundle: PathBuf,
    pub data: PathBuf,
}

impl Paths {
    pub fn packs(&self) -> PathBuf {
        self.data.join("packs")
    }
    fn logs(&self) -> PathBuf {
        self.data.join("logs")
    }
    pub fn bundled_packs(&self) -> PathBuf {
        self.bundle.join("packs")
    }
    fn server_exe(&self) -> PathBuf {
        self.bundle.join("bin").join(exe("procure-server"))
    }
    /// The bundled PostgreSQL; Linux test builds may use the system's.
    fn pg_bin(&self) -> Option<PathBuf> {
        let bundled = self.bundle.join("pgsql").join("bin");
        if bundled.join(exe("postgres")).is_file() {
            return Some(bundled);
        }
        if cfg!(target_os = "linux") {
            let mut found: Vec<PathBuf> = std::fs::read_dir("/usr/lib/postgresql")
                .ok()?
                .flatten()
                .map(|e| e.path().join("bin"))
                .filter(|p| p.join("postgres").is_file())
                .collect();
            found.sort();
            return found.pop();
        }
        None
    }
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Whether this installation carries the standalone bundle.
pub fn bundle_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager;
    let dir = app.path().resource_dir().ok()?.join("full");
    dir.join("bin")
        .join(exe("procure-server"))
        .is_file()
        .then_some(dir)
}

#[derive(Default, Serialize, Deserialize)]
struct Config {
    pg_port: u16,
    server_port: u16,
    llama_port: u16,
    roles_ready: bool,
    bootstrapped: bool,
    /// The ai-model variant to run (the one the user installed).
    ai_variant: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Secrets {
    pg_superuser: String,
    owner: String,
    app: String,
    root: String,
}

fn random_hex(bytes: usize) -> Result<String, String> {
    let mut b = vec![0u8; bytes];
    getrandom::fill(&mut b).map_err(|_| "random".to_string())?;
    Ok(hex::encode(b))
}

/// Writes a file readable only by this user (Unix mode 0600; on Windows the per-user
/// local app data folder already excludes other users).
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp).map_err(|e| e.to_string())?;
    std::io::Write::write_all(&mut f, bytes).map_err(|e| e.to_string())?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
}

/// Status the UI polls.
#[derive(Clone, Serialize)]
pub struct Status {
    pub available: bool,
    /// "off", "starting", "running", "failed".
    pub phase: &'static str,
    pub error: Option<String>,
    pub server_url: Option<String>,
    /// The first-run account and factory have not been created yet.
    pub needs_setup: bool,
    /// "off" (no model installed), "starting", "ready", "failed".
    pub ai: &'static str,
    /// "ready", "no_model", "no_ffmpeg".
    pub voice: &'static str,
    /// "ready", "no_data", "no_tesseract", "no_pdf_tools".
    pub ocr: &'static str,
}

impl Default for Status {
    fn default() -> Self {
        Status {
            available: false,
            phase: "off",
            error: None,
            server_url: None,
            needs_setup: false,
            ai: "off",
            voice: "no_model",
            ocr: "no_data",
        }
    }
}

#[derive(Default)]
struct Procs {
    /// PostgreSQL runs detached under `pg_ctl` (which drops administrator rights on Windows);
    /// this records that this app started it.
    pg: bool,
    server: Option<tokio::process::Child>,
    llama: Option<tokio::process::Child>,
}

/// One pack install in progress (or its last result).
#[derive(Default)]
pub struct Job {
    pub progress: packs::Progress,
    pub running: std::sync::atomic::AtomicBool,
    pub error: Mutex<Option<String>>,
}

pub struct Local {
    pub paths: OnceLock<Paths>,
    status: Mutex<Status>,
    procs: Mutex<Procs>,
    /// Serialises start, restart, setup and stop.
    op: tokio::sync::Mutex<()>,
    pub jobs: Mutex<HashMap<String, Arc<Job>>>,
    #[cfg(windows)]
    job_object: OnceLock<win32job::Job>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Local {
    pub fn new() -> Local {
        Local {
            paths: OnceLock::new(),
            status: Mutex::new(Status::default()),
            procs: Mutex::new(Procs::default()),
            op: tokio::sync::Mutex::new(()),
            jobs: Mutex::new(HashMap::new()),
            #[cfg(windows)]
            job_object: OnceLock::new(),
        }
    }

    pub fn status(&self) -> Status {
        let mut s = lock(&self.status).clone();
        s.available = self.paths.get().is_some();
        s
    }

    fn set(&self, f: impl FnOnce(&mut Status)) {
        f(&mut lock(&self.status));
    }

    pub fn manifest(&self) -> Result<Manifest, PackError> {
        let p = self.paths.get().ok_or(PackError::NotBundled)?;
        let bytes = std::fs::read(p.bundled_packs().join("manifest.json"))
            .map_err(|_| PackError::NotBundled)?;
        Manifest::parse(&bytes)
    }

    /// Windows: put this process (and so every child it starts) into a job object that kills
    /// them all when the app exits or crashes.
    #[cfg(windows)]
    fn ensure_job_object(&self) -> Result<(), String> {
        if self.job_object.get().is_some() {
            return Ok(());
        }
        let mut info = win32job::ExtendedLimitInfo::new();
        info.limit_kill_on_job_close();
        let job = win32job::Job::create_with_limit_info(&info).map_err(|e| e.to_string())?;
        job.assign_current_process().map_err(|e| e.to_string())?;
        let _ = self.job_object.set(job);
        Ok(())
    }

    /// Starts (or completes starting) everything; safe to call again.
    pub async fn start(&self) -> Result<String, String> {
        let _g = self.op.lock().await;
        self.set(|s| {
            s.phase = "starting";
            s.error = None;
        });
        match self.start_inner().await {
            Ok(url) => {
                self.set(|s| {
                    s.phase = "running";
                    s.server_url = Some(url.clone());
                });
                Ok(url)
            }
            Err(code) => {
                tracing::error!(error = %code, "standalone start failed");
                self.set(|s| {
                    s.phase = "failed";
                    s.error = Some(code.clone());
                });
                Err(code)
            }
        }
    }

    /// After a pack is installed: restart the server and model with the new settings.
    pub async fn restart_services(&self) -> Result<String, String> {
        {
            let mut p = lock(&self.procs);
            p.server = None;
            p.llama = None;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        self.start().await
    }

    async fn start_inner(&self) -> Result<String, String> {
        let paths = self.paths.get().ok_or("not_available")?.clone();
        for d in ["pgdata", "packs", "logs", "storage", "outbox", "work"] {
            std::fs::create_dir_all(paths.data.join(d)).map_err(|_| "data_dir")?;
        }
        #[cfg(windows)]
        self.ensure_job_object().map_err(|e| {
            tracing::error!(error = %e, "job object");
            "job_object"
        })?;

        // Core packs come with the installer: verify and unpack them before first use.
        let manifest = self.manifest().map_err(|e| e.code().to_string())?;
        {
            let (m, p) = (manifest.clone(), paths.clone());
            tokio::task::spawn_blocking(move || {
                for pack in m.packs.iter().filter(|p| p.kind == "core") {
                    if let Err(e) = packs::install_core(pack, &p.bundled_packs(), &p.packs()) {
                        tracing::warn!(pack = %pack.id, error = ?e, "core pack not installed");
                    }
                }
            })
            .await
            .map_err(|_| "pack_io")?;
        }

        let cfg_path = paths.data.join("standalone.json");
        let sec_path = paths.data.join("secrets.json");
        let mut cfg: Config = load_json(&cfg_path).unwrap_or_default();
        let secrets: Secrets = match load_json(&sec_path) {
            Some(s) => s,
            None => {
                let s = Secrets {
                    pg_superuser: random_hex(24)?,
                    owner: random_hex(24)?,
                    app: random_hex(24)?,
                    root: random_hex(24)?,
                };
                write_private(
                    &sec_path,
                    &serde_json::to_vec(&s).map_err(|e| e.to_string())?,
                )?;
                s
            }
        };
        let save_cfg = |c: &Config| -> Result<(), String> {
            write_private(
                &cfg_path,
                &serde_json::to_vec_pretty(c).map_err(|e| e.to_string())?,
            )
        };

        let pg_bin = paths.pg_bin().ok_or("no_postgres")?;
        let pgdata = paths.data.join("pgdata");
        let running = lock(&self.procs).pg && pg_ready(&pg_bin, cfg.pg_port).await;
        if !running {
            if !pgdata.join("PG_VERSION").is_file() {
                self.initdb(&paths, &pg_bin, &pgdata, &secrets).await?;
            }
            // A server left over from a crash (Linux) is stopped through its own pid file.
            if pgdata.join("postmaster.pid").is_file() {
                let _ = run_logged(
                    &paths,
                    "pg_ctl",
                    command(&pg_bin.join(exe("pg_ctl")))
                        .args(["stop", "-m", "fast", "-w", "-t", "20", "-D"])
                        .arg(&pgdata),
                )
                .await;
            }
            cfg.pg_port = free_port(cfg.pg_port)?;
            // pg_ctl, not postgres directly: on Windows it starts the server with a restricted
            // token, so this also works when the app was started "as administrator" (postgres
            // refuses administrator rights). The server stays inside the app's job object.
            let mut opts = format!("-p {} -c listen_addresses=127.0.0.1", cfg.pg_port);
            if cfg!(unix) {
                opts.push_str(" -c unix_socket_directories=''");
            }
            let mut c = command(&pg_bin.join(exe("pg_ctl")));
            c.args(["start", "-w", "-t", "60", "-D"])
                .arg(&pgdata)
                .arg("-l")
                .arg(paths.logs().join("postgres.log"))
                .args(["-o", &opts]);
            if !run_logged(&paths, "pg_ctl", &mut c).await? {
                return Err("postgres_start".into());
            }
            lock(&self.procs).pg = true;
            wait_pg(&pg_bin, cfg.pg_port).await?;
        }
        let owner_url = format!(
            "postgres://procure:{}@127.0.0.1:{}/procure",
            secrets.owner, cfg.pg_port
        );
        let app_url = format!(
            "postgres://procure_app:{}@127.0.0.1:{}/procure",
            secrets.app, cfg.pg_port
        );
        if !cfg.roles_ready {
            self.create_roles(&paths, &pg_bin, cfg.pg_port, &secrets)
                .await?;
            cfg.roles_ready = true;
        }
        let mut m = command(&paths.server_exe());
        m.arg("migrate").env("DATABASE_URL", &owner_url);
        if !run_logged(&paths, "migrate", &mut m).await? {
            return Err("migrate".into());
        }

        // Optional services from installed packs.
        let packs_root = paths.packs();
        let installed = packs::Installed::load(&packs_root);
        let ai_pack = manifest.pack("ai-model").ok();
        let variant = cfg.ai_variant.clone().or_else(|| {
            ai_pack.and_then(|p| {
                p.variants
                    .iter()
                    .find(|v| packs::is_installed(&packs_root, &installed, p, Some(&v.variant)))
                    .map(|v| v.variant.clone())
            })
        });
        let model = ai_pack.zip(variant.as_deref()).and_then(|(p, v)| {
            if !packs::is_installed(&packs_root, &installed, p, Some(v)) {
                return None;
            }
            p.files_for(Some(v))
                .ok()?
                .into_iter()
                .find(|f| f.path.ends_with(".gguf"))
                .and_then(|f| packs::resolve(&packs_root, &p.id, &f.path).ok())
        });
        let llama = packs::find_program(
            &packs::unpack_dir(&packs_root, "ai-runtime"),
            "llama-server",
        );
        let mut ai_url = None;
        if let (Some(model), Some(llama)) = (model, llama) {
            let have = lock(&self.procs)
                .llama
                .as_mut()
                .is_some_and(|c| matches!(c.try_wait(), Ok(None)));
            if !have {
                cfg.llama_port = free_port(cfg.llama_port)?;
                let mut c = command(&llama);
                c.arg("-m").arg(&model).args([
                    "--host",
                    "127.0.0.1",
                    "--port",
                    &cfg.llama_port.to_string(),
                    "-a",
                    LOCAL_MODEL,
                    "-c",
                    "8192",
                    "--reasoning-budget",
                    "0",
                    "--no-webui",
                ]);
                if let Some(dir) = llama.parent() {
                    c.current_dir(dir);
                }
                log_to(&paths, "llama", &mut c)?;
                let child = c.spawn().map_err(|_| "ai_start")?;
                lock(&self.procs).llama = Some(child);
                self.set(|s| s.ai = "starting");
            }
            ai_url = Some(format!("http://127.0.0.1:{}", cfg.llama_port));
        } else {
            self.set(|s| s.ai = "off");
        }

        let mut env: Vec<(String, std::ffi::OsString)> = vec![
            ("PROCURE_RUNTIME_DATABASE_URL".into(), app_url.into()),
            (
                "PROCURE_OUTBOX_DIR".into(),
                paths.data.join("outbox").into(),
            ),
            (
                "PROCURE_STORAGE_DIR".into(),
                paths.data.join("storage").into(),
            ),
            ("PROCURE_WORK_DIR".into(), paths.data.join("work").into()),
        ];
        if let Some(u) = &ai_url {
            env.push(("PROCURE_AI_PROVIDER".into(), "local".into()));
            env.push(("PROCURE_AI_BASE_URL".into(), format!("{u}/v1").into()));
            env.push(("PROCURE_AI_MODEL".into(), LOCAL_MODEL.into()));
        }
        let font = packs_root.join("fonts").join("NotoSansJP[wght].ttf");
        if font.is_file() {
            env.push(("PROCURE_PDF_FONT".into(), font.into()));
        }
        // Voice notes: whisper.cpp + the voice model; the server also needs ffmpeg.
        let whisper = packs::find_program(
            &packs::unpack_dir(&packs_root, "voice-runtime"),
            "whisper-cli",
        );
        let voice_model = manifest
            .pack("voice-model")
            .ok()
            .filter(|p| packs::is_installed(&packs_root, &installed, p, None))
            .and_then(|p| {
                p.files_for(None)
                    .ok()?
                    .into_iter()
                    .find(|f| f.path.ends_with(".bin"))
                    .and_then(|f| packs::resolve(&packs_root, &p.id, &f.path).ok())
            });
        // Helper programs: the bundled Windows builds (core packs), else the system's on PATH.
        let mut tool = |pack: &str, name: &str, var: &str| -> bool {
            match packs::find_program(&packs::unpack_dir(&packs_root, pack), name) {
                Some(p) => {
                    env.push((var.into(), p.into()));
                    true
                }
                None => on_path(name),
            }
        };
        let ffmpeg = tool("media-tools", "ffmpeg", "PROCURE_FFMPEG");
        let tesseract = tool("ocr-runtime", "tesseract", "PROCURE_TESSERACT");
        let pdftoppm = tool("pdf-tools", "pdftoppm", "PROCURE_PDFTOPPM");
        let pdftotext = tool("pdf-tools", "pdftotext", "PROCURE_PDFTOTEXT");
        let voice = match (whisper, voice_model) {
            (Some(w), Some(m)) if ffmpeg => {
                env.push(("PROCURE_STT_COMMAND".into(), w.into()));
                env.push(("PROCURE_STT_MODEL".into(), m.into()));
                "ready"
            }
            (Some(_), Some(_)) => "no_ffmpeg",
            _ => "no_model",
        };
        // Scanned documents and FAXes: tesseract with the installed tessdata_best; scanned
        // PDFs also need pdftoppm (and text PDFs pdftotext).
        let tessdata = packs_root.join("ocr-data");
        let ocr = if !tessdata.join("jpn.traineddata").is_file() {
            "no_data"
        } else {
            env.push(("TESSDATA_PREFIX".into(), tessdata.into()));
            match (tesseract, pdftoppm && pdftotext) {
                (true, true) => "ready",
                (true, false) => "no_pdf_tools",
                (false, _) => "no_tesseract",
            }
        };

        let have_server = lock(&self.procs)
            .server
            .as_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(None)));
        if !have_server {
            cfg.server_port = free_port(cfg.server_port)?;
            let mut c = command(&paths.server_exe());
            c.arg("serve")
                .env("PROCURE_BIND", format!("127.0.0.1:{}", cfg.server_port))
                .envs(env)
                .current_dir(&paths.data);
            log_to(&paths, "server", &mut c)?;
            let child = c.spawn().map_err(|_| "server_start")?;
            lock(&self.procs).server = Some(child);
        }
        save_cfg(&cfg)?;
        let url = format!("http://127.0.0.1:{}", cfg.server_port);
        wait_http(&format!("{url}/health"), Duration::from_secs(40))
            .await
            .map_err(|_| "server_start")?;
        self.set(|s| {
            s.needs_setup = !cfg.bootstrapped;
            s.voice = voice;
            s.ocr = ocr;
        });
        if let Some(u) = ai_url {
            self.watch_ai(u);
        }
        Ok(url)
    }

    fn watch_ai(&self, base: String) {
        let me: &'static Local = match LOCAL.get() {
            Some(l) => l,
            None => return,
        };
        tauri::async_runtime::spawn(async move {
            // Loading a 5–9 GB model from disk can take minutes on a slow drive.
            let ok = wait_http(&format!("{base}/health"), Duration::from_secs(600))
                .await
                .is_ok();
            me.set(|s| s.ai = if ok { "ready" } else { "failed" });
        });
    }

    async fn initdb(
        &self,
        paths: &Paths,
        bin: &Path,
        pgdata: &Path,
        s: &Secrets,
    ) -> Result<(), String> {
        let pwfile = paths.data.join("pwfile.tmp");
        write_private(&pwfile, s.pg_superuser.as_bytes())?;
        let mut c = command(&bin.join(exe("initdb")));
        c.arg("-D")
            .arg(pgdata)
            .args([
                "-U",
                "postgres",
                "-A",
                "scram-sha-256",
                "-E",
                "UTF8",
                "--no-locale",
            ])
            .arg(format!("--pwfile={}", pwfile.display()));
        let ok = run_logged(paths, "initdb", &mut c).await;
        let _ = std::fs::remove_file(&pwfile);
        if ok? {
            Ok(())
        } else {
            let _ = std::fs::remove_dir_all(pgdata);
            Err("initdb".into())
        }
    }

    /// The same roles as scripts/db-setup.sh; passwords reach psql through its environment.
    async fn create_roles(
        &self,
        paths: &Paths,
        bin: &Path,
        port: u16,
        s: &Secrets,
    ) -> Result<(), String> {
        const SQL: &str = r"\getenv owner_pw PROCURE_OWNER_PASSWORD
\getenv app_pw PROCURE_APP_PASSWORD
SELECT 'CREATE ROLE procure LOGIN NOSUPERUSER NOBYPASSRLS' WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'procure') \gexec
SELECT 'CREATE ROLE procure_runtime NOLOGIN NOSUPERUSER NOBYPASSRLS' WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'procure_runtime') \gexec
SELECT 'CREATE ROLE procure_app LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB' WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'procure_app') \gexec
ALTER ROLE procure PASSWORD :'owner_pw';
ALTER ROLE procure_app PASSWORD :'app_pw';
GRANT procure_runtime TO procure_app;
SELECT 'CREATE DATABASE procure OWNER procure ENCODING ''UTF8'' TEMPLATE template0' WHERE NOT EXISTS (SELECT 1 FROM pg_database WHERE datname = 'procure') \gexec
";
        let sql_file = paths.data.join("roles.sql.tmp");
        write_private(&sql_file, SQL.as_bytes())?;
        let mut c = command(&bin.join(exe("psql")));
        c.args([
            "-X",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "-h",
            "127.0.0.1",
            "-p",
            &port.to_string(),
            "-U",
            "postgres",
            "-d",
            "postgres",
            "-f",
        ])
        .arg(&sql_file)
        .env("PGPASSWORD", &s.pg_superuser)
        .env("PROCURE_OWNER_PASSWORD", &s.owner)
        .env("PROCURE_APP_PASSWORD", &s.app);
        let ok = run_logged(paths, "roles", &mut c).await;
        let _ = std::fs::remove_file(&sql_file);
        if ok? { Ok(()) } else { Err("roles".into()) }
    }

    /// First run: the local operator account, then the user's factory and its administrator
    /// (the user), through the server's own API exactly as on a hosted server.
    pub async fn setup(
        &self,
        http: &reqwest::Client,
        factory: &str,
        name: &str,
        email: &str,
        password: &str,
        locale: &str,
    ) -> Result<(), String> {
        let _g = self.op.lock().await;
        let paths = self.paths.get().ok_or("not_available")?.clone();
        let cfg_path = paths.data.join("standalone.json");
        let mut cfg: Config = load_json(&cfg_path).ok_or("not_started")?;
        if cfg.bootstrapped {
            return Err("already_set_up".into());
        }
        let s: Secrets = load_json(&paths.data.join("secrets.json")).ok_or("not_started")?;
        let base = format!("http://127.0.0.1:{}/api/v1", cfg.server_port);
        // The operator account may exist from an interrupted earlier attempt.
        let root = match login(http, &base, ROOT_EMAIL, &s.root).await {
            Ok(t) => t,
            Err(_) => {
                let owner_url = format!(
                    "postgres://procure:{}@127.0.0.1:{}/procure",
                    s.owner, cfg.pg_port
                );
                let mut c = command(&paths.server_exe());
                c.args([
                    "bootstrap",
                    "Chuo-Sogo Procure (this PC)",
                    ROOT_EMAIL,
                    "Local operator",
                ])
                .env("DATABASE_URL", owner_url)
                .stdin(Stdio::null())
                .stderr(Stdio::null());
                let out = c.output().await.map_err(|_| "bootstrap")?;
                let token = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .last()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if !out.status.success() || token.is_empty() {
                    return Err("bootstrap".into());
                }
                call(
                    http,
                    &base,
                    "POST",
                    "/auth/set-password",
                    None,
                    serde_json::json!({ "token": token, "password": s.root }),
                )
                .await?;
                login(http, &base, ROOT_EMAIL, &s.root).await?
            }
        };
        let org = call(
            http,
            &base,
            "POST",
            "/platform/organisations",
            Some(&root),
            serde_json::json!({
                "name": factory, "admin_email": email, "admin_name": name, "default_locale": locale,
            }),
        )
        .await?;
        let org_id = org
            .get("org_id")
            .and_then(|v| v.as_str())
            .ok_or("setup")?
            .to_string();
        let token = mailed_token(&paths.data.join("outbox"), email).ok_or("setup")?;
        call(
            http,
            &base,
            "POST",
            "/auth/set-password",
            None,
            serde_json::json!({ "token": token, "password": password }),
        )
        .await?;
        let admin = login(http, &base, email, password).await?;
        call(
            http,
            &base,
            "POST",
            &format!("/invitations/{org_id}/accept"),
            Some(&admin),
            serde_json::Value::Null,
        )
        .await?;
        let _ = call(
            http,
            &base,
            "POST",
            "/auth/logout",
            Some(&admin),
            serde_json::Value::Null,
        )
        .await;
        let _ = call(
            http,
            &base,
            "POST",
            "/auth/logout",
            Some(&root),
            serde_json::Value::Null,
        )
        .await;
        cfg.bootstrapped = true;
        write_private(
            &cfg_path,
            &serde_json::to_vec_pretty(&cfg).map_err(|e| e.to_string())?,
        )?;
        self.set(|st| st.needs_setup = false);
        Ok(())
    }

    pub fn set_ai_variant(&self, variant: &str) {
        if let Some(p) = self.paths.get() {
            let path = p.data.join("standalone.json");
            let mut cfg: Config = load_json(&path).unwrap_or_default();
            cfg.ai_variant = Some(variant.to_string());
            let _ = serde_json::to_vec_pretty(&cfg)
                .map_err(|e| e.to_string())
                .and_then(|b| write_private(&path, &b));
        }
    }

    /// Stops everything (app exit). PostgreSQL gets a clean fast shutdown first.
    pub async fn stop(&self) {
        let (server, llama, pg) = {
            let mut p = lock(&self.procs);
            (p.server.take(), p.llama.take(), std::mem::take(&mut p.pg))
        };
        for mut c in [server, llama].into_iter().flatten() {
            let _ = c.kill().await;
        }
        if pg
            && let Some(paths) = self.paths.get()
            && let Some(bin) = paths.pg_bin()
        {
            let mut c = command(&bin.join(exe("pg_ctl")));
            c.args(["stop", "-m", "fast", "-w", "-t", "20", "-D"])
                .arg(paths.data.join("pgdata"));
            let _ = run_logged(paths, "pg_ctl", &mut c).await;
        }
        self.set(|s| {
            s.phase = "off";
            s.ai = "off";
        });
    }
}

/// The one instance (managed by Tauri, reachable from background tasks).
pub static LOCAL: OnceLock<Local> = OnceLock::new();

/// A child process with a clean environment (no inherited server settings), no window, no
/// stdin, killed when dropped.
fn command(program: &Path) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(program);
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().to_string();
        if k.starts_with("PROCURE_")
            || k.starts_with("PG")
            || k == "DATABASE_URL"
            || k == "TESSDATA_PREFIX"
        {
            c.env_remove(k);
        }
    }
    c.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

fn log_to(paths: &Paths, name: &str, c: &mut tokio::process::Command) -> Result<(), String> {
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.logs().join(format!("{name}.log")))
        .map_err(|_| "log")?;
    let f2 = f.try_clone().map_err(|_| "log")?;
    c.stdout(Stdio::from(f)).stderr(Stdio::from(f2));
    Ok(())
}

/// Runs a one-shot command with its output in `logs/<name>.log`; true on success.
async fn run_logged(
    paths: &Paths,
    name: &str,
    c: &mut tokio::process::Command,
) -> Result<bool, String> {
    log_to(paths, name, c)?;
    let status = c.status().await.map_err(|_| format!("{name}_start"))?;
    Ok(status.success())
}

/// `preferred` if it is free on 127.0.0.1, else any free port.
fn free_port(preferred: u16) -> Result<u16, String> {
    if preferred != 0 && std::net::TcpListener::bind(("127.0.0.1", preferred)).is_ok() {
        return Ok(preferred);
    }
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|_| "port")?;
    Ok(l.local_addr().map_err(|_| "port")?.port())
}

fn on_path(program: &str) -> bool {
    let file = exe(program);
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(&file).is_file()))
}

async fn pg_ready(bin: &Path, port: u16) -> bool {
    port != 0
        && command(&bin.join(exe("pg_isready")))
            .args(["-q", "-h", "127.0.0.1", "-p", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok_and(|s| s.success())
}

async fn wait_pg(bin: &Path, port: u16) -> Result<(), String> {
    for _ in 0..120 {
        if pg_ready(bin, port).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err("postgres_start".into())
}

async fn wait_http(url: &str, max: Duration) -> Result<(), ()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| ())?;
    let deadline = tokio::time::Instant::now() + max;
    while tokio::time::Instant::now() < deadline {
        if client
            .get(url)
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(())
}

async fn call(
    http: &reqwest::Client,
    base: &str,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let m = reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| "setup")?;
    let mut req = http.request(m, format!("{base}{path}"));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    if !body.is_null() {
        req = req.json(&body);
    }
    let res = req.send().await.map_err(|_| "network")?;
    let status = res.status();
    let v = res
        .json::<serde_json::Value>()
        .await
        .unwrap_or(serde_json::Value::Null);
    if status.is_success() {
        Ok(v)
    } else {
        // The server's own error code (e.g. a weak password) goes back to the form.
        let code = v
            .pointer("/error/code")
            .and_then(|c| c.as_str())
            .unwrap_or("setup");
        let detail = v.pointer("/error/detail").and_then(|c| c.as_str());
        tracing::warn!(%path, status = status.as_u16(), code, "setup call refused");
        Err(match detail {
            Some(d) => format!("{code}/{d}"),
            None => code.to_string(),
        })
    }
}

async fn login(
    http: &reqwest::Client,
    base: &str,
    email: &str,
    password: &str,
) -> Result<String, String> {
    let v = call(
        http,
        base,
        "POST",
        "/auth/login",
        None,
        serde_json::json!({ "email": email, "password": password }),
    )
    .await?;
    v.get("token")
        .and_then(|t| t.as_str())
        .map(str::to_owned)
        .ok_or_else(|| "setup".into())
}

/// The activation token the local server "mailed" (test-mode outbox file) to `email`.
pub fn mailed_token(outbox: &Path, email: &str) -> Option<String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(outbox)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    files.sort();
    let to = format!("To: {email}\n");
    files.iter().rev().find_map(|f| {
        let text = std::fs::read_to_string(f).ok()?;
        if !text.starts_with(&to) {
            return None;
        }
        text.lines()
            .find_map(|l| l.strip_prefix("token: "))
            .map(|t| t.trim().to_string())
    })
}

/// Total RAM and free space on the disk holding `dir`.
pub fn machine(dir: &Path) -> (u64, u64) {
    use sysinfo::{Disks, MemoryRefreshKind, RefreshKind, System};
    let ram = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()),
    )
    .map_or(0, |s| s.total_memory());
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let Ok(disks) = Disks::new_with_refreshed_list() else {
        return (ram, 0);
    };
    let free = disks
        .iter()
        .filter(|d| dir.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
        .unwrap_or(0);
    (ram, free)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_activation_token_is_read_only_from_mail_to_that_address() {
        // nosemgrep: rust.lang.security.temp-dir.temp-dir
        let d = std::env::temp_dir().join(format!("procure-outbox-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("1.txt"),
            "To: a@x.test\nTemplate: activation\ntoken: AAA\n",
        )
        .unwrap();
        std::fs::write(
            d.join("2.txt"),
            "To: b@x.test\nTemplate: activation\ntoken: BBB\n",
        )
        .unwrap();
        std::fs::write(
            d.join("3.txt"),
            "To: a@x.test.evil\nTemplate: activation\ntoken: CCC\n",
        )
        .unwrap();
        assert_eq!(mailed_token(&d, "a@x.test").as_deref(), Some("AAA"));
        assert_eq!(mailed_token(&d, "b@x.test").as_deref(), Some("BBB"));
        assert_eq!(mailed_token(&d, "c@x.test"), None);
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn generated_secrets_are_random_and_private() {
        let a = random_hex(24).unwrap();
        assert_eq!(a.len(), 48);
        assert_ne!(a, random_hex(24).unwrap());
        // nosemgrep: rust.lang.security.temp-dir.temp-dir
        let p = std::env::temp_dir().join(format!("procure-secret-{}", uuid::Uuid::new_v4()));
        write_private(&p, b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn a_taken_port_is_replaced() {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let taken = l.local_addr().unwrap().port();
        assert_ne!(free_port(taken).unwrap(), taken);
    }

    /// End to end on this machine, run explicitly (PostgreSQL refuses root, so as a normal
    /// user): real PostgreSQL + procure-server + llama-server from a staged bundle, real pack
    /// verification and small real HTTPS downloads, first-run setup, one AI extraction.
    ///   PROCURE_E2E_BUNDLE=<staged full/> PROCURE_E2E_DATA=<empty dir> <test binary> --ignored standalone
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs a staged bundle, the packs and a non-root user"]
    async fn standalone_end_to_end() {
        let env = |k: &str| PathBuf::from(std::env::var(k).unwrap());
        let l: &'static Local = LOCAL.get_or_init(Local::new);
        l.paths
            .set(Paths {
                bundle: env("PROCURE_E2E_BUNDLE"),
                data: env("PROCURE_E2E_DATA"),
            })
            .ok();
        let paths = l.paths.get().unwrap().clone();
        let m = l.manifest().unwrap();
        // Large packs: big files pre-placed (offline copy) are verified; licences download.
        let dl = packs::download_client().unwrap();
        for (id, v) in [("ai-model", Some("light")), ("voice-model", None)] {
            let p = m.pack(id).unwrap();
            let t0 = std::time::Instant::now();
            packs::download_pack(&dl, p, v, &paths.packs(), &packs::Progress::default())
                .await
                .unwrap();
            assert!(packs::is_installed(
                &paths.packs(),
                &packs::Installed::load(&paths.packs()),
                p,
                v
            ));
            println!("pack {id} verified/downloaded in {:?}", t0.elapsed());
        }
        let t0 = std::time::Instant::now();
        let url = l.start().await.unwrap();
        println!(
            "local stack up at {url} in {:?}: {:?}",
            t0.elapsed(),
            l.status().phase
        );
        assert!(l.status().needs_setup);
        // This machine has tesseract, poppler-utils and ffmpeg installed (Windows: bundled).
        assert_eq!((l.status().ocr, l.status().voice), ("ready", "ready"));
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap();
        let (email, pw) = ("yamada@factory.e2e", "correct-horse-battery-1");
        l.setup(&http, "株式会社テスト工業", "山田 太郎", email, pw, "ja")
            .await
            .unwrap();
        assert!(!l.status().needs_setup);
        assert_eq!(
            l.setup(&http, "x", "y", "z@x.e2e", pw, "ja").await,
            Err("already_set_up".into())
        );
        let base = format!("{url}/api/v1");
        let token = login(&http, &base, email, pw).await.unwrap();
        let me = call(
            &http,
            &base,
            "GET",
            "/me",
            Some(&token),
            serde_json::Value::Null,
        )
        .await
        .unwrap();
        let org = me
            .pointer("/memberships/0/org_id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        println!(
            "signed in as {email}, org {}",
            me.pointer("/memberships/0/org_name").unwrap()
        );
        let req = call(
            &http,
            &base,
            "POST",
            &format!("/orgs/{org}/requests"),
            Some(&token),
            serde_json::json!({ "title": "ベアリング手配", "currency": "JPY" }),
        )
        .await
        .unwrap();
        let rid = req.get("id").and_then(|v| v.as_str()).unwrap().to_string();
        for _ in 0..1200 {
            if l.status().ai != "starting" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        assert_eq!(l.status().ai, "ready");
        let t0 = std::time::Instant::now();
        let s = call(&http, &base, "POST", &format!("/orgs/{org}/requests/{rid}/ai/extract-items"), Some(&token),
            serde_json::json!({ "text": "深溝玉軸受 6204-2RS を10個、来週金曜までにお願いします。" })).await.unwrap();
        println!("AI extraction in {:?}: {}", t0.elapsed(), s);
        assert_eq!(
            s.get("status").and_then(|v| v.as_str()),
            Some("pending"),
            "{s}"
        );
        l.stop().await;
        let port = url.rsplit(':').next().unwrap().parse::<u16>().unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(
            std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
            "server still running"
        );
        println!("all stopped");
    }
}
