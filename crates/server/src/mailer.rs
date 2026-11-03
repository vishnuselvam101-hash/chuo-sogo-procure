//! Outgoing account mail (activation, password reset).
//! Only test-mode mailers exist: nothing leaves the machine until a real
//! provider is configured and approved.

use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mail {
    pub to: String,
    pub template: &'static str,
    pub locale: String,
    /// Template variables, e.g. the one-time token. Never logged.
    pub vars: Vec<(&'static str, String)>,
}

pub trait Mailer: Send + Sync {
    fn send(&self, mail: Mail) -> Result<(), std::io::Error>;
}

/// Writes each mail as a file into a local outbox directory (dev / test mode).
pub struct FileMailer {
    pub dir: PathBuf,
}

impl Mailer for FileMailer {
    fn send(&self, mail: Mail) -> Result<(), std::io::Error> {
        std::fs::create_dir_all(&self.dir)?;
        let name = format!(
            "{}-{}.txt",
            chrono::Utc::now().format("%Y%m%dT%H%M%S%.6f"),
            mail.template
        );
        let mut body = format!(
            "To: {}\nTemplate: {}\nLocale: {}\n",
            mail.to, mail.template, mail.locale
        );
        for (k, v) in &mail.vars {
            body.push_str(&format!("{k}: {v}\n"));
        }
        // Contains one-time tokens: owner-readable only.
        // (On Windows the per-user data directory already limits access to its owner.)
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(self.dir.join(name))?;
        f.write_all(body.as_bytes())
    }
}

/// Keeps mails in memory so tests can read tokens.
#[derive(Default)]
pub struct MemoryMailer {
    pub sent: Mutex<Vec<Mail>>,
}

impl Mailer for MemoryMailer {
    fn send(&self, mail: Mail) -> Result<(), std::io::Error> {
        self.sent
            .lock()
            .map_err(|_| std::io::Error::other("mailer lock poisoned"))?
            .push(mail);
        Ok(())
    }
}
