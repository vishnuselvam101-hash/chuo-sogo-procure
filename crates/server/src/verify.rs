//! `procure-server verify`: an integrity check for operations and restore drills.
//!
//! For every organisation: the audit log's hash chain is recomputed, and every stored file the
//! database refers to (request attachments, listing photos and documents, issued invoice PDFs)
//! must exist in the storage directory with exactly the recorded SHA-256. Read-only.

use serde::Serialize;
use uuid::Uuid;

use crate::{audit, db, storage};

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub organisations: usize,
    pub audit_events: i64,
    /// Organisations whose audit chain is broken, with the first broken sequence number.
    pub broken_chains: Vec<(Uuid, i64)>,
    pub files_checked: usize,
    /// SHA-256 (hex) referenced by the database but absent from storage.
    pub missing_files: Vec<String>,
    /// Present but with different content.
    pub corrupt_files: Vec<String>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.broken_chains.is_empty()
            && self.missing_files.is_empty()
            && self.corrupt_files.is_empty()
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Runs the check with the owner connection (row-level security still applies, so each
/// organisation is checked inside its own context).
pub async fn run(pool: &sqlx::PgPool, files: &storage::Storage) -> Result<Report, sqlx::Error> {
    let mut report = Report::default();
    let mut tx = db::auth_tx(pool).await?;
    let orgs: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM organisations ORDER BY created_at")
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    report.organisations = orgs.len();
    let mut seen = std::collections::HashSet::new();
    for org in orgs {
        let mut tx = pool.begin().await?;
        db::set_org(&mut tx, org).await?;
        // Issued documents are visible in the platform scope; drafts have no PDF.
        db::enable_platform_scope(&mut tx).await?;
        if let Some(seq) = audit::verify_chain(&mut tx, org).await? {
            report.broken_chains.push((org, seq));
        }
        let events: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE org_id = $1")
            .bind(org)
            .fetch_one(&mut *tx)
            .await?;
        report.audit_events += events;
        let hashes: Vec<Vec<u8>> = sqlx::query_scalar(
            "SELECT sha256 FROM attachments WHERE org_id = $1
             UNION SELECT sha256 FROM listing_media WHERE org_id = $1
             UNION SELECT pdf_sha256 FROM issued_invoices WHERE org_id = $1 AND pdf_sha256 IS NOT NULL",
        )
        .bind(org)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        for h in hashes {
            let Ok(hash) = <[u8; 32]>::try_from(h.as_slice()) else {
                report.corrupt_files.push(hex(&h));
                continue;
            };
            if !seen.insert(hash) {
                continue;
            }
            report.files_checked += 1;
            match files.get(&hash).await {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    report.missing_files.push(hex(&hash));
                }
                // `get` refuses content that no longer matches its hash.
                Err(_) => report.corrupt_files.push(hex(&hash)),
            }
        }
    }
    Ok(report)
}
