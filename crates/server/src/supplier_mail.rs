//! Live supplier mail and FAX (founder approval 2026-11-03). Off unless the server is
//! configured (`PROCURE_SUPPLIER_MAIL=live` and an SMTP account); otherwise every message
//! stays a test-mode record as before.
//!
//! - **Out:** dispatch and reminders queue messages in `outbox`; [`send_queued`] sends them
//!   over SMTP, retries with backoff and marks them sent or failed. FAX goes to a mail-to-FAX
//!   gateway (`PROCURE_FAX_GATEWAY`, e.g. `{number}@fax.example.jp`), so it is mail too.
//! - **In:** [`poll_inbox`] reads new messages from the IMAP mailbox read-only (EXAMINE; it
//!   never changes or deletes mail), keeps each one in `mail_inbox`, and when the subject or
//!   the thread carries a reply token, keeps it as the supplier's document on its RFQ and
//!   opens a task for our staff. Anything else (a FAX, a new thread) waits in the staff
//!   inbox to be assigned. Nothing read from a mailbox is ever acted on without a person.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use futures_util::future::BoxFuture;
use uuid::Uuid;

use crate::AppState;
use crate::api::records::{Channel, KeptFile, keep_document};
use crate::db;

/// Messages one poll reads at most; the rest come next time.
const POLL_BATCH: usize = 50;
/// Bytes one poll downloads at most.
pub const POLL_BYTES: u64 = 50 * 1024 * 1024;
const SEND_BATCH: i64 = 20;
const MAX_ATTEMPTS: i32 = 6;

pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}

pub struct ImapConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub mailbox: String,
    pub poll: Duration,
    /// Messages received before this day are never read (the mailbox may hold years).
    pub since: NaiveDate,
}

/// Sends one formatted message.
pub trait Transport: Send + Sync {
    fn send<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
        raw: &'a [u8],
    ) -> BoxFuture<'a, Result<(), String>>;
}

/// One message in the mailbox: its UID, size, and content unless it was not downloaded
/// (empty, or larger than 10 MB).
pub struct Fetched {
    pub uid: u32,
    pub size: u64,
    pub raw: Option<Vec<u8>>,
}

/// Reads messages after a UID from the mailbox, without changing it.
pub trait Inbox: Send + Sync {
    /// (UIDVALIDITY, messages in UID order) with UID ≥ `from_uid`, received on or after
    /// `since`. Sizes are checked before anything is downloaded; one call downloads at most
    /// [`POLL_BYTES`], and stops before the message that would exceed it (it comes next time).
    fn fetch<'a>(
        &'a self,
        from_uid: u32,
        since: NaiveDate,
    ) -> BoxFuture<'a, Result<(u32, Vec<Fetched>), String>>;
}

pub struct SupplierMail {
    /// `Name <address>` our messages come from.
    pub from: String,
    pub reply_to: Option<String>,
    /// `{number}` is replaced by the FAX number (digits only).
    pub fax_gateway: Option<String>,
    /// Senders (addresses or `@domain`) whose messages are received FAXes.
    pub fax_senders: Vec<String>,
    /// When set, email only to these domains (a pilot with chosen suppliers).
    pub allow_domains: Vec<String>,
    pub transport: Arc<dyn Transport>,
    pub inbox: Option<Arc<dyn Inbox>>,
    pub imap: Option<ImapConfig>,
}

fn env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn list(key: &str) -> Vec<String> {
    env(key)
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

impl SupplierMail {
    /// `None` (test mode) unless `PROCURE_SUPPLIER_MAIL=live`; live without a complete SMTP
    /// account is an error, so a half-configured server never starts.
    pub fn from_env() -> Result<Option<Self>, String> {
        if env("PROCURE_SUPPLIER_MAIL").as_deref() != Some("live") {
            return Ok(None);
        }
        let need =
            |k: &str| env(k).ok_or_else(|| format!("{k} is required for live supplier mail"));
        let port = |k: &str, d: u16| -> Result<u16, String> {
            env(k).map_or(Ok(d), |v| v.parse().map_err(|_| format!("{k}: not a port")))
        };
        let smtp = SmtpConfig {
            host: need("PROCURE_SMTP_HOST")?,
            port: port("PROCURE_SMTP_PORT", 465)?,
            user: need("PROCURE_SMTP_USER")?,
            password: need("PROCURE_SMTP_PASSWORD")?,
        };
        let from = need("PROCURE_MAIL_FROM")?;
        let imap = match env("PROCURE_IMAP_HOST") {
            None => None,
            Some(host) => Some(ImapConfig {
                host,
                port: port("PROCURE_IMAP_PORT", 993)?,
                user: env("PROCURE_IMAP_USER").unwrap_or_else(|| smtp.user.clone()),
                password: env("PROCURE_IMAP_PASSWORD").unwrap_or_else(|| smtp.password.clone()),
                mailbox: env("PROCURE_IMAP_MAILBOX").unwrap_or_else(|| "INBOX".into()),
                poll: Duration::from_secs(
                    env("PROCURE_IMAP_POLL_SECONDS")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(120u64)
                        .max(30),
                ),
                since: match env("PROCURE_IMAP_SINCE") {
                    Some(d) => NaiveDate::parse_from_str(&d, "%Y-%m-%d")
                        .map_err(|_| "PROCURE_IMAP_SINCE: YYYY-MM-DD".to_string())?,
                    None => Utc::now().date_naive(),
                },
            }),
        };
        let fax_gateway = env("PROCURE_FAX_GATEWAY");
        if let Some(g) = &fax_gateway
            && !(g.contains("{number}") && g.contains('@'))
        {
            return Err("PROCURE_FAX_GATEWAY must look like {number}@gateway".into());
        }
        let transport: Arc<dyn Transport> = Arc::new(SmtpTransport::new(&smtp)?);
        let inbox: Option<Arc<dyn Inbox>> = imap.as_ref().map(|c| {
            Arc::new(ImapInbox {
                host: c.host.clone(),
                port: c.port,
                user: c.user.clone(),
                password: c.password.clone(),
                mailbox: c.mailbox.clone(),
            }) as Arc<dyn Inbox>
        });
        Ok(Some(Self {
            from,
            reply_to: env("PROCURE_MAIL_REPLY_TO"),
            fax_gateway,
            fax_senders: list("PROCURE_FAX_SENDERS"),
            allow_domains: list("PROCURE_SUPPLIER_MAIL_ALLOW"),
            transport,
            inbox,
            imap,
        }))
    }

    /// Where a message to this supplier goes, or why it cannot be sent.
    pub fn address(
        &self,
        channel: &str,
        email: Option<&str>,
        fax: Option<&str>,
    ) -> Result<(&'static str, String), &'static str> {
        if channel == "fax" {
            let gateway = self.fax_gateway.as_deref().ok_or("fax_not_configured")?;
            let digits: String = fax
                .ok_or("supplier_without_fax")?
                .chars()
                .filter(char::is_ascii_digit)
                .collect();
            return Ok(("fax", gateway.replace("{number}", &digits)));
        }
        let email = email.ok_or("supplier_without_email")?.to_lowercase();
        let domain = email.rsplit_once('@').map(|(_, d)| d).unwrap_or_default();
        if !self.allow_domains.is_empty() && !self.allow_domains.iter().any(|d| d == domain) {
            return Err("address_not_allowed");
        }
        Ok(("email", email))
    }

    fn is_fax_sender(&self, from: &str) -> bool {
        let from = from.to_lowercase();
        self.fax_senders.iter().any(|s| {
            if s.starts_with('@') {
                from.ends_with(s.as_str())
            } else {
                from == *s
            }
        })
    }
}

/// A new reply token: 12 hex digits, unguessable enough to match replies, short enough for a
/// subject line.
pub fn reply_token() -> String {
    Uuid::new_v4().simple().to_string()[..12].to_string()
}

/// The subject of a quote request or reminder; replies keep it ("Re: …").
pub fn subject(kind: &str, number: &str, token: &str) -> String {
    match kind {
        "reminder" => format!("【再送】見積依頼 / Reminder: RFQ {number} [#{token}]"),
        "order" => format!("注文書 / Purchase order {number} [#{token}]"),
        _ => format!("見積依頼 / Request for quotation {number} [#{token}]"),
    }
}

/// The reply token in a subject (`[#0123456789ab]`), if any.
pub fn token_in(subject: &str) -> Option<String> {
    let bytes = subject.as_bytes();
    subject.match_indices("[#").find_map(|(at, _)| {
        let t = subject.get(at + 2..at + 14)?;
        (bytes.get(at + 14) == Some(&b']')
            && t.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()))
        .then(|| t.to_string())
    })
}

/// Formats one message. Returns (Message-ID, raw bytes).
pub fn format_message(
    m: &SupplierMail,
    to: &str,
    subject: &str,
    body: &str,
    attach_body: bool,
) -> Result<(String, Vec<u8>), String> {
    use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart, header::ContentType};
    let from: Mailbox = m
        .from
        .parse()
        .map_err(|e| format!("PROCURE_MAIL_FROM: {e}"))?;
    let domain = from.email.domain().to_string();
    let id = format!("<{}@{domain}>", Uuid::now_v7().simple());
    let mut b = lettre::Message::builder()
        .from(from)
        .to(to.parse::<Mailbox>().map_err(|e| format!("to: {e}"))?)
        .subject(subject)
        .message_id(Some(id.clone()));
    if let Some(r) = &m.reply_to {
        b = b.reply_to(r.parse::<Mailbox>().map_err(|e| format!("reply-to: {e}"))?);
    }
    let text = SinglePart::builder()
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string());
    let msg = if attach_body {
        // FAX gateways send the attachment as the page.
        b.multipart(MultiPart::mixed().singlepart(text).singlepart(
            Attachment::new("rfq.txt".into()).body(body.to_string(), ContentType::TEXT_PLAIN),
        ))
    } else {
        b.singlepart(text)
    }
    .map_err(|e| e.to_string())?;
    Ok((id, msg.formatted()))
}

pub struct SmtpTransport {
    inner: lettre::AsyncSmtpTransport<lettre::Tokio1Executor>,
}

impl SmtpTransport {
    fn new(c: &SmtpConfig) -> Result<Self, String> {
        use lettre::transport::smtp::authentication::Credentials;
        let builder = if c.port == 465 {
            lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::relay(&c.host)
        } else {
            lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::starttls_relay(&c.host)
        }
        .map_err(|e| format!("PROCURE_SMTP_HOST: {e}"))?;
        Ok(Self {
            inner: builder
                .port(c.port)
                .credentials(Credentials::new(c.user.clone(), c.password.clone()))
                .timeout(Some(Duration::from_secs(30)))
                .build(),
        })
    }
}

impl Transport for SmtpTransport {
    fn send<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
        raw: &'a [u8],
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            use lettre::AsyncTransport;
            use lettre::address::Envelope;
            let from: lettre::message::Mailbox = from.parse().map_err(|e| format!("{e}"))?;
            let to: lettre::Address = to.parse().map_err(|e| format!("{e}"))?;
            let envelope = Envelope::new(Some(from.email), vec![to]).map_err(|e| e.to_string())?;
            self.inner
                .send_raw(&envelope, raw)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
    }
}

pub struct ImapInbox {
    host: String,
    port: u16,
    user: String,
    password: String,
    mailbox: String,
}

impl Inbox for ImapInbox {
    fn fetch<'a>(
        &'a self,
        from_uid: u32,
        since: NaiveDate,
    ) -> BoxFuture<'a, Result<(u32, Vec<Fetched>), String>> {
        Box::pin(async move {
            use futures_util::TryStreamExt;
            use tokio_rustls::rustls;
            let roots = rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            };
            let config = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
            let name = rustls::pki_types::ServerName::try_from(self.host.clone())
                .map_err(|e| e.to_string())?;
            let tcp = tokio::time::timeout(
                Duration::from_secs(30),
                tokio::net::TcpStream::connect((self.host.as_str(), self.port)),
            )
            .await
            .map_err(|_| "connect timeout".to_string())?
            .map_err(|e| e.to_string())?;
            let tls = tokio_rustls::TlsConnector::from(Arc::new(config))
                .connect(name, tcp)
                .await
                .map_err(|e| e.to_string())?;
            let mut client = async_imap::Client::new(tls);
            client
                .read_response()
                .await
                .map_err(|e| e.to_string())?
                .ok_or("no greeting")?;
            let mut session = client
                .login(&self.user, &self.password)
                .await
                .map_err(|(e, _)| e.to_string())?;
            // Read-only: the mailbox is never changed.
            let mb = session
                .examine(&self.mailbox)
                .await
                .map_err(|e| e.to_string())?;
            let validity = mb.uid_validity.unwrap_or(0);
            let query = format!(
                "UID {}:* SINCE {}",
                from_uid.max(1),
                since.format("%d-%b-%Y")
            );
            let mut uids: Vec<u32> = session
                .uid_search(&query)
                .await
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter(|u| *u >= from_uid)
                .collect();
            uids.sort_unstable();
            uids.truncate(POLL_BATCH);
            let mut out = Vec::new();
            if !uids.is_empty() {
                let join = |u: &[u32]| u.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
                // Sizes first: nothing large is downloaded.
                let mut sizes: Vec<(u32, u64)> = session
                    .uid_fetch(join(&uids), "(UID RFC822.SIZE)")
                    .await
                    .map_err(|e| e.to_string())?
                    .try_collect::<Vec<_>>()
                    .await
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .filter_map(|f| Some((f.uid?, u64::from(f.size.unwrap_or(0)))))
                    .collect();
                sizes.sort_unstable();
                let max = crate::storage::MAX_ATTACHMENT_BYTES as u64;
                let mut budget = POLL_BYTES;
                let mut wanted = Vec::new();
                for (uid, size) in sizes {
                    let fits = size > 0 && size <= max;
                    if fits && size > budget {
                        break; // the rest next time, in order
                    }
                    if fits {
                        budget -= size;
                        wanted.push(uid);
                    }
                    out.push(Fetched {
                        uid,
                        size,
                        raw: None,
                    });
                }
                if !wanted.is_empty() {
                    let bodies: Vec<_> = session
                        .uid_fetch(join(&wanted), "(UID BODY.PEEK[])")
                        .await
                        .map_err(|e| e.to_string())?
                        .try_collect()
                        .await
                        .map_err(|e| e.to_string())?;
                    for f in bodies {
                        if let (Some(uid), Some(body)) = (f.uid, f.body())
                            && let Some(m) = out.iter_mut().find(|m| m.uid == uid)
                            && body.len() as u64 <= max
                        {
                            m.raw = Some(body.to_vec());
                        }
                    }
                }
            }
            let _ = session.logout().await;
            Ok((validity, out))
        })
    }
}

/// Organisations whose data the background jobs look at (customers; few).
pub(crate) async fn customer_orgs(state: &AppState) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut tx = state.pool.begin().await?;
    db::enable_platform_scope(&mut tx).await?;
    let ids = sqlx::query_scalar("SELECT id FROM organisations WHERE kind = 'customer'")
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(ids)
}

#[derive(sqlx::FromRow)]
struct Queued {
    id: Uuid,
    to_address: Option<String>,
    channel: String,
    subject: String,
    body: String,
    attempts: i32,
    request_id: Uuid,
    rfq_id: Uuid,
}

/// How long a claimed message is left alone by other senders while it is being sent.
const LEASE_SECONDS: f64 = 300.0;

/// Sends what is queued, for every organisation. Returns how many were sent. Each message is
/// claimed (leased) and committed before it is sent, and marked in its own transaction, so a
/// failure after sending never puts a sent message back in the queue.
pub async fn send_queued(state: &AppState, m: &SupplierMail) -> Result<usize, sqlx::Error> {
    let mut sent = 0;
    for org in customer_orgs(state).await? {
        let claimed = {
            let mut tx = state.pool.begin().await?;
            db::set_org(&mut tx, org).await?;
            let rows = sqlx::query_as::<_, Queued>(
                "WITH c AS (
                     SELECT o.id FROM outbox o
                     WHERE o.status = 'queued' AND o.next_attempt_at <= now()
                     ORDER BY o.next_attempt_at LIMIT $1 FOR UPDATE SKIP LOCKED)
                 UPDATE outbox o SET next_attempt_at = now() + make_interval(secs => $2)
                 FROM c, rfq_recipients r, rfqs f
                 WHERE o.id = c.id AND r.id = o.rfq_recipient_id AND f.id = r.rfq_id
                 RETURNING o.id, o.to_address, o.channel, o.subject, o.body, o.attempts,
                           f.request_id, f.id AS rfq_id",
            )
            .bind(SEND_BATCH)
            .bind(LEASE_SECONDS)
            .fetch_all(&mut *tx)
            .await?;
            tx.commit().await?;
            rows
        };
        for q in claimed {
            let to = q.to_address.clone().unwrap_or_default();
            let result = match format_message(m, &to, &q.subject, &q.body, q.channel == "fax") {
                Ok((message_id, raw)) => m
                    .transport
                    .send(&m.from, &to, &raw)
                    .await
                    .map(|()| message_id),
                Err(e) => Err(e),
            };
            let mut tx = state.pool.begin().await?;
            db::set_org(&mut tx, org).await?;
            match result {
                Ok(message_id) => {
                    sqlx::query(
                        "UPDATE outbox SET status = 'sent', sent_at = now(), message_id = $2,
                                attempts = attempts + 1, last_error = NULL
                         WHERE id = $1",
                    )
                    .bind(q.id)
                    .bind(&message_id)
                    .execute(&mut *tx)
                    .await?;
                    crate::audit::record(
                        &mut tx,
                        crate::audit::Event {
                            org_id: org,
                            actor: None,
                            action: "outbox.sent",
                            entity_type: "outbox",
                            entity_id: Some(q.id.to_string()),
                            data: serde_json::json!({ "channel": q.channel, "to": to }),
                            request_id: None,
                        },
                    )
                    .await?;
                    sent += 1;
                }
                Err(e) => {
                    let attempts = q.attempts + 1;
                    let failed = attempts >= MAX_ATTEMPTS;
                    // 1, 2, 4, 8, 16 minutes.
                    let wait = 60i64 << (attempts - 1).clamp(0, 4);
                    let error: String = e.chars().take(1000).collect();
                    tracing::warn!(outbox = %q.id, attempts, error = %error, "supplier mail not sent");
                    sqlx::query(
                        "UPDATE outbox SET attempts = $2, last_error = $3,
                                status = CASE WHEN $4 THEN 'failed' ELSE 'queued' END,
                                next_attempt_at = now() + make_interval(secs => $5)
                         WHERE id = $1",
                    )
                    .bind(q.id)
                    .bind(attempts)
                    .bind(&error)
                    .bind(failed)
                    .bind(wait as f64)
                    .execute(&mut *tx)
                    .await?;
                    if failed {
                        // A person decides what to do (wrong address, gateway down, …).
                        crate::api::ops::open_auto_task(
                            &mut tx,
                            org,
                            q.request_id,
                            Some(q.rfq_id),
                            "exception",
                            "A message to a supplier could not be sent: check the address",
                            Utc::now(),
                        )
                        .await
                        .map_err(|e| sqlx::Error::Protocol(format!("{e:?}")))?;
                    }
                }
            }
            tx.commit().await?;
        }
    }
    Ok(sent)
}

#[derive(sqlx::FromRow, Clone)]
struct Matched {
    recipient_id: Uuid,
    supplier_id: Uuid,
    supplier_name: String,
    supplier_email: Option<String>,
    rfq_id: Uuid,
    rfq_number: String,
    request_id: Uuid,
}

/// The RFQ recipient a reply belongs to. The token in the subject decides; the thread
/// headers are used only without a token, and a token and headers pointing at different
/// recipients (or organisations) match nothing, so a person looks at it.
async fn find_recipient(
    state: &AppState,
    token: Option<&str>,
    in_reply_to: &[String],
) -> Result<Option<(Uuid, Matched)>, sqlx::Error> {
    if token.is_none() && in_reply_to.is_empty() {
        return Ok(None);
    }
    let mut by_token = None;
    let mut by_thread = None;
    for org in customer_orgs(state).await? {
        let mut tx = state.pool.begin().await?;
        db::set_org(&mut tx, org).await?;
        db::enable_staff_scope(&mut tx).await?;
        let q = |cond: &str| {
            format!(
                "SELECT r.id AS recipient_id, r.supplier_id, s.name AS supplier_name,
                        s.email::text AS supplier_email, f.id AS rfq_id, f.number AS rfq_number,
                        f.request_id
                 FROM outbox o
                 JOIN rfq_recipients r ON r.id = o.rfq_recipient_id
                 JOIN rfqs f ON f.id = r.rfq_id
                 JOIN suppliers s ON s.id = r.supplier_id
                 WHERE {cond} LIMIT 1"
            )
        };
        if by_token.is_none()
            && let Some(t) = token
        {
            by_token = sqlx::query_as::<_, Matched>(&q("o.reply_token = $1"))
                .bind(t)
                .fetch_optional(&mut *tx)
                .await?
                .map(|h| (org, h));
        }
        if by_thread.is_none() && !in_reply_to.is_empty() {
            by_thread = sqlx::query_as::<_, Matched>(&q("o.message_id = ANY($1)"))
                .bind(in_reply_to)
                .fetch_optional(&mut *tx)
                .await?
                .map(|h| (org, h));
        }
        tx.commit().await?;
    }
    Ok(match (by_token, by_thread) {
        (Some(t), Some(h)) if (t.0, t.1.recipient_id) != (h.0, h.1.recipient_id) => None,
        (Some(t), _) => Some(t),
        (None, h) => h,
    })
}

fn domain(address: &str) -> Option<String> {
    address
        .rsplit_once('@')
        .map(|(_, d)| d.trim_end_matches('>').to_lowercase())
        .filter(|d| !d.is_empty())
}

/// A message to keep on an RFQ.
pub(crate) struct Incoming<'a> {
    pub org: Uuid,
    pub rfq_id: Uuid,
    pub request_id: Uuid,
    pub supplier: Option<(Uuid, &'a str)>,
    pub channel: Channel,
    pub bytes: &'a [u8],
    pub received_at: DateTime<Utc>,
}

/// Keeps `m.bytes` (a whole message) as the supplier's document on the RFQ and opens a task
/// to read it, in the caller's transaction (which must be set to `m.org`). Returns the
/// document id.
pub(crate) async fn assign_message(
    state: &AppState,
    tx: &mut db::Tx,
    m: &Incoming<'_>,
    p: Option<&crate::principal::Principal>,
) -> crate::error::ApiResult<Uuid> {
    let file_name = format!(
        "{}-{}.eml",
        match m.channel {
            Channel::Fax => "fax",
            _ => "reply",
        },
        m.received_at.format("%Y%m%d-%H%M%S")
    );
    let id = keep_document(
        state,
        tx,
        m.org,
        p,
        &KeptFile {
            rfq_id: m.rfq_id,
            supplier_id: m.supplier.map(|s| s.0),
            channel: m.channel,
            file_name: &file_name,
            bytes: m.bytes,
            received_at: Some(m.received_at),
        },
    )
    .await?;
    // A reply to an order email or follow-up is read by the chasing agent (when agents are on).
    crate::agents::chasing::on_reply_kept(tx, m.org, id, m.bytes).await?;
    let who = m
        .supplier
        .map_or_else(|| "a supplier".to_string(), |s| s.1.to_string());
    crate::api::ops::open_auto_task(
        tx,
        m.org,
        m.request_id,
        Some(m.rfq_id),
        "follow_up",
        &format!("Reply received from {who}: read it and record the quotation"),
        Utc::now() + chrono::Duration::hours(24),
    )
    .await?;
    Ok(id)
}

/// Reads new messages once. Returns how many were assigned to an RFQ automatically.
pub async fn poll_inbox(state: &AppState, m: &SupplierMail) -> Result<usize, String> {
    let (Some(inbox), Some(cfg)) = (&m.inbox, &m.imap) else {
        return Ok(0);
    };
    let db_err = |e: sqlx::Error| e.to_string();
    // Next UID per UIDVALIDITY we have seen; a changed UIDVALIDITY starts again from `since`.
    let last: Option<(i64, i64)> = {
        let mut tx = state.pool.begin().await.map_err(db_err)?;
        db::enable_platform_scope(&mut tx).await.map_err(db_err)?;
        let r = sqlx::query_as(
            "SELECT uid_validity, max(uid) FROM mail_inbox
             GROUP BY uid_validity ORDER BY max(created_at) DESC LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        r
    };
    let from_uid = last.map_or(1, |(_, u)| u32::try_from(u + 1).unwrap_or(u32::MAX));
    let (validity, messages) = inbox.fetch(from_uid, cfg.since).await?;
    let (validity, messages) = match last {
        Some((v, _)) if i64::from(validity) != v => inbox.fetch(1, cfg.since).await?,
        _ => (validity, messages),
    };
    let mut assigned = 0;
    for msg in messages {
        // The time we received it, not the sender's Date header (which anyone can write).
        let received_at = Utc::now();
        let Some(raw) = msg.raw else {
            // Not downloaded: recorded so it is not fetched again; staff see it in the mail
            // client.
            tracing::warn!(
                uid = msg.uid,
                size = msg.size,
                "mail not read: empty or larger than 10 MB"
            );
            let mut tx = state.pool.begin().await.map_err(db_err)?;
            db::enable_platform_scope(&mut tx).await.map_err(db_err)?;
            sqlx::query(
                "INSERT INTO mail_inbox (id, uid_validity, uid, channel, received_at, size_bytes,
                                         status)
                 VALUES ($1, $2, $3, 'email', $4, $5, 'skipped')
                 ON CONFLICT (uid_validity, uid) DO NOTHING",
            )
            .bind(Uuid::now_v7())
            .bind(i64::from(validity))
            .bind(i64::from(msg.uid))
            .bind(received_at)
            .bind(i64::try_from(msg.size).unwrap_or(i64::MAX))
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            continue;
        };
        let parsed = mail_parser::MessageParser::default().parse(&raw);
        let subject: String = parsed
            .as_ref()
            .and_then(|p| p.subject())
            .unwrap_or("")
            .chars()
            .take(500)
            .collect();
        let from: String = parsed
            .as_ref()
            .and_then(|p| p.from())
            .and_then(|a| a.first())
            .and_then(|a| a.address())
            .unwrap_or("")
            .chars()
            .take(320)
            .collect();
        let refs: Vec<String> = parsed
            .as_ref()
            .map(|p| {
                let mut v: Vec<String> = Vec::new();
                for h in [p.in_reply_to(), p.references()] {
                    if let Some(list) = h.as_text_list() {
                        v.extend(
                            list.iter()
                                .map(|s| format!("<{}>", s.trim_matches(['<', '>']))),
                        );
                    } else if let Some(s) = h.as_text() {
                        v.push(format!("<{}>", s.trim_matches(['<', '>'])));
                    }
                }
                v
            })
            .unwrap_or_default();
        let channel = if m.is_fax_sender(&from) {
            Channel::Fax
        } else {
            Channel::Email
        };
        let hash = state.storage.put(&raw).await.map_err(|e| e.to_string())?;
        let inbox_id = Uuid::now_v7();
        let inserted = {
            let mut tx = state.pool.begin().await.map_err(db_err)?;
            db::enable_platform_scope(&mut tx).await.map_err(db_err)?;
            let n = sqlx::query(
                "INSERT INTO mail_inbox (id, uid_validity, uid, channel, from_address, subject,
                                         received_at, size_bytes, sha256)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                 ON CONFLICT (uid_validity, uid) DO NOTHING",
            )
            .bind(inbox_id)
            .bind(i64::from(validity))
            .bind(i64::from(msg.uid))
            .bind(match channel {
                Channel::Fax => "fax",
                _ => "email",
            })
            .bind(&from)
            .bind(&subject)
            .bind(received_at)
            .bind(i64::try_from(raw.len()).unwrap_or(i64::MAX))
            .bind(hash.as_slice())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?
            .rows_affected();
            tx.commit().await.map_err(db_err)?;
            n == 1
        };
        if !inserted {
            continue;
        }
        let Some((org, hit)) = find_recipient(state, token_in(&subject).as_deref(), &refs)
            .await
            .map_err(db_err)?
        else {
            continue;
        };
        // Filed automatically only when it comes from the supplier's own mail domain;
        // anything else (a forward, a distributor, a FAX) waits for a person.
        let sender_ok = matches!(channel, Channel::Email)
            && domain(&from).is_some()
            && hit.supplier_email.as_deref().and_then(domain) == domain(&from);
        if !sender_ok {
            tracing::info!(rfq = %hit.rfq_number, "reply not from the supplier's domain; left for staff");
            continue;
        }
        let incoming = Incoming {
            org,
            rfq_id: hit.rfq_id,
            request_id: hit.request_id,
            supplier: Some((hit.supplier_id, &hit.supplier_name)),
            channel,
            bytes: &raw,
            received_at,
        };
        let result: Result<bool, crate::error::ApiError> = async {
            let mut tx = state.pool.begin().await?;
            db::set_org(&mut tx, org).await?;
            db::enable_platform_scope(&mut tx).await?;
            let document = assign_message(state, &mut tx, &incoming, None).await?;
            let n = sqlx::query(
                "UPDATE mail_inbox SET status = 'assigned', org_id = $2, document_id = $3,
                                       decided_at = now()
                 WHERE id = $1 AND status = 'unassigned'",
            )
            .bind(inbox_id)
            .bind(org)
            .bind(document)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if n == 1 {
                tx.commit().await?;
            }
            Ok(n == 1)
        }
        .await;
        match result {
            Ok(true) => {
                tracing::info!(rfq = %hit.rfq_number, "supplier reply kept");
                assigned += 1;
            }
            Ok(false) => {}
            Err(e) => tracing::warn!(error = ?e, "reply matched but not kept; left for staff"),
        }
    }
    Ok(assigned)
}

/// Starts the sender and the mailbox reader.
pub fn spawn(state: AppState, m: Arc<SupplierMail>) {
    let s = state.clone();
    let mail = m.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(20));
        loop {
            tick.tick().await;
            if let Err(e) = send_queued(&s, &mail).await {
                tracing::warn!(error = %e, "supplier mail sender");
            }
        }
    });
    if let Some(cfg) = &m.imap {
        let poll = cfg.poll;
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(poll);
            loop {
                tick.tick().await;
                if let Err(e) = poll_inbox(&state, &m).await {
                    tracing::warn!(error = %e, "supplier mailbox");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_found_in_replies_only_in_their_own_form() {
        let t = "0123456789ab";
        let s = subject("rfq", "RFQ-2026-000001", t);
        assert_eq!(token_in(&s).as_deref(), Some(t));
        assert_eq!(token_in(&format!("Re: Fwd: {s}")).as_deref(), Some(t));
        assert_eq!(token_in("Re: RFQ [#0123456789AB]"), None);
        assert_eq!(token_in("[#0123456789a]"), None);
        assert_eq!(token_in("[#0123456789abc]"), None);
        assert_eq!(reply_token().len(), 12);
    }
}
