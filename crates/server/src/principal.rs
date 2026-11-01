//! The authenticated caller and how it relates to a target organisation.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use chrono::{Duration, Utc};
use procure_domain::rbac::{self, Access, OrgKind, Permission, Role};
use uuid::Uuid;

use crate::AppState;
use crate::db;
use crate::error::ApiError;
use crate::security;

pub const SESSION_IDLE_TIMEOUT: Duration = Duration::hours(2);

#[derive(Debug, Clone)]
pub struct Membership {
    pub org_id: Uuid,
    pub org_kind: OrgKind,
    pub role: Role,
}

#[derive(Debug, Clone)]
pub struct Principal {
    pub user_id: Uuid,
    pub session_id: Uuid,
    pub memberships: Vec<Membership>,
    pub request_id: Option<String>,
}

impl Principal {
    pub fn platform_role(&self) -> Option<Role> {
        self.memberships
            .iter()
            .find(|m| m.org_kind == OrgKind::Platform)
            .map(|m| m.role)
    }

    /// Checks a permission in the platform org and returns that org's id.
    pub fn require_platform(&self, perm: Permission) -> Result<Uuid, ApiError> {
        let m = self
            .memberships
            .iter()
            .find(|m| m.org_kind == OrgKind::Platform)
            .ok_or(ApiError::Forbidden)?;
        if rbac::allows(Access::Member(m.role), perm) {
            Ok(m.org_id)
        } else {
            Err(ApiError::Forbidden)
        }
    }

    /// Resolves access to `org_id`. Unrelated orgs yield `NotFound`, never `Forbidden`.
    pub async fn access(&self, state: &AppState, org_id: Uuid) -> Result<Access, ApiError> {
        if let Some(m) = self.memberships.iter().find(|m| m.org_id == org_id) {
            return Ok(Access::Member(m.role));
        }
        let Some(role) = self.platform_role() else {
            return Ok(Access::None);
        };
        // Every internal role (admins included) reaches a customer org only through an
        // explicit, audited service assignment.
        let mut tx = db::user_tx(&state.pool, self.user_id).await?;
        let assigned: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM service_assignments
                            WHERE org_id = $1 AND user_id = $2 AND revoked_at IS NULL)",
        )
        .bind(org_id)
        .bind(self.user_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(if assigned {
            Access::Assigned(role)
        } else {
            Access::None
        })
    }

    /// Access + permission check in one step.
    pub async fn require(
        &self,
        state: &AppState,
        org_id: Uuid,
        perm: Permission,
    ) -> Result<Access, ApiError> {
        match self.access(state, org_id).await? {
            Access::None => Err(ApiError::NotFound),
            a if rbac::allows(a, perm) => Ok(a),
            _ => Err(ApiError::Forbidden),
        }
    }
}

pub fn bearer(parts: &Parts) -> Option<&str> {
    parts
        .headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|t| t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Peer address of the connection ("unknown" in in-process tests).
pub struct ClientIp(pub String);

/// Set at startup (`PROCURE_TRUST_LOCAL_PROXY=1`) when the server runs behind a reverse proxy
/// on the same machine (Tailscale serve, Caddy): only then is `X-Forwarded-For` believed,
/// and only for connections that come from this machine.
pub static TRUST_LOCAL_PROXY: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// The client address: the peer, or — behind a trusted local proxy — the address the proxy
/// reports (the last `X-Forwarded-For` entry, which the proxy itself appended).
pub fn client_ip(parts: &Parts) -> String {
    let Some(peer) = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
    else {
        return "unknown".to_string();
    };
    if peer.is_loopback() && TRUST_LOCAL_PROXY.load(std::sync::atomic::Ordering::Relaxed) {
        let forwarded = parts
            .headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .map(str::trim)
            .and_then(|v| v.parse::<std::net::IpAddr>().ok());
        if let Some(ip) = forwarded {
            return ip.to_string();
        }
    }
    peer.to_string()
}

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(ClientIp(client_ip(parts)))
    }
}

pub fn request_id(parts: &Parts) -> Option<String> {
    parts
        .headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(64).collect())
}

impl FromRequestParts<AppState> for Principal {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer(parts).ok_or(ApiError::Unauthenticated)?;
        authenticate(state, parts, token, "app", SESSION_IDLE_TIMEOUT).await
    }
}

/// Resolves a bearer token to a principal. A session only works for the app it was issued
/// to (`audience`): customer-app tokens never reach the admin API and vice versa.
pub async fn authenticate(
    state: &AppState,
    parts: &Parts,
    token: &str,
    audience: &str,
    idle_timeout: Duration,
) -> Result<Principal, ApiError> {
    let hash = security::token_hash(token);
    let now = Utc::now();

    let mut tx = db::auth_tx(&state.pool).await?;
    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        "UPDATE sessions s SET last_seen_at = now()
         FROM users u
         WHERE s.token_hash = $1 AND s.revoked_at IS NULL AND s.expires_at > $2
           AND s.last_seen_at > $3 AND s.audience = $4
           AND u.id = s.user_id AND u.status = 'active'
         RETURNING s.id, s.user_id",
    )
    .bind(&hash)
    .bind(now)
    .bind(now - idle_timeout)
    .bind(audience)
    .fetch_optional(&mut *tx)
    .await?;
    let (session_id, user_id) = row.ok_or(ApiError::Unauthenticated)?;
    let memberships = load_memberships(&mut tx, user_id).await?;
    tx.commit().await?;

    Ok(Principal {
        user_id,
        session_id,
        memberships,
        request_id: request_id(parts),
    })
}

pub async fn load_memberships(tx: &mut db::Tx, user_id: Uuid) -> Result<Vec<Membership>, ApiError> {
    db::set_user(tx, user_id).await?;
    let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT m.org_id, o.kind, m.role FROM memberships m JOIN organisations o ON o.id = m.org_id
         WHERE m.user_id = $1 AND m.status = 'active' AND o.status = 'active'
         ORDER BY o.kind DESC, o.name",
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(org_id, kind, role)| {
            Some(Membership {
                org_id,
                org_kind: OrgKind::from_code(&kind)?,
                role: Role::from_code(&role)?,
            })
        })
        .collect())
}
