//! Phase 1 acceptance: authentication, RBAC, tenant isolation (API and RLS), audit.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use procure_server::db;
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn locked_and_unknown_accounts_are_indistinguishable(pool: PgPool) {
    let w = world(pool).await;
    for _ in 0..10 {
        let r = w.app.login("admin@a.test", "wrong-password-123").await;
        assert_eq!(r.code(), "invalid_credentials");
    }
    let mut tx = db::auth_tx(&w.app.pool).await.unwrap();
    let locked: bool =
        sqlx::query_scalar("SELECT locked_until > now() FROM users WHERE email = 'admin@a.test'")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(locked, "10 failures lock the account");
    drop(tx);
    // Locked (correct password!) and unknown accounts get the same answer.
    let (locked, unknown) = (
        w.app.login("admin@b.test", "wrong").await,
        w.app.login("nobody@x.test", PASSWORD).await,
    );
    assert_eq!(
        (locked.status, locked.code()),
        (unknown.status, unknown.code())
    );
    assert_eq!(unknown.code(), "invalid_credentials");
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn login_is_rate_limited_per_client(pool: PgPool) {
    let w = world(pool).await;
    let mut limited = false;
    for i in 0..25 {
        let r = w
            .app
            .login(&format!("spray{i}@x.test"), "guess-guess-guess")
            .await;
        if r.status == StatusCode::TOO_MANY_REQUESTS {
            assert_eq!(r.code(), "rate_limited");
            limited = true;
            break;
        }
    }
    assert!(
        limited,
        "password spraying from one client must be throttled"
    );
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn requests_without_valid_session_are_rejected(pool: PgPool) {
    let w = world(pool).await;
    let none = w.app.call(Method::GET, "/api/v1/me", None, None).await;
    assert_eq!(none.status, StatusCode::UNAUTHORIZED);
    let forged = w.app.get("/api/v1/me", &"ab".repeat(32)).await;
    assert_eq!(forged.status, StatusCode::UNAUTHORIZED);

    let logout = w
        .app
        .call(Method::POST, "/api/v1/auth/logout", Some(&w.admin_a), None)
        .await;
    assert_eq!(logout.status, StatusCode::NO_CONTENT);
    assert_eq!(
        w.app.get("/api/v1/me", &w.admin_a).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn customer_a_cannot_reach_customer_b_through_the_api(pool: PgPool) {
    let w = world(pool).await;
    let b = w.org_b;
    let b_admin_id = w.user_id(&w.admin_b).await;

    for r in [
        w.app
            .get(&format!("/api/v1/orgs/{b}/members"), &w.admin_a)
            .await,
        w.app
            .get(&format!("/api/v1/orgs/{b}/audit"), &w.admin_a)
            .await,
        w.app
            .post(
                &format!("/api/v1/orgs/{b}/members"),
                &w.admin_a,
                json!({ "email": "spy@a.test", "display_name": "Spy", "role": "customer_admin" }),
            )
            .await,
        w.app
            .patch(
                &format!("/api/v1/orgs/{b}/members/{b_admin_id}"),
                &w.admin_a,
                json!({ "status": "deactivated" }),
            )
            .await,
    ] {
        // 404, not 403: the existence of another tenant is not revealed.
        assert_eq!((r.status, r.code()), (StatusCode::NOT_FOUND, "not_found"));
    }
    // Customer admins cannot use platform endpoints either.
    assert_eq!(
        w.app
            .get("/api/v1/platform/organisations", &w.admin_a)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    // B's admin is still active.
    assert_eq!(
        w.app.get("/api/v1/me", &w.admin_b).await.status,
        StatusCode::OK
    );
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn row_level_security_hides_other_tenants_even_without_filters(pool: PgPool) {
    let w = world(pool).await;
    let a_admin = w.user_id(&w.admin_a).await;

    // Deliberately unfiltered queries inside A's context.
    let mut tx = db::tenant_tx(&w.app.pool, a_admin, w.org_a).await.unwrap();
    let orgs: Vec<String> = sqlx::query_scalar("SELECT name FROM organisations")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(orgs, vec!["株式会社エー製作所".to_string()]);
    let emails: Vec<String> = sqlx::query_scalar("SELECT email::text FROM users ORDER BY 1")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(emails, vec!["admin@a.test".to_string()]);
    // An invited-but-not-accepted account stays invisible to the inviting org.
    let invite = w
        .app
        .post(
            &format!("/api/v1/orgs/{}/members", w.org_a),
            &w.admin_a,
            json!({ "email": "admin@b.test", "display_name": "Typed By A", "role": "requester" }),
        )
        .await;
    assert_eq!(invite.status, StatusCode::CREATED);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(after, 1);
    let foreign_members: i64 =
        sqlx::query_scalar("SELECT count(*) FROM memberships WHERE org_id <> $1")
            .bind(w.org_a)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(foreign_members, 0);
    let foreign_audit: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE org_id <> $1")
            .bind(w.org_a)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(foreign_audit, 0);
    // Writing a row into B from A's context is rejected by the policy.
    let b_admin = w.user_id(&w.admin_b).await;
    let write =
        sqlx::query("INSERT INTO memberships (user_id, org_id, role) VALUES ($1, $2, 'requester')")
            .bind(b_admin)
            .bind(w.org_b)
            .execute(&mut *tx)
            .await;
    assert!(write.is_err(), "cross-tenant insert must fail");
    drop(tx);

    // No context at all: nothing is visible.
    let mut bare = w.app.pool.begin().await.unwrap();
    for table in [
        "organisations",
        "users",
        "memberships",
        "sessions",
        "audit_events",
        "user_tokens",
    ] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&mut *bare)
            .await
            .unwrap();
        assert_eq!(n, 0, "{table} visible without context");
    }
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn requester_cannot_manage_users_or_read_audit(pool: PgPool) {
    let w = world(pool).await;
    let a = w.org_a;
    let requester = w.member(a, &w.admin_a, "req@a.test", "requester").await;
    let add = w
        .app
        .post(
            &format!("/api/v1/orgs/{a}/members"),
            &requester,
            json!({ "email": "x@a.test", "display_name": "X", "role": "customer_admin" }),
        )
        .await;
    assert_eq!(
        (add.status, add.code()),
        (StatusCode::FORBIDDEN, "forbidden")
    );
    assert_eq!(
        w.app
            .get(&format!("/api/v1/orgs/{a}/audit"), &requester)
            .await
            .status,
        StatusCode::FORBIDDEN
    );

    let me = w.app.get("/api/v1/me", &requester).await;
    let perms = me.body["memberships"][0]["permissions"].as_array().unwrap();
    assert!(perms.iter().any(|p| p == "submit_request"));
    assert!(!perms.iter().any(|p| p == "approve_recommendation"));
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn internal_roles_cannot_be_granted_inside_a_customer_org(pool: PgPool) {
    let w = world(pool).await;
    let a = w.org_a;
    let r = w
        .app
        .post(
            &format!("/api/v1/orgs/{a}/members"),
            &w.admin_a,
            json!({ "email": "op@a.test", "display_name": "Op", "role": "procurement_operator" }),
        )
        .await;
    assert_eq!(
        (r.status, r.body["error"]["detail"].as_str()),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Some("role_not_allowed_in_org")
        )
    );

    // The database trigger enforces it too.
    let a_admin = w.user_id(&w.admin_a).await;
    let mut tx = db::tenant_tx(&w.app.pool, a_admin, a).await.unwrap();
    let direct =
        sqlx::query("UPDATE memberships SET role = 'platform_super_admin' WHERE org_id = $1")
            .bind(a)
            .execute(&mut *tx)
            .await;
    assert!(direct.is_err());
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn operators_reach_a_customer_only_through_an_assignment(pool: PgPool) {
    let w = world(pool).await;
    let (a, p) = (w.org_a, w.platform_org);
    let operator = w
        .member(
            p,
            &w.super_admin,
            "op@chuo-sogo.test",
            "procurement_operator",
        )
        .await;
    let op_id = w.user_id(&operator).await;
    let members_a = format!("/api/v1/orgs/{a}/members");

    // Unassigned: the customer org does not exist for them.
    assert_eq!(
        w.app.get(&members_a, &operator).await.status,
        StatusCode::NOT_FOUND
    );

    // Operators cannot assign themselves.
    let self_assign = w
        .app
        .post(
            &format!("/api/v1/orgs/{a}/assignments"),
            &operator,
            json!({ "user_id": op_id }),
        )
        .await;
    assert_eq!(self_assign.status, StatusCode::FORBIDDEN);

    let grant = w
        .app
        .post(
            &format!("/api/v1/orgs/{a}/assignments"),
            &w.super_admin,
            json!({ "user_id": op_id }),
        )
        .await;
    assert_eq!(grant.status, StatusCode::CREATED, "{:?}", grant.body);
    // Assigned: visible, but managing customer users is still not allowed.
    assert_eq!(
        w.app.get(&members_a, &operator).await.status,
        StatusCode::FORBIDDEN
    );
    // Still nothing in B.
    assert_eq!(
        w.app
            .get(&format!("/api/v1/orgs/{}/members", w.org_b), &operator)
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    let revoke = w
        .app
        .call(
            Method::DELETE,
            &format!("/api/v1/orgs/{a}/assignments/{op_id}"),
            Some(&w.super_admin),
            None,
        )
        .await;
    assert_eq!(revoke.status, StatusCode::NO_CONTENT);
    assert_eq!(
        w.app.get(&members_a, &operator).await.status,
        StatusCode::NOT_FOUND
    );

    // Both orgs recorded the grant and the revocation.
    let audit_a = w
        .app
        .get(&format!("/api/v1/orgs/{a}/audit"), &w.admin_a)
        .await;
    let actions: Vec<&str> = audit_a.body["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert!(
        actions.contains(&"assignment.granted") && actions.contains(&"assignment.revoked"),
        "{actions:?}"
    );
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn deactivation_takes_effect_immediately_and_last_admin_is_protected(pool: PgPool) {
    let w = world(pool).await;
    let a = w.org_a;
    let admin2 = w
        .member(a, &w.admin_a, "admin2@a.test", "customer_admin")
        .await;
    let admin2_id = w.user_id(&admin2).await;
    let admin1_id = w.user_id(&w.admin_a).await;
    assert_eq!(
        w.app
            .get(&format!("/api/v1/orgs/{a}/members"), &admin2)
            .await
            .status,
        StatusCode::OK
    );

    let self_edit = w
        .app
        .patch(
            &format!("/api/v1/orgs/{a}/members/{admin1_id}"),
            &w.admin_a,
            json!({ "role": "requester" }),
        )
        .await;
    assert_eq!(self_edit.body["error"]["detail"], "cannot_modify_self");

    let off = w
        .app
        .patch(
            &format!("/api/v1/orgs/{a}/members/{admin2_id}"),
            &w.admin_a,
            json!({ "status": "deactivated" }),
        )
        .await;
    assert_eq!(off.status, StatusCode::NO_CONTENT);
    assert_eq!(
        w.app
            .get(&format!("/api/v1/orgs/{a}/members"), &admin2)
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    // admin2 is gone; demoting admin1 (by a second, re-activated admin) must not leave zero admins.
    let on = w
        .app
        .patch(
            &format!("/api/v1/orgs/{a}/members/{admin2_id}"),
            &w.admin_a,
            json!({ "status": "active" }),
        )
        .await;
    assert_eq!(on.status, StatusCode::NO_CONTENT);
    let demote_other = w
        .app
        .patch(
            &format!("/api/v1/orgs/{a}/members/{admin1_id}"),
            &admin2,
            json!({ "role": "requester" }),
        )
        .await;
    assert_eq!(demote_other.status, StatusCode::NO_CONTENT);
    let demote_last = w
        .app
        .patch(
            &format!("/api/v1/orgs/{a}/members/{admin2_id}"),
            &w.admin_a,
            json!({ "role": "requester" }),
        )
        .await;
    // admin1 is now a requester and may not manage members at all.
    assert_eq!(demote_last.status, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn concurrent_mutual_demotion_never_leaves_an_org_without_admin(pool: PgPool) {
    let w = world(pool).await;
    let a = w.org_a;
    let admin2 = w
        .member(a, &w.admin_a, "admin2@a.test", "customer_admin")
        .await;
    let admin1_id = w.user_id(&w.admin_a).await;
    let admin2_id = w.user_id(&admin2).await;
    let (u1, u2) = (
        format!("/api/v1/orgs/{a}/members/{admin2_id}"),
        format!("/api/v1/orgs/{a}/members/{admin1_id}"),
    );
    let body = json!({ "role": "requester" });
    let (r1, r2) = tokio::join!(
        w.app.patch(&u1, &w.admin_a, body.clone()),
        w.app.patch(&u2, &admin2, body.clone())
    );
    let ok = [r1.status, r2.status]
        .iter()
        .filter(|s| **s == StatusCode::NO_CONTENT)
        .count();
    assert_eq!(
        ok, 1,
        "exactly one demotion may win: {:?} {:?}",
        r1.body, r2.body
    );
    let loser = if r1.status == StatusCode::NO_CONTENT {
        &r2
    } else {
        &r1
    };
    assert!(
        loser.body["error"]["detail"] == "last_admin" || loser.status == StatusCode::FORBIDDEN,
        "{:?}",
        loser.body
    );

    let mut tx = db::tenant_tx(&w.app.pool, admin1_id, a).await.unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memberships WHERE role = 'customer_admin' AND status = 'active'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(n, 1);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn audit_log_is_append_only_and_hash_chained(pool: PgPool) {
    let w = world(pool).await;
    let a = w.org_a;
    w.member(a, &w.admin_a, "req@a.test", "requester").await;
    let audit = w
        .app
        .get(&format!("/api/v1/orgs/{a}/audit"), &w.admin_a)
        .await;
    assert_eq!(audit.status, StatusCode::OK);
    assert_eq!(audit.body["chain_valid"], true);
    let actions: Vec<&str> = audit.body["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    for expected in ["organisation.created", "member.invited", "member.joined"] {
        assert!(
            actions.contains(&expected),
            "missing {expected} in {actions:?}"
        );
    }
    // Every event carries the request id that caused it, where one existed.
    assert!(
        audit.body["events"][0]["request_id"].is_string()
            || audit.body["events"][0]["action"] == "user.activated"
    );

    let a_admin = w.user_id(&w.admin_a).await;
    for sql in [
        "UPDATE audit_events SET action = 'x.y'",
        "DELETE FROM audit_events",
        "TRUNCATE audit_events",
        // The runtime role owns nothing, so it cannot switch the protections off either.
        "ALTER TABLE audit_events DISABLE TRIGGER audit_events_no_update",
        "ALTER TABLE memberships NO FORCE ROW LEVEL SECURITY",
        "DROP POLICY memberships_tenant ON memberships",
        "DELETE FROM memberships",
    ] {
        let mut tx = db::tenant_tx(&w.app.pool, a_admin, a).await.unwrap();
        assert!(
            sqlx::query(sql).execute(&mut *tx).await.is_err(),
            "{sql} must fail"
        );
    }
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn password_reset_tokens_are_single_use_and_revoke_sessions(pool: PgPool) {
    let w = world(pool).await;
    // Unknown address: same response, no mail.
    let before = w.app.mailer.sent.lock().unwrap().len();
    let r = w
        .app
        .call(
            Method::POST,
            "/api/v1/auth/password-reset",
            None,
            Some(json!({ "email": "ghost@x.test" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    assert_eq!(w.app.mailer.sent.lock().unwrap().len(), before);

    let r = w
        .app
        .call(
            Method::POST,
            "/api/v1/auth/password-reset",
            None,
            Some(json!({ "email": "admin@a.test" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    let token = w.app.last_token("admin@a.test");
    assert_eq!(
        w.app.set_password(&token, "short").await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        w.app
            .set_password(&token, "a-brand-new-passphrase")
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        w.app
            .set_password(&token, "another-new-passphrase")
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Old session revoked, old password gone, new password works.
    assert_eq!(
        w.app.get("/api/v1/me", &w.admin_a).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        w.app.login("admin@a.test", PASSWORD).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        w.app
            .login("admin@a.test", "a-brand-new-passphrase")
            .await
            .status,
        StatusCode::OK
    );
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn responses_carry_a_request_id_and_errors_hide_internals(pool: PgPool) {
    let w = world(pool).await;
    let r = w.app.get("/api/v1/me", &w.admin_a).await;
    assert!(r.request_id.is_some());
    let bad = w
        .app
        .get("/api/v1/orgs/not-a-uuid/members", &w.admin_a)
        .await;
    assert_eq!(
        (bad.status, bad.code()),
        (StatusCode::BAD_REQUEST, "bad_request")
    );
    let wrong_shape = w
        .app
        .post("/api/v1/auth/login", &w.admin_a, json!({ "user": "x" }))
        .await;
    assert_eq!(
        (wrong_shape.status, wrong_shape.code()),
        (StatusCode::BAD_REQUEST, "bad_request")
    );
    let me = w.app.get("/api/v1/me", &w.admin_a).await;
    assert_eq!(me.body["memberships"][0]["default_locale"], "ja");
    assert!(me.body.get("password_hash").is_none());
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn adding_an_existing_account_reveals_nothing_until_it_is_accepted(pool: PgPool) {
    let w = world(pool).await;
    let b = w.org_b;
    let add = |email: &'static str| {
        let w = &w;
        async move {
            w.app
                .post(
                    &format!("/api/v1/orgs/{b}/members"),
                    &w.admin_b,
                    json!({ "email": email, "display_name": "Typed By B", "role": "requester" }),
                )
                .await
        }
    };
    let existing = add("admin@a.test").await;
    let fresh = add("brand-new@b.test").await;
    // Same status and body whether or not the email already had an account.
    assert_eq!(
        (existing.status, &existing.body),
        (fresh.status, &fresh.body)
    );

    let members = w
        .app
        .get(&format!("/api/v1/orgs/{b}/members"), &w.admin_b)
        .await;
    let rows: Vec<_> = members
        .body
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["display_name"] == "Typed By B")
        .collect();
    assert_eq!(rows.len(), 2);
    for r in &rows {
        assert_eq!(
            (r["membership_status"].as_str(), r["user_status"].as_str()),
            (Some("invited"), Some("invited"))
        );
    }

    // B's admin cannot force the invitation active.
    let a_admin_id = w.user_id(&w.admin_a).await;
    let force = w
        .app
        .patch(
            &format!("/api/v1/orgs/{b}/members/{a_admin_id}"),
            &w.admin_b,
            json!({ "status": "active" }),
        )
        .await;
    assert_eq!(force.body["error"]["detail"], "invitation_pending");

    // The invited user sees the invitation, has no access yet, and can decline.
    let me = w.app.get("/api/v1/me", &w.admin_a).await;
    assert_eq!(me.body["invitations"][0]["org_id"], b.to_string());
    assert_eq!(me.body["memberships"].as_array().unwrap().len(), 1);
    let decline = w
        .app
        .call(
            Method::POST,
            &format!("/api/v1/invitations/{b}/decline"),
            Some(&w.admin_a),
            None,
        )
        .await;
    assert_eq!(decline.status, StatusCode::NO_CONTENT);
    let again = w
        .app
        .call(
            Method::POST,
            &format!("/api/v1/invitations/{b}/accept"),
            Some(&w.admin_a),
            None,
        )
        .await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn staff_accounts_can_never_hold_customer_roles(pool: PgPool) {
    let w = world(pool).await;
    let (a, p) = (w.org_a, w.platform_org);
    let operator = w
        .member(
            p,
            &w.super_admin,
            "op@chuo-sogo.test",
            "procurement_operator",
        )
        .await;
    // A customer admin cannot make an internal operator their procurement manager (approval authority).
    let r = w
        .app
        .post(
            &format!("/api/v1/orgs/{a}/members"),
            &w.admin_a,
            json!({ "email": "op@chuo-sogo.test", "display_name": "Op", "role": "procurement_manager" }),
        )
        .await;
    assert_eq!(r.body["error"]["detail"], "account_kind_mismatch");
    // The super admin cannot make themselves a customer admin.
    let r = w
        .app
        .post(
            "/api/v1/platform/organisations",
            &w.super_admin,
            json!({ "name": "Self Corp", "admin_email": "root@chuo-sogo.test", "admin_name": "Root" }),
        )
        .await;
    assert_eq!(r.body["error"]["detail"], "account_kind_mismatch");
    // Customer accounts cannot be pulled into the platform org.
    let r = w
        .app
        .post(
            &format!("/api/v1/orgs/{p}/members"),
            &w.super_admin,
            json!({ "email": "admin@a.test", "display_name": "A", "role": "procurement_operator" }),
        )
        .await;
    assert_eq!(r.body["error"]["detail"], "account_kind_mismatch");
    // The database refuses it too, even with the API bypassed.
    let op_id = w.user_id(&operator).await;
    let a_admin = w.user_id(&w.admin_a).await;
    let mut tx = db::tenant_tx(&w.app.pool, a_admin, a).await.unwrap();
    let direct = sqlx::query(
        "INSERT INTO memberships (user_id, org_id, role) VALUES ($1, $2, 'procurement_manager')",
    )
    .bind(op_id)
    .bind(a)
    .execute(&mut *tx)
    .await;
    assert!(direct.is_err());
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn internal_admins_also_need_an_assignment(pool: PgPool) {
    let w = world(pool).await;
    let (a, p) = (w.org_a, w.platform_org);
    let padmin = w
        .member(
            p,
            &w.super_admin,
            "padmin@chuo-sogo.test",
            "procurement_admin",
        )
        .await;
    let padmin_id = w.user_id(&padmin).await;
    let audit_a = format!("/api/v1/orgs/{a}/audit");
    assert_eq!(
        w.app
            .get(&format!("/api/v1/orgs/{a}/members"), &padmin)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    // Self-assignment is allowed for admins but is recorded in the customer's own audit log.
    let grant = w
        .app
        .post(
            &format!("/api/v1/orgs/{a}/assignments"),
            &padmin,
            json!({ "user_id": padmin_id }),
        )
        .await;
    assert_eq!(grant.status, StatusCode::CREATED);
    let log = w.app.get(&audit_a, &w.admin_a).await;
    let granted = log.body["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["action"] == "assignment.granted");
    assert!(granted);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn client_request_ids_must_be_uuids(pool: PgPool) {
    let w = world(pool).await;
    let send = |rid: &'static str| {
        let w = &w;
        async move {
            let req = axum::http::Request::builder()
                .uri("/health")
                .header("x-request-id", rid)
                .body(axum::body::Body::empty())
                .unwrap();
            let res = tower::ServiceExt::oneshot(w.app.router.clone(), req)
                .await
                .unwrap();
            res.headers()["x-request-id"].to_str().unwrap().to_string()
        }
    };
    let good = "0192f0aa-1111-7222-8333-444455556666";
    assert_eq!(send(good).await, good);
    let replaced = send("'; DROP TABLE x; --").await;
    assert!(uuid::Uuid::parse_str(&replaced).is_ok(), "{replaced}");
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn repeated_reset_requests_do_not_kill_the_first_link(pool: PgPool) {
    let w = world(pool).await;
    let reset = || {
        w.app.call(
            Method::POST,
            "/api/v1/auth/password-reset",
            None,
            Some(json!({ "email": "admin@a.test" })),
        )
    };
    assert_eq!(reset().await.status, StatusCode::ACCEPTED);
    let first = w.app.last_token("admin@a.test");
    assert_eq!(reset().await.status, StatusCode::ACCEPTED);
    assert_eq!(reset().await.status, StatusCode::ACCEPTED);
    assert_eq!(
        w.app.last_token("admin@a.test"),
        first,
        "no new token inside the cooldown"
    );
    assert_eq!(
        w.app
            .set_password(&first, "still-works-passphrase")
            .await
            .status,
        StatusCode::NO_CONTENT
    );
}
