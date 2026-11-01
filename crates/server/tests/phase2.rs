//! Phase 2 acceptance: request lifecycle, product verification, supplier directory and
//! customer decisions, RFQ test-mode dispatch, quotations, deterministic comparison,
//! recommendation and human approval under the customer's policy, all audited.

// Test-only scenario helpers: a failed unwrap is a failed test, which is the intent.
#![allow(clippy::unwrap_used)]

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

/// Org A with an assigned operator, a requester, two procurement managers and two
/// verified suppliers that A has approved.
struct Flow {
    w: World,
    operator: String,
    requester: String,
    manager1: String,
    manager2: String,
    s1: Uuid,
    s2: Uuid,
}

impl Flow {
    async fn new(pool: PgPool) -> Self {
        let w = world(pool).await;
        let operator = w
            .member(
                w.platform_org,
                &w.super_admin,
                "op@chuo-sogo.test",
                "procurement_operator",
            )
            .await;
        let op_id = w.user_id(&operator).await;
        let r = w
            .app
            .post(
                &format!("/api/v1/orgs/{}/assignments", w.org_a),
                &w.super_admin,
                json!({ "user_id": op_id }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let requester = w
            .member(w.org_a, &w.admin_a, "req@a.test", "requester")
            .await;
        let manager1 = w
            .member(w.org_a, &w.admin_a, "m1@a.test", "procurement_manager")
            .await;
        let manager2 = w
            .member(w.org_a, &w.admin_a, "m2@a.test", "procurement_manager")
            .await;
        let mut f = Flow {
            w,
            operator,
            requester,
            manager1,
            manager2,
            s1: Uuid::nil(),
            s2: Uuid::nil(),
        };
        f.s1 = f
            .supplier("Tokyo Bearings KK", "rfq@tokyo-bearings.test")
            .await;
        f.s2 = f
            .supplier("Osaka Motion Ltd", "sales@osaka-motion.test")
            .await;
        for s in [f.s1, f.s2] {
            f.ok(f
                .post_op(&format!("suppliers/{s}/proposal"), json!({}))
                .await);
            let r =
                f.w.app
                    .put(
                        &f.url(&format!("suppliers/{s}/decision")),
                        &f.manager1,
                        json!({ "decision": "approved" }),
                    )
                    .await;
            assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
        }
        f
    }

    fn url(&self, path: &str) -> String {
        format!("/api/v1/orgs/{}/{path}", self.w.org_a)
    }

    fn ok(&self, r: Resp) -> Value {
        assert!(r.status.is_success(), "{}: {:?}", r.status, r.body);
        r.body
    }

    async fn post_op(&self, path: &str, body: Value) -> Resp {
        self.w.app.post(&self.url(path), &self.operator, body).await
    }

    async fn supplier(&self, name: &str, email: &str) -> Uuid {
        let r = self
            .w
            .app
            .post(
                "/api/v1/suppliers",
                &self.operator,
                json!({ "name": name, "country": "jp", "email": email, "categories": ["bearings"],
                        "notes": "internal: met at trade fair" }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let id = uuid(&r.body["id"]);
        let r = self
            .w
            .app
            .post(
                &format!("/api/v1/suppliers/{id}/status"),
                &self.operator,
                json!({ "status": "verified" }),
            )
            .await;
        assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
        id
    }

    /// Creates and submits a request as `who`. Returns (request id, [item ids]).
    async fn submitted(&self, who: &str) -> (Uuid, Vec<Uuid>) {
        let r = self
            .w
            .app
            .post(
                &self.url("requests"),
                who,
                json!({
                    "title": "ベアリング交換部品",
                    "currency": "JPY",
                    "needed_by": "2026-12-15",
                    "items": [
                        { "description": "Deep groove ball bearing", "manufacturer": "NSK",
                          "mpn": "6204ZZ", "quantity": 100 },
                        { "description": "Linear guide", "mpn": "HSR15", "quantity": 4, "unit": "set" }
                    ]
                }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let id = uuid(&r.body["id"]);
        assert!(r.body["number"].as_str().unwrap().starts_with("PR-"));
        self.ok(self
            .w
            .app
            .post(&self.url(&format!("requests/{id}/submit")), who, json!({}))
            .await);
        let detail = self
            .w
            .app
            .get(&self.url(&format!("requests/{id}")), who)
            .await;
        let items = detail.body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| uuid(&i["id"]))
            .collect();
        (id, items)
    }

    async fn status(&self, request: Uuid) -> String {
        let r = self
            .w
            .app
            .get(&self.url(&format!("requests/{request}")), &self.operator)
            .await;
        r.body["request"]["status"].as_str().unwrap().to_string()
    }

    /// Operator identifies both items and starts sourcing.
    async fn sourcing(&self, request: Uuid, items: &[Uuid]) {
        self.ok(self
            .post_op(&format!("requests/{request}/pick-up"), json!({}))
            .await);
        for (i, item) in items.iter().enumerate() {
            self.ok(self
                .post_op(
                    &format!("requests/{request}/items/{item}/verification"),
                    json!({ "manufacturer": "NSK", "mpn": format!("VERIFIED-{i}"),
                            "evidence": "https://example.test/datasheet.pdf" }),
                )
                .await);
        }
        self.ok(self
            .post_op(&format!("requests/{request}/start-sourcing"), json!({}))
            .await);
    }

    async fn rfq(&self, request: Uuid, suppliers: &[Uuid]) -> Uuid {
        let r = self
            .post_op(
                &format!("requests/{request}/rfqs"),
                json!({ "supplier_ids": suppliers, "response_due": "2026-11-20",
                        "message": "Please quote." }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let rfq = uuid(&r.body["id"]);
        let d = self.ok(self
            .post_op(&format!("rfqs/{rfq}/dispatch"), json!({}))
            .await);
        assert_eq!(d["channel"], "test");
        rfq
    }

    async fn quote(&self, rfq: Uuid, supplier: Uuid, items: &[Uuid], prices: [i64; 2]) -> Uuid {
        let r = self
            .post_op(
                &format!("rfqs/{rfq}/quotations"),
                json!({
                    "supplier_id": supplier, "currency": "JPY", "shipping_minor": 2000,
                    "valid_until": "2099-01-01",
                    "lines": [
                        { "request_item_id": items[0], "unit_price_minor": prices[0], "quantity": 100,
                          "lead_time_days": 7 },
                        { "request_item_id": items[1], "unit_price_minor": prices[1], "quantity": 4,
                          "lead_time_days": 21 }
                    ]
                }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        uuid(&r.body["id"])
    }

    async fn compare(&self, rfq: Uuid) -> Value {
        let r = self
            .post_op(&format!("rfqs/{rfq}/comparisons"), json!({}))
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        r.body
    }

    /// Request `who` raised, taken to `under_comparison` with S1 cheaper than S2.
    /// Returns (request, rfq, comparison, quote s1, quote s2).
    async fn compared(&self, who: &str) -> (Uuid, Uuid, Uuid, Uuid, Uuid) {
        let (request, items) = self.submitted(who).await;
        self.sourcing(request, &items).await;
        let rfq = self.rfq(request, &[self.s1, self.s2]).await;
        let q1 = self.quote(rfq, self.s1, &items, [1200, 45_000]).await;
        let q2 = self.quote(rfq, self.s2, &items, [1300, 44_000]).await;
        let cmp = self.compare(rfq).await;
        (request, rfq, uuid(&cmp["id"]), q1, q2)
    }

    async fn recommend_and_send(&self, request: Uuid, cmp: Uuid, quote: Uuid) -> Uuid {
        let r = self
            .post_op(
                &format!("requests/{request}/recommendations"),
                json!({ "comparison_id": cmp, "quotation_id": quote }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let rec = uuid(&r.body["id"]);
        self.ok(self
            .post_op(&format!("recommendations/{rec}/send"), json!({}))
            .await);
        rec
    }

    async fn decide(&self, who: &str, rec: Uuid, decision: &str, comment: &str) -> Resp {
        self.w
            .app
            .post(
                &self.url(&format!("recommendations/{rec}/decisions")),
                who,
                json!({ "decision": decision, "comment": comment }),
            )
            .await
    }

    async fn audit_actions(&self) -> (Vec<String>, bool) {
        let r = self
            .w
            .app
            .get(&self.url("audit?limit=500"), &self.w.admin_a)
            .await;
        let actions = r.body["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["action"].as_str().unwrap().to_string())
            .collect();
        (actions, r.body["chain_valid"].as_bool().unwrap())
    }
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn full_flow_from_request_to_multi_step_approval(pool: PgPool) {
    let f = Flow::new(pool).await;
    // Two approvals from ¥100,000.
    f.ok(f
        .w
        .app
        .put(
            &f.url("approval-policies/JPY"),
            &f.w.admin_a,
            json!({ "tiers": [{ "min_minor": 100000, "approvals": 2 }] }),
        )
        .await);

    let (request, rfq, cmp, q1, _q2) = f.compared(&f.requester).await;
    assert_eq!(f.status(request).await, "under_comparison");

    // Deterministic totals: S1 = 1200*100 + 45000*4 + 2000 = 302000; S2 = 130000 + 176000 + 2000.
    let c =
        f.w.app
            .get(&f.url(&format!("comparisons/{cmp}")), &f.manager1)
            .await;
    assert_eq!(c.status, StatusCode::OK);
    let results = c.body["results"].as_array().unwrap();
    assert_eq!(results[0]["quotation_id"], json!(q1));
    assert_eq!(results[0]["total_minor"], 302_000);
    assert_eq!(results[0]["rank"], 1);
    assert_eq!(results[1]["total_minor"], 308_000);
    assert_eq!(results[0]["compliance"], "compliant");

    // Supplier messages are recorded, not sent, and never name the customer.
    let outbox =
        f.w.app
            .get(&f.url(&format!("rfqs/{rfq}/outbox")), &f.operator)
            .await;
    let msgs = outbox.body.as_array().unwrap();
    assert_eq!(msgs.len(), 2);
    for m in msgs {
        assert_eq!(m["channel"], "test");
        let body = m["body"].as_str().unwrap();
        assert!(
            body.contains("VERIFIED-0") && body.contains("qty 100 pcs"),
            "{body}"
        );
        assert!(
            !body.contains("エー製作所") && !body.contains("a.test"),
            "{body}"
        );
    }

    let rec = f.recommend_and_send(request, cmp, q1).await;
    assert_eq!(f.status(request).await, "awaiting_approval");
    let d =
        f.w.app
            .get(&f.url(&format!("recommendations/{rec}")), &f.manager1)
            .await;
    assert_eq!(d.body["recommendation"]["required_approvals"], 2);
    assert_eq!(d.body["remaining_approvals"], 2);
    // Customers never see internal supplier notes.
    assert!(!d.body.to_string().contains("trade fair"));

    let r = f.decide(&f.manager1, rec, "approve", "").await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["status"], "awaiting_approval");
    assert_eq!(r.body["remaining_approvals"], 1);
    // The same approver counts once.
    let r = f.decide(&f.manager1, rec, "approve", "").await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(f.status(request).await, "awaiting_approval");

    let r = f.decide(&f.manager2, rec, "approve", "OK").await;
    assert_eq!(r.body["status"], "approved", "{:?}", r.body);
    assert_eq!(f.status(request).await, "approved");

    // Policy changes after sending do not affect the decided recommendation.
    let d =
        f.w.app
            .get(&f.url(&format!("recommendations/{rec}")), &f.manager2)
            .await;
    assert_eq!(d.body["recommendation"]["status"], "approved");
    assert_eq!(d.body["decisions"].as_array().unwrap().len(), 2);

    let (actions, chain_valid) = f.audit_actions().await;
    assert!(chain_valid);
    for expected in [
        "request.created",
        "request.submitted",
        "request.picked_up",
        "item.verified",
        "request.sourcing_started",
        "supplier.proposed",
        "supplier.customer_decision",
        "rfq.created",
        "rfq.dispatched",
        "request.rfq_sent",
        "quotation.recorded",
        "request.quotes_received",
        "comparison.run",
        "recommendation.created",
        "recommendation.sent",
        "approval.recorded",
        "request.approved",
        "approval_policy.updated",
    ] {
        assert!(actions.iter().any(|a| a == expected), "missing {expected}");
    }
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn staff_never_approve_and_requesters_never_self_approve(pool: PgPool) {
    let f = Flow::new(pool).await;
    // A manager raises their own request.
    let (request, _, cmp, q1, _) = f.compared(&f.manager1).await;
    let rec = f.recommend_and_send(request, cmp, q1).await;

    // Operator (assigned staff) and platform super admin cannot decide.
    assert_eq!(
        f.decide(&f.operator, rec, "approve", "").await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.decide(&f.w.super_admin, rec, "approve", "").await.status,
        StatusCode::NOT_FOUND
    );
    // Requesters and customer admins lack the permission.
    assert_eq!(
        f.decide(&f.requester, rec, "approve", "").await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.decide(&f.w.admin_a, rec, "approve", "").await.status,
        StatusCode::FORBIDDEN
    );
    // The requesting manager cannot approve their own request by default.
    assert_eq!(
        f.decide(&f.manager1, rec, "approve", "").await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(f.status(request).await, "awaiting_approval");

    let r = f.decide(&f.manager2, rec, "approve", "").await;
    assert_eq!(r.body["status"], "approved", "{:?}", r.body);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn rejection_and_clarification_need_a_comment(pool: PgPool) {
    let f = Flow::new(pool).await;
    let (request, _, cmp, q1, _) = f.compared(&f.requester).await;
    let rec = f.recommend_and_send(request, cmp, q1).await;
    let r = f.decide(&f.manager1, rec, "reject", "  ").await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = f
        .decide(&f.manager1, rec, "clarify", "Is lead time negotiable?")
        .await;
    assert_eq!(r.body["status"], "clarification_requested", "{:?}", r.body);
    assert_eq!(f.status(request).await, "needs_clarification");

    // The requester must answer before resubmitting.
    let submit = f.url(&format!("requests/{request}/submit"));
    let r = f.w.app.post(&submit, &f.requester, json!({})).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r =
        f.w.app
            .post(
                &submit,
                &f.requester,
                json!({ "answer": "Yes, up to 30 days." }),
            )
            .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
    assert_eq!(f.status(request).await, "submitted");

    // Second round ends in a rejection.
    let (request, _, cmp, q1, _) = f.compared(&f.requester).await;
    let rec = f.recommend_and_send(request, cmp, q1).await;
    let r = f.decide(&f.manager2, rec, "reject", "Budget frozen").await;
    assert_eq!(r.body["status"], "rejected");
    assert_eq!(f.status(request).await, "rejected");
    // Nothing more can happen to a rejected recommendation.
    assert_eq!(
        f.decide(&f.manager1, rec, "approve", "").await.status,
        StatusCode::CONFLICT
    );
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn recommendation_guards(pool: PgPool) {
    let f = Flow::new(pool).await;
    let s3 = f.supplier("Nagoya Parts", "s3@nagoya.test").await;
    let (request, items) = f.submitted(&f.requester).await;
    f.sourcing(request, &items).await;
    let rfq = f.rfq(request, &[f.s1, f.s2, s3]).await;
    let q1 = f.quote(rfq, f.s1, &items, [1200, 45_000]).await;
    let q2 = f.quote(rfq, f.s2, &items, [1300, 44_000]).await;
    // Cheapest, but A has not approved this supplier.
    let q3 = f.quote(rfq, s3, &items, [1000, 40_000]).await;
    let cmp = uuid(&f.compare(rfq).await["id"]);
    let path = format!("requests/{request}/recommendations");
    let rec = |quote: Uuid, reason: &str| {
        f.post_op(
            &path,
            json!({ "comparison_id": cmp, "quotation_id": quote, "reason": reason }),
        )
    };
    let r = rec(q3, "").await;
    assert_eq!(
        (r.status, r.body["error"]["detail"].as_str()),
        (StatusCode::CONFLICT, Some("supplier_not_approved"))
    );
    // Not the top-ranked quotation: a reason is required.
    let r = rec(q2, "").await;
    assert_eq!(r.body["error"]["detail"], "reason_required");

    // A short quotation is non-compliant and cannot be recommended.
    let r = f
        .post_op(
            &format!("rfqs/{rfq}/quotations"),
            json!({ "supplier_id": f.s1, "currency": "JPY", "lines": [
                { "request_item_id": items[0], "unit_price_minor": 1, "quantity": 50 },
                { "request_item_id": items[1], "unit_price_minor": 1, "quantity": 4 } ] }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let short = uuid(&r.body["id"]);
    // The old comparison is now stale.
    let r = rec(q1, "").await;
    assert_eq!(r.body["error"]["detail"], "stale_comparison");
    let cmp2 = f.compare(rfq).await;
    let s1_result = cmp2["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["quotation_id"] == json!(short))
        .unwrap()
        .clone();
    assert_eq!(s1_result["compliance"], "non_compliant");
    assert_eq!(s1_result["reasons"], json!(["insufficient_quantity"]));
    // Only the current quotation per supplier is compared.
    assert_eq!(cmp2["results"].as_array().unwrap().len(), 3);
    let r = f
        .post_op(
            &format!("requests/{request}/recommendations"),
            json!({ "comparison_id": cmp2["id"], "quotation_id": short }),
        )
        .await;
    assert_eq!(r.body["error"]["detail"], "quotation_not_compliant");
    // With a reason, the second-ranked approved supplier can be recommended.
    let r = f
        .post_op(
            &format!("requests/{request}/recommendations"),
            json!({ "comparison_id": cmp2["id"], "quotation_id": q2, "reason": "Faster delivery" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(f.status(request).await, "recommended");
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn workflow_guards(pool: PgPool) {
    let f = Flow::new(pool).await;
    let app = &f.w.app;
    // Submitting an empty draft is refused.
    let r = app
        .post(
            &f.url("requests"),
            &f.requester,
            json!({ "title": "Empty", "currency": "JPY" }),
        )
        .await;
    let empty = uuid(&r.body["id"]);
    let r = app
        .post(
            &f.url(&format!("requests/{empty}/submit")),
            &f.requester,
            json!({}),
        )
        .await;
    assert_eq!(r.body["error"]["detail"], "no_items");
    // Invalid input.
    for bad in [
        json!({ "title": "x", "currency": "XYZ" }),
        json!({ "title": "  ", "currency": "JPY" }),
        json!({ "title": "x", "currency": "JPY", "items": [{ "description": "a", "quantity": 0 }] }),
        json!({ "title": "x", "currency": "JPY", "items": [{ "description": "a", "quantity": 1, "unit": "PCS" }] }),
    ] {
        let r = app
            .post(&f.url("requests"), &f.requester, bad.clone())
            .await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }

    let (request, items) = f.submitted(&f.requester).await;
    // Items are frozen once submitted.
    let r = app
        .post(
            &f.url(&format!("requests/{request}/items")),
            &f.requester,
            json!({ "description": "late", "quantity": 1 }),
        )
        .await;
    assert_eq!(r.body["error"]["detail"], "not_editable");
    // Verification only while identifying; sourcing only once everything is verified.
    let verify = format!("requests/{request}/items/{}/verification", items[0]);
    let body = json!({ "manufacturer": "NSK", "mpn": "6204ZZ", "evidence": "catalogue p.12" });
    assert_eq!(
        f.post_op(&verify, body.clone()).await.status,
        StatusCode::CONFLICT
    );
    f.ok(f
        .post_op(&format!("requests/{request}/pick-up"), json!({}))
        .await);
    // Customers cannot cancel once work has started.
    let r = app
        .post(
            &f.url(&format!("requests/{request}/cancel")),
            &f.requester,
            json!({}),
        )
        .await;
    assert_eq!(r.body["error"]["detail"], "invalid_transition");
    // Evidence is mandatory.
    let r = f
        .post_op(
            &verify,
            json!({ "manufacturer": "NSK", "mpn": "6204ZZ", "evidence": "" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    f.ok(f.post_op(&verify, body).await);
    let r = f
        .post_op(&format!("requests/{request}/start-sourcing"), json!({}))
        .await;
    assert_eq!(r.body["error"]["detail"], "items_not_verified");
    // No RFQ before sourcing; unverified suppliers are refused.
    let r = f
        .post_op(
            &format!("requests/{request}/rfqs"),
            json!({ "supplier_ids": [f.s1] }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);

    // Operators cannot act for the customer, customers cannot act for the operator.
    let r = app
        .post(
            &f.url(&format!("requests/{request}/start-sourcing")),
            &f.w.admin_a,
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = f
        .post_op("requests", json!({ "title": "x", "currency": "JPY" }))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // Draft cancel works for the owner only.
    let r = app
        .post(
            &f.url(&format!("requests/{empty}/cancel")),
            &f.manager1,
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = app
        .post(
            &f.url(&format!("requests/{empty}/cancel")),
            &f.requester,
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn draft_items_can_be_edited_and_reset_verification(pool: PgPool) {
    let f = Flow::new(pool).await;
    let app = &f.w.app;
    let r = app
        .post(
            &f.url("requests"),
            &f.requester,
            json!({ "title": "Draft", "currency": "USD",
                    "items": [{ "description": "Motor", "quantity": 2 }] }),
        )
        .await;
    let request = uuid(&r.body["id"]);
    let r = app
        .post(
            &f.url(&format!("requests/{request}/items")),
            &f.requester,
            json!({ "description": "Coupling", "quantity": 2, "unit": "box" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let added = uuid(&r.body["id"]);
    let r = app
        .put(
            &f.url(&format!("requests/{request}/items/{added}")),
            &f.requester,
            json!({ "description": "Flexible coupling", "quantity": 3 }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
    // Another customer user cannot edit someone else's draft.
    let r = app
        .delete(
            &f.url(&format!("requests/{request}/items/{added}")),
            &f.manager1,
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = app
        .delete(
            &f.url(&format!("requests/{request}/items/{added}")),
            &f.requester,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = app
        .patch(
            &f.url(&format!("requests/{request}")),
            &f.requester,
            json!({ "title": "Renamed", "currency": "JPY" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let d = app
        .get(&f.url(&format!("requests/{request}")), &f.requester)
        .await;
    assert_eq!(d.body["request"]["title"], "Renamed");
    assert_eq!(d.body["request"]["currency"], "JPY");
    assert_eq!(d.body["items"].as_array().unwrap().len(), 1);

    // Clarification from identifying: answering lets the requester edit; edits clear verification.
    app.post(
        &f.url(&format!("requests/{request}/submit")),
        &f.requester,
        json!({}),
    )
    .await;
    let item = uuid(&d.body["items"][0]["id"]);
    f.ok(f
        .post_op(&format!("requests/{request}/pick-up"), json!({}))
        .await);
    f.ok(f
        .post_op(
            &format!("requests/{request}/items/{item}/verification"),
            json!({ "mpn": "M-1", "manufacturer": "", "evidence": "label photo" }),
        )
        .await);
    f.ok(f
        .post_op(
            &format!("requests/{request}/clarification"),
            json!({ "question": "Voltage?" }),
        )
        .await);
    let r = app
        .put(
            &f.url(&format!("requests/{request}/items/{item}")),
            &f.requester,
            json!({ "description": "Motor 200V", "quantity": 2 }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let d = app
        .get(&f.url(&format!("requests/{request}")), &f.requester)
        .await;
    assert_eq!(d.body["items"][0]["verification_status"], "unverified");
    assert_eq!(d.body["request"]["clarification_question"], "Voltage?");
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn visibility_and_tenant_isolation(pool: PgPool) {
    let f = Flow::new(pool).await;
    let app = &f.w.app;
    let (request, _, cmp, q1, _) = f.compared(&f.requester).await;
    let rec = f.recommend_and_send(request, cmp, q1).await;
    let other =
        f.w.member(f.w.org_a, &f.w.admin_a, "req2@a.test", "requester")
            .await;

    // Requesters see only their own requests.
    let r = app
        .get(&f.url(&format!("requests/{request}")), &other)
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = app.get(&f.url("requests"), &other).await;
    assert_eq!(r.body.as_array().unwrap().len(), 0);
    let r = app.get(&f.url("requests"), &f.manager1).await;
    assert_eq!(r.body.as_array().unwrap().len(), 1);
    // Requesters cannot read quotations or comparisons.
    let r = app
        .get(&f.url(&format!("comparisons/{cmp}")), &f.requester)
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // Org B sees nothing of org A, and gets 404 rather than 403.
    let b = |path: &str| format!("/api/v1/orgs/{}/{path}", f.w.org_a);
    for path in [
        format!("requests/{request}"),
        "requests".to_string(),
        format!("comparisons/{cmp}"),
        format!("recommendations/{rec}"),
        "suppliers".to_string(),
    ] {
        let r = app.get(&b(&path), &f.w.admin_b).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
    }
    // Using org B's context with org A ids finds nothing either.
    let in_b = format!("/api/v1/orgs/{}/requests/{request}", f.w.org_b);
    assert_eq!(
        app.get(&in_b, &f.w.admin_b).await.status,
        StatusCode::NOT_FOUND
    );
    let in_b = format!("/api/v1/orgs/{}/comparisons/{cmp}", f.w.org_b);
    let mgr_b =
        f.w.member(f.w.org_b, &f.w.admin_b, "m@b.test", "procurement_manager")
            .await;
    assert_eq!(app.get(&in_b, &mgr_b).await.status, StatusCode::NOT_FOUND);
    // Org B's manager sees no suppliers (none proposed to B), and cannot decide on A's.
    let r = app
        .get(&format!("/api/v1/orgs/{}/suppliers", f.w.org_b), &mgr_b)
        .await;
    assert_eq!(r.body.as_array().unwrap().len(), 0);
    let r = app
        .put(
            &format!("/api/v1/orgs/{}/suppliers/{}/decision", f.w.org_b, f.s1),
            &mgr_b,
            json!({ "decision": "approved" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    // Staff without an assignment to A cannot touch A's workflow.
    let op2 =
        f.w.member(
            f.w.platform_org,
            &f.w.super_admin,
            "op2@chuo-sogo.test",
            "procurement_operator",
        )
        .await;
    let r = app
        .post(
            &f.url(&format!("requests/{request}/pick-up")),
            &op2,
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // Customers cannot read the internal directory.
    let r = app.get("/api/v1/suppliers", &f.manager1).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = app
        .post(
            "/api/v1/suppliers",
            &f.w.admin_a,
            json!({ "name": "X", "country": "JP" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // A's view of suppliers never includes internal notes.
    let r = app.get(&f.url("suppliers"), &f.manager1).await;
    assert_eq!(r.body.as_array().unwrap().len(), 2);
    assert!(!r.body.to_string().contains("trade fair"));
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn approval_policy_validation_and_permissions(pool: PgPool) {
    let f = Flow::new(pool).await;
    let app = &f.w.app;
    let url = f.url("approval-policies/JPY");
    for (tiers, code) in [
        (
            json!([{ "min_minor": 0, "approvals": 3 }, { "min_minor": 10, "approvals": 1 }]),
            "decreasing_approvals",
        ),
        (
            json!([{ "min_minor": 0, "approvals": 0 }]),
            "invalid_approvals",
        ),
        (
            json!([{ "min_minor": -5, "approvals": 1 }]),
            "negative_threshold",
        ),
        (
            json!([{ "min_minor": 5, "approvals": 1 }, { "min_minor": 5, "approvals": 2 }]),
            "duplicate_threshold",
        ),
    ] {
        let r = app.put(&url, &f.w.admin_a, json!({ "tiers": tiers })).await;
        assert_eq!(r.body["error"]["detail"], code);
    }
    let r = app.put(&url, &f.manager1, json!({ "tiers": [] })).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = app.put(&url, &f.operator, json!({ "tiers": [] })).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = app
        .put(
            &f.url("approval-policies/XYZ"),
            &f.w.admin_a,
            json!({ "tiers": [] }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    for v in 1..=2 {
        let r = app
            .put(
                &url,
                &f.w.admin_a,
                json!({ "tiers": [{ "min_minor": 1, "approvals": v }] }),
            )
            .await;
        assert_eq!(r.body["version"], v);
    }
    let r = app.get(&f.url("approval-policies"), &f.manager1).await;
    assert_eq!(r.body[0]["tiers"][0]["approvals"], 2);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn database_rejects_cross_tenant_references(pool: PgPool) {
    let f = Flow::new(pool).await;
    let (request, items) = f.submitted(&f.requester).await;
    // Even bypassing the API: an org B row cannot point at org A's request or items.
    let mut tx =
        procure_server::db::tenant_tx(&f.w.app.pool, f.w.user_id(&f.w.admin_b).await, f.w.org_b)
            .await
            .unwrap();
    let r = sqlx::query(
        "INSERT INTO request_items (id, org_id, request_id, line_no, description, quantity)
         VALUES ($1, $2, $3, 99, 'x', 1)",
    )
    .bind(Uuid::now_v7())
    .bind(f.w.org_b)
    .bind(request)
    .execute(&mut *tx)
    .await;
    assert!(r.is_err());
    drop(tx);
    // RLS hides A's items from a B-context transaction.
    let mut tx =
        procure_server::db::tenant_tx(&f.w.app.pool, f.w.user_id(&f.w.admin_b).await, f.w.org_b)
            .await
            .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM request_items WHERE id = ANY($1)")
        .bind(&items)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 0);
    // Approval records are append-only for the runtime role.
    let r = sqlx::query("UPDATE approval_records SET decision = 'approve'")
        .execute(&mut *tx)
        .await;
    assert!(r.is_err());
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn review_findings_regressions(pool: PgPool) {
    let f = Flow::new(pool).await;
    let app = &f.w.app;
    let (request, rfq, cmp, q1, _) = f.compared(&f.requester).await;
    let r = f
        .post_op(
            &format!("requests/{request}/recommendations"),
            json!({ "comparison_id": cmp, "quotation_id": q1 }),
        )
        .await;
    let rec = uuid(&r.body["id"]);

    // An unsent recommendation is internal: customers cannot see it yet.
    let r = app
        .get(&f.url(&format!("recommendations/{rec}")), &f.manager1)
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = app
        .get(&f.url(&format!("requests/{request}")), &f.requester)
        .await;
    assert!(r.body["recommendation"].is_null());
    let r = app
        .get(&f.url(&format!("requests/{request}")), &f.operator)
        .await;
    assert_eq!(r.body["recommendation"]["id"], json!(rec));

    // Quotations are frozen once recommended.
    let items: Vec<Uuid> = r.body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| uuid(&i["id"]))
        .collect();
    let revise = json!({ "supplier_id": f.s1, "currency": "JPY", "lines": [
        { "request_item_id": items[0], "unit_price_minor": 1500, "quantity": 100 },
        { "request_item_id": items[1], "unit_price_minor": 45000, "quantity": 4 } ] });
    let r = f
        .post_op(&format!("rfqs/{rfq}/quotations"), revise.clone())
        .await;
    assert_eq!(r.body["error"]["detail"], "quotations_frozen");

    // Withdraw (reason required), then the revision can be recorded and compared again.
    let withdraw = format!("recommendations/{rec}/withdraw");
    assert_eq!(
        f.post_op(&withdraw, json!({})).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    f.ok(f
        .post_op(&withdraw, json!({ "reason": "Supplier revised price" }))
        .await);
    assert_eq!(f.status(request).await, "under_comparison");
    let _revised = uuid(&f.ok(f.post_op(&format!("rfqs/{rfq}/quotations"), revise).await)["id"]);
    // The old comparison is stale because the quotation set changed.
    let r = f
        .post_op(
            &format!("requests/{request}/recommendations"),
            json!({ "comparison_id": cmp, "quotation_id": q1 }),
        )
        .await;
    assert_eq!(r.body["error"]["detail"], "stale_comparison");
    let cmp2 = uuid(&f.compare(rfq).await["id"]);
    let detail = app
        .get(&f.url(&format!("comparisons/{cmp2}")), &f.operator)
        .await;
    let top = detail.body["results"][0]["quotation_id"].clone();
    let rec2 = f.recommend_and_send(request, cmp2, uuid(&top)).await;

    // The customer revokes its approval of the supplier: approval can no longer pass.
    let supplier = app
        .get(&f.url(&format!("recommendations/{rec2}")), &f.manager1)
        .await
        .body["recommendation"]["supplier_id"]
        .clone();
    let r = app
        .put(
            &f.url(&format!("suppliers/{}/decision", uuid(&supplier))),
            &f.manager1,
            json!({ "decision": "rejected", "reason": "Quality incident" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = f.decide(&f.manager1, rec2, "approve", "").await;
    assert_eq!(r.body["error"]["detail"], "supplier_not_approved");
    // Rejecting is still possible.
    let r = f
        .decide(&f.manager1, rec2, "reject", "Supplier no longer approved")
        .await;
    assert_eq!(r.body["status"], "rejected");

    // RFQs never go to a supplier the customer rejected.
    let (request2, items2) = f.submitted(&f.requester).await;
    f.sourcing(request2, &items2).await;
    let r = f
        .post_op(
            &format!("requests/{request2}/rfqs"),
            json!({ "supplier_ids": [uuid(&supplier)] }),
        )
        .await;
    assert_eq!(r.body["error"]["detail"], "supplier_rejected_by_customer");

    // Internal supplier notes are unreadable from a customer transaction, even with raw SQL.
    let mut tx =
        procure_server::db::tenant_tx(&app.pool, f.w.user_id(&f.manager1).await, f.w.org_a)
            .await
            .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM supplier_notes")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 0);
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM suppliers")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(visible, 2);
}
