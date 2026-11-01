//! Transaction helpers that set the row-level-security context.
//! Every database access in the server goes through one of these.

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub type Tx = Transaction<'static, Postgres>;

async fn set(tx: &mut Tx, key: &str, value: &str) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config($1, $2, true)")
        .bind(key)
        .bind(value)
        .execute(&mut **tx)
        .await
        .map(|_| ())
}

/// Authentication paths only: exact-key lookups of users, sessions and tokens.
pub async fn auth_tx(pool: &PgPool) -> Result<Tx, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set(&mut tx, "app.auth", "on").await?;
    Ok(tx)
}

/// Authenticated user with no organisation selected (own memberships only).
pub async fn user_tx(pool: &PgPool, user_id: Uuid) -> Result<Tx, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set(&mut tx, "app.user_id", &user_id.to_string()).await?;
    Ok(tx)
}

/// Authenticated user acting on one organisation. RLS limits every tenant table to `org_id`.
pub async fn tenant_tx(pool: &PgPool, user_id: Uuid, org_id: Uuid) -> Result<Tx, sqlx::Error> {
    let mut tx = user_tx(pool, user_id).await?;
    set_org(&mut tx, org_id).await?;
    Ok(tx)
}

/// Switch the organisation context inside a transaction (e.g. writing an audit
/// event into the platform org and into a newly created customer org).
pub async fn set_org(tx: &mut Tx, org_id: Uuid) -> Result<(), sqlx::Error> {
    set(tx, "app.org_id", &org_id.to_string()).await
}

/// Make the authenticated user's own rows (memberships, sessions) visible.
pub async fn set_user(tx: &mut Tx, user_id: Uuid) -> Result<(), sqlx::Error> {
    set(tx, "app.user_id", &user_id.to_string()).await
}

/// Temporarily allow exact-key user lookups (e.g. find a user by email while inviting).
/// Callers must switch it off again with `auth_lookup(tx, false)`.
pub async fn auth_lookup(tx: &mut Tx, on: bool) -> Result<(), sqlx::Error> {
    set(tx, "app.auth", if on { "on" } else { "" }).await
}

/// Only after the caller's platform permission has been verified.
pub async fn enable_platform_scope(tx: &mut Tx) -> Result<(), sqlx::Error> {
    set(tx, "app.scope", "platform").await
}

/// Only after verifying the caller holds a platform-org role: makes the platform-level
/// supplier directory visible and writable in this transaction.
pub async fn enable_staff_scope(tx: &mut Tx) -> Result<(), sqlx::Error> {
    set(tx, "app.staff", "on").await
}
