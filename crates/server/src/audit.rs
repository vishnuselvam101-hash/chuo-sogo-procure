//! Append-only, per-organisation hash-chained audit log.
//! The hash is computed inside PostgreSQL so writing and verifying use one definition.

use serde_json::Value;
use uuid::Uuid;

use crate::db::{self, Tx};

pub struct Event<'a> {
    pub org_id: Uuid,
    pub actor: Option<Uuid>,
    pub action: &'a str,
    pub entity_type: &'a str,
    pub entity_id: Option<String>,
    pub data: Value,
    pub request_id: Option<&'a str>,
}

const HASH_EXPR: &str = "sha256(prev_hash || convert_to(concat_ws('|', org_id::text, seq::text, \
     coalesce(actor_user_id::text, ''), action, entity_type, coalesce(entity_id, ''), data::text, \
     coalesce(request_id, ''), (extract(epoch from created_at) * 1000000)::bigint::text), 'UTF8'))";

/// Records one event in the same transaction as the change it describes.
/// Switches the transaction's org context to `event.org_id`; callers that keep working
/// on another org must call `db::set_org` again.
pub async fn record(tx: &mut Tx, event: Event<'_>) -> Result<(), sqlx::Error> {
    db::set_org(tx, event.org_id).await?;
    // Serialise writers per org so seq/prev_hash form a single chain.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 7))")
        .bind(event.org_id)
        .execute(&mut **tx)
        .await?;
    let sql = format!(
        "WITH last AS (
             SELECT seq, hash FROM audit_events WHERE org_id = $1 ORDER BY seq DESC LIMIT 1
         ), row AS (
             SELECT $1::uuid AS org_id,
                    coalesce((SELECT seq FROM last), 0) + 1 AS seq,
                    $2::uuid AS actor_user_id, $3::text AS action, $4::text AS entity_type,
                    $5::text AS entity_id, $6::jsonb AS data, $7::text AS request_id,
                    date_trunc('microseconds', clock_timestamp()) AS created_at,
                    coalesce((SELECT hash FROM last), '\\x{zero}'::bytea) AS prev_hash
         )
         INSERT INTO audit_events (org_id, seq, actor_user_id, action, entity_type, entity_id, data,
                                   request_id, created_at, prev_hash, hash)
         SELECT org_id, seq, actor_user_id, action, entity_type, entity_id, data, request_id,
                created_at, prev_hash, {HASH_EXPR} FROM row",
        zero = "0".repeat(64),
    );
    sqlx::query(&sql)
        .bind(event.org_id)
        .bind(event.actor)
        .bind(event.action)
        .bind(event.entity_type)
        .bind(event.entity_id)
        .bind(event.data)
        .bind(event.request_id)
        .execute(&mut **tx)
        .await
        .map(|_| ())
}

/// Recomputes the chain for the org in context. Returns the first broken seq, if any.
pub async fn verify_chain(tx: &mut Tx, org_id: Uuid) -> Result<Option<i64>, sqlx::Error> {
    let sql = format!(
        "SELECT seq FROM (
             SELECT seq, hash, {HASH_EXPR} AS expected,
                    prev_hash,
                    lag(hash) OVER (ORDER BY seq) AS prior,
                    lag(seq) OVER (ORDER BY seq) AS prior_seq
             FROM audit_events WHERE org_id = $1
         ) c
         WHERE hash <> expected
            OR (prior IS NOT NULL AND prev_hash <> prior)
            OR (prior_seq IS NOT NULL AND seq <> prior_seq + 1)
            OR (prior_seq IS NULL AND seq <> 1)
         ORDER BY seq LIMIT 1"
    );
    sqlx::query_scalar(&sql)
        .bind(org_id)
        .fetch_optional(&mut **tx)
        .await
}
