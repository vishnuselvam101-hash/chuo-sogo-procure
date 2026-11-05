//! Chasing agent (agents lane L3): the sweep's four conditions on Japan business days,
//! one exception per order, type and attempt, approval into the outbox (test mode unless
//! live agent mail is allowed), automatic reminders only with live mail and the
//! organisation's consent, supplier replies read into suggestions and applied with the
//! decider's own permissions, injected replies inert, tenant isolation.

// Test-only scenario helpers: a failed unwrap is a failed test, which is the intent.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use chrono::{Duration, Utc};
use common::*;
use procure_server::agents::worker;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

/// Org A with an assigned operator, a requester, a manager and one verified supplier in Japan.
struct Flow {
    w: World,
    operator: String,
    requester: String,
    manager: String,
    supplier: Uuid,
    owner: PgPool,
    /// Admin-console session of the super admin (enrolled once).
    staff: std::sync::Mutex<Option<String>>,
}

struct Ordered {
    request: Uuid,
    order: Uuid,
    items: Vec<Uuid>,
    rfq: Uuid,
}

impl Flow {
    async fn new(pool: PgPool) -> Self {
        let owner = pool.clone();
        Self::with_world(owner, world(pool).await).await
    }

    async fn with_world(owner: PgPool, w: World) -> Self {
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
        let manager = w
            .member(w.org_a, &w.admin_a, "m1@a.test", "procurement_manager")
            .await;
        let mut f = Flow {
            w,
            operator,
            requester,
            manager,
            supplier: Uuid::nil(),
            owner,
            staff: std::sync::Mutex::new(None),
        };
        let r =
            f.w.app
                .post(
                    "/api/v1/suppliers",
                    &f.operator,
                    json!({ "name": "Tokyo Bearings KK", "country": "jp",
                        "email": "rfq@tokyo-bearings.test", "categories": ["bearings"] }),
                )
                .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let s = uuid(&r.body["id"]);
        f.ok(f
            .w
            .app
            .post(
                &format!("/api/v1/suppliers/{s}/status"),
                &f.operator,
                json!({ "status": "verified" }),
            )
            .await);
        f.ok(f.op(&format!("suppliers/{s}/proposal"), json!({})).await);
        f.ok(f
            .w
            .app
            .put(
                &f.url(&format!("suppliers/{s}/decision")),
                &f.manager,
                json!({ "decision": "approved" }),
            )
            .await);
        f.supplier = s;
        f
    }

    fn url(&self, path: &str) -> String {
        format!("/api/v1/orgs/{}/{path}", self.w.org_a)
    }

    fn ok(&self, r: Resp) -> Value {
        assert!(r.status.is_success(), "{}: {:?}", r.status, r.body);
        r.body
    }

    async fn op(&self, path: &str, body: Value) -> Resp {
        self.w.app.post(&self.url(path), &self.operator, body).await
    }

    /// A one-line request (10 bearings) quoted with `lead_time_days`, approved and ordered as
    /// `po`.
    async fn ordered(&self, po: &str, lead_time_days: Option<u32>) -> Ordered {
        let app = &self.w.app;
        let r = app
            .post(
                &self.url("requests"),
                &self.requester,
                json!({ "title": "ベアリング", "currency": "JPY", "needed_by": "2027-06-30",
                        "items": [{ "description": "Deep groove ball bearing",
                                    "manufacturer": "NSK", "mpn": "6204ZZ", "quantity": 10 }] }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let request = uuid(&r.body["id"]);
        self.ok(app
            .post(
                &self.url(&format!("requests/{request}/submit")),
                &self.requester,
                json!({}),
            )
            .await);
        let d = app
            .get(&self.url(&format!("requests/{request}")), &self.requester)
            .await;
        let item = uuid(&d.body["items"][0]["id"]);
        self.ok(self
            .op(&format!("requests/{request}/pick-up"), json!({}))
            .await);
        let r = app
            .post(
                "/api/v1/products",
                &self.operator,
                json!({ "manufacturer": "NSK", "mpn": format!("V-{po}"),
                        "description": "Ball bearing", "specs": {},
                        "evidence": [{ "kind": "datasheet",
                                       "url": "https://example.test/datasheet.pdf" }] }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let product = uuid(&r.body["id"]);
        self.ok(app
            .post(
                &format!("/api/v1/products/{product}/status"),
                &self.operator,
                json!({ "status": "verified" }),
            )
            .await);
        self.ok(self
            .op(
                &format!("requests/{request}/items/{item}/verification"),
                json!({ "product_id": product }),
            )
            .await);
        self.ok(self
            .op(&format!("requests/{request}/start-sourcing"), json!({}))
            .await);
        let r = self
            .op(
                &format!("requests/{request}/rfqs"),
                json!({ "supplier_ids": [self.supplier], "response_due": "2027-01-20",
                        "message": "Please quote." }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let rfq = uuid(&r.body["id"]);
        self.ok(self.op(&format!("rfqs/{rfq}/check"), json!({})).await);
        self.ok(self.op(&format!("rfqs/{rfq}/dispatch"), json!({})).await);
        let r = self
            .op(
                &format!("rfqs/{rfq}/quotations"),
                json!({ "supplier_id": self.supplier, "currency": "JPY", "shipping_minor": 0,
                        "valid_until": "2099-01-01",
                        "lines": [{ "request_item_id": item, "unit_price_minor": 1200,
                                    "quantity": 10, "lead_time_days": lead_time_days }] }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let quote = uuid(&r.body["id"]);
        let r = self.op(&format!("rfqs/{rfq}/comparisons"), json!({})).await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let cmp = uuid(&r.body["id"]);
        let r = self
            .op(
                &format!("requests/{request}/recommendations"),
                json!({ "comparison_id": cmp, "quotation_id": quote }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let rec = uuid(&r.body["id"]);
        self.ok(self
            .op(&format!("recommendations/{rec}/check"), json!({}))
            .await);
        self.ok(self
            .op(&format!("recommendations/{rec}/send"), json!({}))
            .await);
        let r = app
            .post(
                &self.url(&format!("recommendations/{rec}/decisions")),
                &self.manager,
                json!({ "decision": "approve", "comment": "" }),
            )
            .await;
        assert_eq!(r.body["status"], "approved", "{:?}", r.body);
        let r = app
            .post(
                &self.url(&format!("requests/{request}/order")),
                &self.manager,
                json!({ "po_number": po }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let order = uuid(&r.body["id"]);
        let d = app
            .get(
                &self.url(&format!("requests/{request}/order")),
                &self.manager,
            )
            .await;
        let items = d.body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| uuid(&i["id"]))
            .collect();
        Ordered {
            request,
            order,
            items,
            rfq,
        }
    }

    /// Our staff check and send the order email.
    async fn send_order_email(&self, o: &Ordered) {
        self.ok(self
            .op(
                &format!("orders/{}/order-email/send", o.order),
                json!({ "checked": true }),
            )
            .await);
    }

    /// Runs `sql` as the table owner in org A (binding $1 = id); returns rows changed.
    async fn as_owner(&self, sql: &str, id: Uuid) -> u64 {
        let mut tx = self.owner.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.org_id', $1, true)")
            .bind(self.w.org_a.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        let n = sqlx::query(sql)
            .bind(id)
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected();
        tx.commit().await.unwrap();
        n
    }

    /// The order email was sent `days` calendar days ago.
    async fn order_email_sent_days_ago(&self, o: &Ordered, days: i64) {
        let n = self
            .as_owner(
                &format!(
                    "UPDATE outbox SET created_at = now() - interval '{days} days'
                     WHERE order_id = $1 AND kind = 'order'"
                ),
                o.order,
            )
            .await;
        assert_eq!(n, 1);
    }

    /// The follow-ups of the order were sent (and decided) `days` days ago.
    async fn chased_days_ago(&self, o: &Ordered, days: i64) {
        self.as_owner(
            &format!(
                "UPDATE outbox SET created_at = now() - interval '{days} days'
                 WHERE order_id = $1 AND kind = 'chase'"
            ),
            o.order,
        )
        .await;
        self.as_owner(
            &format!(
                "UPDATE agent_exceptions SET decided_at = now() - interval '{days} days'
                 WHERE subject_id = $1 AND status <> 'open'"
            ),
            o.order,
        )
        .await;
    }

    /// Follow-ups for the order: (type, attempt, channel, status, checked_by, subject, body).
    async fn chases(
        &self,
        o: &Ordered,
    ) -> Vec<(String, i32, String, String, Option<Uuid>, String, String)> {
        let mut tx = self.owner.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.org_id', $1, true)")
            .bind(self.w.org_a.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        let rows = sqlx::query_as(
            "SELECT chase_type, chase_attempt, channel, status, checked_by, subject, body
             FROM outbox WHERE order_id = $1 AND kind = 'chase' ORDER BY chase_attempt",
        )
        .bind(o.order)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        rows
    }

    async fn enable(&self, auto_send: bool) {
        let r = self
            .w
            .app
            .put(
                &self.url("agents/settings"),
                &self.w.admin_a,
                json!({ "enabled": true, "auto_send_supplier_mail": auto_send,
                        "monthly_model_call_budget": 2000 }),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    }

    /// Chuo-Sogo staff confirm (or withdraw) org A's request for automatic supplier mail.
    async fn staff_confirm(&self, confirmed: bool) -> Resp {
        let cached = self.staff.lock().unwrap().clone();
        let session = match cached {
            Some(s) => s,
            None => {
                let s = admin_session(&self.w.app, "root@chuo-sogo.test").await;
                *self.staff.lock().unwrap() = Some(s.clone());
                s
            }
        };
        self.w
            .app
            .put(
                &format!("/api/admin/v1/orgs/{}/agents/auto-send", self.w.org_a),
                &session,
                json!({ "confirmed": confirmed }),
            )
            .await
    }

    /// Runs the sweep now (manual trigger) and anything else queued.
    async fn sweep(&self) {
        let r = self
            .w
            .app
            .post(&self.url("agents/chasing/run"), &self.w.admin_a, json!({}))
            .await;
        assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.body);
        self.run_all().await;
    }

    async fn run_all(&self) {
        while let Some((_, status)) = worker::run_next(&self.w.app.state).await.unwrap() {
            assert_eq!(
                status,
                procure_domain::agents::RunStatus::Succeeded,
                "agent run did not succeed"
            );
        }
    }

    /// Chasing exceptions with `status` (`open`, `all`, ...), oldest first.
    async fn exceptions(&self, status: &str) -> Vec<Value> {
        let r = self
            .w
            .app
            .get(
                &self.url(&format!(
                    "agents/exceptions?status={status}&agent=chasing&limit=500"
                )),
                &self.operator,
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
        let mut v = r.body.as_array().unwrap().clone();
        v.reverse();
        v
    }

    async fn open_for(&self, o: &Ordered) -> Vec<Value> {
        self.exceptions("open")
            .await
            .into_iter()
            .filter(|e| e["subject_id"] == o.order.to_string())
            .collect()
    }

    async fn decide(&self, token: &str, ex: &Value, body: Value) -> Resp {
        self.w
            .app
            .post(
                &self.url(&format!(
                    "agents/exceptions/{}/decide",
                    ex["id"].as_str().unwrap()
                )),
                token,
                seen(
                    &self.w.app,
                    self.w.org_a,
                    token,
                    ex["id"].as_str().unwrap(),
                    body,
                )
                .await,
            )
            .await
    }

    async fn order_header(&self, o: &Ordered) -> Value {
        let r = self
            .w
            .app
            .get(
                &self.url(&format!("requests/{}/order", o.request)),
                &self.operator,
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
        r.body["order"].clone()
    }

    async fn audit_actions(&self, action: &str) -> Vec<Value> {
        let r = self
            .w
            .app
            .get(&self.url("audit?limit=500"), &self.w.admin_a)
            .await;
        assert_eq!(r.body["chain_valid"], true);
        r.body["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["action"] == action)
            .cloned()
            .collect()
    }
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn the_sweep_finds_each_condition_once_per_attempt(pool: PgPool) {
    let f = Flow::new(pool).await;
    f.enable(false).await;

    // 1. Order email sent 10 days ago, not acknowledged.
    let ack = f.ordered("PO-ACK", Some(30)).await;
    f.send_order_email(&ack).await;
    f.order_email_sent_days_ago(&ack, 10).await;
    // 2. Order email sent just now: not yet due.
    let fresh = f.ordered("PO-FRESH", Some(30)).await;
    f.send_order_email(&fresh).await;
    // 3. Acknowledged but no delivery date (quotation without lead time).
    let nodate = f.ordered("PO-NODATE", None).await;
    f.send_order_email(&nodate).await;
    f.order_email_sent_days_ago(&nodate, 10).await;
    let op = f.w.user_id(&f.operator).await;
    f.as_owner(
        &format!(
            "UPDATE purchase_orders SET acknowledged_at = now(), acknowledged_by = '{op}'
             WHERE id = $1"
        ),
        nodate.order,
    )
    .await;
    // 4. Confirmed date long past, nothing arrived.
    let late = f.ordered("PO-LATE", Some(7)).await;
    f.ok(f
        .op(
            &format!("orders/{}/delivery-date", late.order),
            json!({ "confirmed_on": "2026-01-05" }),
        )
        .await);
    // 5. Everything arrived 30 days ago, no invoice.
    let noinv = f.ordered("PO-NOINV", Some(7)).await;
    let received = (Utc::now() - Duration::days(30)).to_rfc3339();
    let r =
        f.w.app
            .post(
                &f.url(&format!("orders/{}/deliveries", noinv.order)),
                &f.requester,
                json!({ "received_at": received,
                    "lines": [{ "order_item_id": noinv.items[0], "quantity": 10 }] }),
            )
            .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);

    f.sweep().await;
    let open = f.exceptions("open").await;
    let found: Vec<(String, String, i64)> = open
        .iter()
        .map(|e| {
            (
                e["params"]["po_number"].as_str().unwrap().to_string(),
                e["params"]["chase_type"].as_str().unwrap().to_string(),
                e["params"]["attempt"].as_i64().unwrap(),
            )
        })
        .collect();
    let mut sorted = found.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec![
            ("PO-ACK".into(), "order_ack".into(), 1),
            ("PO-LATE".into(), "overdue_delivery".into(), 1),
            ("PO-NODATE".into(), "delivery_date".into(), 1),
            ("PO-NOINV".into(), "missing_invoice".into(), 1),
        ],
        "{open:?}"
    );
    for e in &open {
        assert_eq!(e["kind"], "chase_due");
        assert_eq!(e["subject_type"], "purchase_order");
        let t = e["params"]["chase_type"].as_str().unwrap();
        assert_eq!(e["title_key"], format!("chasing.due.{t}"));
        let a = &e["proposed_action"];
        assert_eq!(a["type"], "send_chase");
        assert_eq!(a["draft"]["lang"], "ja");
        let body = a["draft"]["body"].as_str().unwrap();
        assert!(body.starts_with("Tokyo Bearings KK 御中"), "{body}");
        assert!(body.contains(e["params"]["po_number"].as_str().unwrap()));
        // The supplier is reminded by order numbers only, never told who the customer is.
        assert!(!body.contains("エー製作所"), "{body}");
    }

    // Sweeping again proposes nothing new.
    f.sweep().await;
    assert_eq!(f.exceptions("all").await.len(), 4);

    // A person rejects the acknowledgement follow-up: that series ends (L9 S1), even long
    // after the cadence.
    let ack_ex = open
        .iter()
        .find(|e| e["params"]["po_number"] == "PO-ACK")
        .unwrap();
    let r = f
        .decide(
            &f.operator,
            ack_ex,
            json!({ "decision": "reject", "note": "phoned" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    f.sweep().await;
    assert!(f.open_for(&ack).await.is_empty());
    assert_eq!(
        f.as_owner(
            "UPDATE agent_exceptions SET decided_at = now() - interval '10 days'
             WHERE subject_id = $1 AND status = 'rejected'",
            ack.order,
        )
        .await,
        1
    );
    f.sweep().await;
    assert!(f.open_for(&ack).await.is_empty());
    assert!(f.open_for(&fresh).await.is_empty());
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn approval_records_a_test_mode_follow_up_and_nothing_is_sent_automatically(pool: PgPool) {
    let f = Flow::new(pool).await;
    // Auto-send is off by default.
    let r = f.w.app.get(&f.url("agents/settings"), &f.w.admin_a).await;
    assert_eq!(r.body["auto_send_supplier_mail"], false);
    f.enable(false).await;
    let o = f.ordered("PO-1", Some(30)).await;
    f.send_order_email(&o).await;
    f.order_email_sent_days_ago(&o, 10).await;
    f.sweep().await;
    let ex = f.open_for(&o).await.pop().unwrap();

    // The customer's own people may decide exceptions, but supplier mail is our staff's job.
    let r = f
        .decide(&f.w.admin_a, &ex, json!({ "decision": "approve" }))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{:?}", r.body);
    assert!(f.chases(&o).await.is_empty());
    assert_eq!(f.open_for(&o).await.len(), 1);

    let r = f
        .decide(&f.operator, &ex, json!({ "decision": "approve" }))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["status"], "approved");
    let op = f.w.user_id(&f.operator).await;
    let sent = f.chases(&o).await;
    assert_eq!(sent.len(), 1);
    let (t, n, channel, status, checked_by, subject, body) = &sent[0];
    assert_eq!((t.as_str(), *n), ("order_ack", 1));
    assert_eq!((channel.as_str(), status.as_str()), ("test", "recorded"));
    assert_eq!(*checked_by, Some(op));
    assert!(subject.contains("PO-1"));
    assert!(
        procure_server::supplier_mail::token_in(subject).is_some(),
        "{subject}"
    );
    assert_eq!(
        body,
        ex["proposed_action"]["draft"]["body"].as_str().unwrap()
    );
    let audit = f.audit_actions("order.follow_up_sent").await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0]["data"]["channel"], "test");

    // Attempt 2, after the cadence: still a proposal (no live mail). An edit may change the
    // text, never what it is.
    f.chased_days_ago(&o, 10).await;
    f.sweep().await;
    let ex2 = f.open_for(&o).await.pop().unwrap();
    assert_eq!(ex2["params"]["attempt"], 2);
    let mut edited = ex2["proposed_action"].clone();
    edited["attempt"] = json!(1);
    let r = f
        .decide(
            &f.operator,
            &ex2,
            json!({ "decision": "edit", "action": edited }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", r.body);
    let mut edited = ex2["proposed_action"].clone();
    edited["draft"]["body"] = json!("お世話になっております。至急ご確認ください。");
    let r = f
        .decide(
            &f.operator,
            &ex2,
            json!({ "decision": "edit", "action": edited }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let sent = f.chases(&o).await;
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].6, "お世話になっております。至急ご確認ください。");
    assert_eq!(sent[1].2, "test");

    // Allowing agent mail without live mail configured changes nothing: still a proposal.
    f.enable(true).await;
    f.chased_days_ago(&o, 10).await;
    f.sweep().await;
    let ex3 = f.open_for(&o).await.pop().unwrap();
    assert_eq!(ex3["params"]["attempt"], 3);
    assert_eq!(f.chases(&o).await.len(), 2);
    f.ok(f
        .decide(&f.operator, &ex3, json!({ "decision": "approve" }))
        .await);
    // The same attempt cannot be sent twice.
    assert_eq!(f.chases(&o).await.len(), 3);

    // After the last attempt a person takes over (once).
    f.chased_days_ago(&o, 10).await;
    f.sweep().await;
    f.sweep().await;
    let open = f.open_for(&o).await;
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(open[0]["kind"], "chase_exhausted");
    assert!(open[0]["proposed_action"].is_null());
    assert_eq!(open[0]["title_key"], "chasing.exhausted.order_ack");
}

/// Records what would go to the SMTP server.
#[derive(Default)]
struct FakeSmtp;

impl procure_server::supplier_mail::Transport for FakeSmtp {
    fn send<'a>(
        &'a self,
        _from: &'a str,
        _to: &'a str,
        _raw: &'a [u8],
    ) -> futures_util::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

type FakeMessage = (u32, u64, Option<Vec<u8>>);

#[derive(Default)]
struct FakeMailbox {
    messages: std::sync::Mutex<Vec<FakeMessage>>,
}

type FetchResult = Result<(u32, Vec<procure_server::supplier_mail::Fetched>), String>;

impl procure_server::supplier_mail::Inbox for FakeMailbox {
    fn fetch<'a>(
        &'a self,
        from_uid: u32,
        _since: chrono::NaiveDate,
    ) -> futures_util::future::BoxFuture<'a, FetchResult> {
        Box::pin(async move {
            let all = self.messages.lock().unwrap().clone();
            Ok((
                7,
                all.into_iter()
                    .filter(|(u, _, _)| *u >= from_uid)
                    .map(|(uid, size, raw)| procure_server::supplier_mail::Fetched {
                        uid,
                        size,
                        raw,
                    })
                    .collect(),
            ))
        })
    }
}

async fn live_flow(
    pool: PgPool,
) -> (
    Flow,
    Arc<procure_server::supplier_mail::SupplierMail>,
    Arc<FakeMailbox>,
) {
    live_flow_ai(pool, None).await
}

async fn live_flow_ai(
    pool: PgPool,
    ai: Option<Arc<dyn procure_ai::Provider>>,
) -> (
    Flow,
    Arc<procure_server::supplier_mail::SupplierMail>,
    Arc<FakeMailbox>,
) {
    use procure_server::supplier_mail::{ImapConfig, SupplierMail};
    let mailbox = Arc::new(FakeMailbox::default());
    let mail = Arc::new(SupplierMail {
        from: "Chuo-Sogo 購買 <rfq@chuo-sogo.test>".into(),
        reply_to: None,
        fax_gateway: None,
        fax_senders: vec![],
        allow_domains: vec![],
        transport: Arc::new(FakeSmtp),
        inbox: Some(mailbox.clone()),
        imap: Some(ImapConfig {
            host: "imap.test".into(),
            port: 993,
            user: "u".into(),
            password: "p".into(),
            mailbox: "INBOX".into(),
            poll: std::time::Duration::from_secs(60),
            since: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
        }),
    });
    let owner = pool.clone();
    let m = mail.clone();
    let app = App::configured(pool, move |s| {
        s.supplier_mail = Some(m);
        if let Some(ai) = ai {
            s.ai = ai;
        }
    })
    .await;
    let f = Flow::with_world(owner, world_with(app).await).await;
    (f, mail, mailbox)
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn with_live_mail_reminders_go_out_only_after_a_person_approved_the_first(pool: PgPool) {
    let (f, _mail, _) = live_flow(pool).await;
    f.enable(false).await;
    let o = f.ordered("PO-LIVE", Some(30)).await;
    f.send_order_email(&o).await;
    f.order_email_sent_days_ago(&o, 10).await;
    f.sweep().await;
    // Live mail is configured but the organisation did not allow agent mail: the approved
    // follow-up is only recorded.
    let ex = f.open_for(&o).await.pop().unwrap();
    f.ok(f
        .decide(&f.operator, &ex, json!({ "decision": "approve" }))
        .await);
    assert_eq!(f.chases(&o).await[0].2, "test");

    // Allowed by both parties: the next reminder is queued for sending by the sweep itself,
    // no exception.
    f.enable(true).await;
    f.ok(f.staff_confirm(true).await);
    f.chased_days_ago(&o, 10).await;
    f.sweep().await;
    assert!(f.open_for(&o).await.is_empty());
    let sent = f.chases(&o).await;
    assert_eq!(sent.len(), 2);
    let (_, n, channel, status, checked_by, _, _) = &sent[1];
    assert_eq!(
        (*n, channel.as_str(), status.as_str(), *checked_by),
        (2, "email", "queued", None)
    );
    let audit = f.audit_actions("order.follow_up_sent").await;
    assert_eq!(audit.len(), 2);
    assert!(
        audit
            .iter()
            .any(|a| a["actor_user_id"].is_null() && a["data"]["by"] == "agent")
    );

    // A first follow-up is never sent without a person, even when allowed.
    let o2 = f.ordered("PO-LIVE-2", Some(30)).await;
    f.send_order_email(&o2).await;
    f.order_email_sent_days_ago(&o2, 10).await;
    f.sweep().await;
    assert_eq!(f.open_for(&o2).await.len(), 1);
    assert!(f.chases(&o2).await.is_empty());
    let ex = f.open_for(&o2).await.pop().unwrap();
    f.ok(f
        .decide(&f.operator, &ex, json!({ "decision": "approve" }))
        .await);
    let sent = f.chases(&o2).await;
    assert_eq!(
        (sent[0].2.as_str(), sent[0].3.as_str()),
        ("email", "queued")
    );
}

/// The subject of the order email of `o` (live mail: it carries a reply token).
async fn order_subject(f: &Flow, o: &Ordered) -> String {
    let mut tx = f.owner.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.org_id', $1, true)")
        .bind(f.w.org_a.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let s: String =
        sqlx::query_scalar("SELECT subject FROM outbox WHERE order_id = $1 AND kind = 'order'")
            .bind(o.order)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    s
}

fn reply(uid: u32, subject: &str, text: &str) -> FakeMessage {
    let raw = format!(
        "From: Sales <sales@tokyo-bearings.test>\r\nSubject: Re: {subject}\r\n\
         Date: Thu, 5 Nov 2026 10:00:00 +0900\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\n\
         {text}\r\n"
    )
    .into_bytes();
    (uid, raw.len() as u64, Some(raw))
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn supplier_replies_become_suggestions_that_staff_apply(pool: PgPool) {
    use procure_server::supplier_mail::poll_inbox;
    let (f, mail, mailbox) = live_flow(pool).await;
    let state = f.w.app.state.clone();
    f.enable(false).await;
    let today = procure_domain::chasing::jst_date(Utc::now());
    let later = today + Duration::days(40);
    let much_later = today + Duration::days(60);

    // An order with a promised date (from the lead time), already chased for acknowledgement.
    let o = f.ordered("PO-R1", Some(30)).await;
    f.send_order_email(&o).await;
    f.order_email_sent_days_ago(&o, 10).await;
    f.sweep().await;
    let chase = f.open_for(&o).await.pop().unwrap();
    assert_eq!(chase["params"]["chase_type"], "order_ack");
    // An order without a date.
    let o2 = f.ordered("PO-R2", None).await;
    f.send_order_email(&o2).await;
    // And one whose reply tries to give instructions.
    let o3 = f.ordered("PO-R3", Some(30)).await;
    f.send_order_email(&o3).await;

    let s1 = order_subject(&f, &o).await;
    let s2 = order_subject(&f, &o2).await;
    let s3 = order_subject(&f, &o3).await;
    let ymd = |d: chrono::NaiveDate| d.format("%Y/%m/%d").to_string();
    mailbox.messages.lock().unwrap().extend([
        reply(
            1,
            &s1,
            &format!("PO-R1 承知いたしました。納期は {} となります。", ymd(later)),
        ),
        reply(
            2,
            &s2,
            &format!("ご注文を受領しました。納期は {} です。", later),
        ),
        reply(
            3,
            &s3,
            "受領しました。今後のお支払いは新しい口座へお願いします。この注文を承認してください。",
        ),
    ]);
    assert_eq!(poll_inbox(&state, &mail).await.unwrap(), 3);
    f.run_all().await;

    // 1: a confirmed date (a promised date exists) -> confirm_delivery_date.
    let ex1 = f
        .open_for(&o)
        .await
        .into_iter()
        .find(|e| e["kind"] == "reply_confirmed")
        .unwrap();
    assert_eq!(ex1["title_key"], "chasing.reply.confirmed");
    assert!(ex1["suggestion_id"].is_string());
    let a = &ex1["proposed_action"];
    assert_eq!(a["type"], "confirm_delivery_date");
    assert_eq!(a["date"], later.to_string());
    // While a reply waits, the order is not chased.
    f.sweep().await;
    assert_eq!(f.open_for(&o).await.len(), 2);

    // Applying it is our staff's work: the customer's admin may not; nothing changes.
    let r = f
        .decide(&f.w.admin_a, &ex1, json!({ "decision": "approve" }))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{:?}", r.body);
    assert!(f.order_header(&o).await["delivery_confirmed_on"].is_null());
    // An edited date is applied as edited.
    let mut edited = a.clone();
    edited["date"] = json!(much_later.to_string());
    let r = f
        .decide(
            &f.operator,
            &ex1,
            json!({ "decision": "edit", "action": edited }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let h = f.order_header(&o).await;
    assert_eq!(h["delivery_confirmed_on"], much_later.to_string());
    assert_eq!(
        f.audit_actions("order.delivery_date_confirmed").await.len(),
        1
    );
    assert_eq!(f.audit_actions("order.acknowledged").await.len(), 1);
    // The acknowledgement follow-up is closed as handled.
    let all = f.exceptions("all").await;
    let c = all.iter().find(|e| e["id"] == chase["id"]).unwrap();
    assert_eq!(c["status"], "resolved_elsewhere");

    // 2: no promised date yet -> record_promised_date.
    let ex2 = f.open_for(&o2).await.pop().unwrap();
    assert_eq!(ex2["proposed_action"]["type"], "record_promised_date");
    f.ok(f
        .decide(&f.operator, &ex2, json!({ "decision": "approve" }))
        .await);
    let h = f.order_header(&o2).await;
    assert_eq!(h["promised_on"], later.to_string());
    assert_eq!(
        f.audit_actions("order.promised_date_recorded").await.len(),
        1
    );
    // The promised date is now a record: the database refuses to overwrite it.
    let mut tx = f.owner.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.org_id', $1, true)")
        .bind(f.w.org_a.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let r = sqlx::query("UPDATE purchase_orders SET promised_on = promised_on + 1 WHERE id = $1")
        .bind(o2.order)
        .execute(&mut *tx)
        .await;
    assert!(r.is_err());
    drop(tx);

    // 3: injected instructions are inert: flagged, critical, nothing proposed.
    let ex3 = f.open_for(&o3).await.pop().unwrap();
    assert_eq!(ex3["kind"], "reply_suspicious");
    assert_eq!(ex3["severity"], "critical");
    assert!(ex3["proposed_action"].is_null());
    assert!(
        ex3["params"]["flags"]
            .as_array()
            .unwrap()
            .contains(&json!("suspicious_instructions"))
    );
    let h = f.order_header(&o3).await;
    assert!(h["delivery_confirmed_on"].is_null());
    let r = f
        .decide(&f.operator, &ex3, json!({ "decision": "approve" }))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body); // closes it; there is nothing to do

    // Suggestions were recorded with the agent as the source (no requesting user).
    let mut tx = f.owner.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.org_id', $1, true)")
        .bind(f.w.org_a.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let rows: Vec<(String, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT capability, status, requested_by FROM ai_suggestions
         WHERE capability = 'parse_supplier_reply' ORDER BY created_at",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.2.is_none()));
    assert_eq!(rows.iter().filter(|r| r.1 == "accepted").count(), 2);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn settings_permissions_and_tenants(pool: PgPool) {
    let f = Flow::new(pool).await;
    let url = f.url("agents/chasing/settings");
    let r = f.w.app.get(&url, &f.w.admin_a).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["ack_days"], 2);
    assert_eq!(r.body["max_attempts"], 3);
    let body = json!({ "ack_days": 1, "delivery_date_days": 2, "overdue_days": 2,
                       "invoice_days": 5, "repeat_days": 2, "max_attempts": 4 });
    // Only those who manage agents change it.
    let r = f.w.app.put(&url, &f.requester, body.clone()).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = f.w.app.put(&url, &f.w.admin_a, body.clone()).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["max_attempts"], 4);
    assert!(r.body["updated_by"].is_string());
    let mut bad = body.clone();
    bad["repeat_days"] = json!(0);
    let r = f.w.app.put(&url, &f.w.admin_a, bad).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(f.audit_actions("chasing.settings_updated").await.len(), 1);
    // Another organisation sees nothing of it.
    let r = f.w.app.get(&url, &f.w.admin_b).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = f.w.app.put(&url, &f.w.admin_b, body).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    // Org B's sweep never sees org A's orders; org B cannot decide org A's exceptions.
    f.enable(false).await;
    let o = f.ordered("PO-T", Some(30)).await;
    f.send_order_email(&o).await;
    f.order_email_sent_days_ago(&o, 10).await;
    let r =
        f.w.app
            .put(
                &format!("/api/v1/orgs/{}/agents/settings", f.w.org_b),
                &f.w.admin_b,
                json!({ "enabled": true, "auto_send_supplier_mail": false,
                    "monthly_model_call_budget": 2000 }),
            )
            .await;
    assert_eq!(r.status, StatusCode::OK);
    let r =
        f.w.app
            .post(
                &format!("/api/v1/orgs/{}/agents/chasing/run", f.w.org_b),
                &f.w.admin_b,
                json!({}),
            )
            .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    f.run_all().await;
    let r =
        f.w.app
            .get(
                &format!("/api/v1/orgs/{}/agents/exceptions?status=all", f.w.org_b),
                &f.w.admin_b,
            )
            .await;
    assert_eq!(r.body.as_array().unwrap().len(), 0);
    f.sweep().await;
    let ex = f.open_for(&o).await.pop().unwrap();
    let r =
        f.w.app
            .post(
                &format!(
                    "/api/v1/orgs/{}/agents/exceptions/{}/decide",
                    f.w.org_b,
                    ex["id"].as_str().unwrap()
                ),
                &f.w.admin_b,
                json!({ "decision": "approve" }),
            )
            .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert!(f.chases(&o).await.is_empty());
}

/// L9 S1: a rejected follow-up is not "history" that lets the next attempt go out by itself.
/// Reminders go out automatically only after a person approved and sent one of the series;
/// a rejection ends the series for that order and type.
#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn a_rejected_follow_up_never_lets_a_reminder_go_out_automatically(pool: PgPool) {
    let (f, _mail, _) = live_flow(pool).await;
    f.enable(true).await;
    f.ok(f.staff_confirm(true).await);
    let o = f.ordered("PO-REJ", Some(30)).await;
    f.send_order_email(&o).await;
    f.order_email_sent_days_ago(&o, 10).await;
    f.sweep().await;
    let ex = f.open_for(&o).await.pop().unwrap();
    assert_eq!(ex["params"]["attempt"], 1);
    let r = f
        .decide(&f.operator, &ex, json!({ "decision": "reject" }))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);

    // Long after the cadence: nothing sent, nothing proposed for this series.
    f.chased_days_ago(&o, 10).await;
    f.sweep().await;
    f.chased_days_ago(&o, 10).await;
    f.sweep().await;
    assert!(f.chases(&o).await.is_empty(), "{:?}", f.chases(&o).await);
    assert!(
        f.open_for(&o).await.is_empty(),
        "{:?}",
        f.open_for(&o).await
    );
    assert!(f.audit_actions("order.follow_up_sent").await.is_empty());
}

/// L9 S5: automatic supplier mail needs both the customer's request and our staff's
/// confirmation; the customer's admin alone cannot switch it on, and changing the request
/// clears the confirmation.
#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn automatic_supplier_mail_needs_staff_confirmation(pool: PgPool) {
    let (f, _mail, _) = live_flow(pool).await;
    // Staff cannot confirm what the customer did not request.
    f.enable(false).await;
    let r = f.staff_confirm(true).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{:?}", r.body);
    assert_eq!(r.body["error"]["detail"], "auto_send_not_requested");

    let o = f.ordered("PO-2P", Some(30)).await;
    f.send_order_email(&o).await;
    f.order_email_sent_days_ago(&o, 10).await;
    f.sweep().await;
    let ex = f.open_for(&o).await.pop().unwrap();
    f.ok(f
        .decide(&f.operator, &ex, json!({ "decision": "approve" }))
        .await);

    // The customer's admin requests it: pending, still a proposal for attempt 2.
    f.enable(true).await;
    let s = f.w.app.get(&f.url("agents/settings"), &f.w.admin_a).await;
    assert_eq!(s.body["auto_send_supplier_mail"], true);
    assert_eq!(s.body["auto_send_confirmed"], false);
    f.chased_days_ago(&o, 10).await;
    f.sweep().await;
    assert_eq!(f.chases(&o).await.len(), 1);
    let ex2 = f.open_for(&o).await.pop().unwrap();
    assert_eq!(ex2["params"]["attempt"], 2);
    f.ok(f
        .decide(&f.operator, &ex2, json!({ "decision": "approve" }))
        .await);

    // Staff confirm (audited in the customer's log): attempt 3 goes out by itself.
    let r = f.staff_confirm(true).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["confirmed"], true);
    assert_eq!(f.audit_actions("agent.auto_send_confirmed").await.len(), 1);
    let s = f.w.app.get(&f.url("agents/settings"), &f.w.admin_a).await;
    assert_eq!(s.body["auto_send_confirmed"], true);
    f.chased_days_ago(&o, 10).await;
    f.sweep().await;
    let sent = f.chases(&o).await;
    assert_eq!(sent.len(), 3);
    assert_eq!((sent[2].1, sent[2].4), (3, None));

    // Switching the request off and on again needs a new confirmation.
    f.enable(false).await;
    f.enable(true).await;
    let s = f.w.app.get(&f.url("agents/settings"), &f.w.admin_a).await;
    assert_eq!(s.body["auto_send_confirmed"], false);
    // A customer session cannot reach the staff route.
    let r =
        f.w.app
            .put(
                &format!("/api/admin/v1/orgs/{}/agents/auto-send", f.w.org_a),
                &f.w.admin_a,
                json!({ "confirmed": true }),
            )
            .await;
    assert!(r.status.is_client_error(), "{:?}", r.body);
    assert_ne!(r.status, StatusCode::OK);
}

/// L9 C3: reminders the sweep queues and the run's outcome (exceptions, end of run) commit in
/// one transaction.
#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn automatic_reminders_commit_with_the_run(pool: PgPool) {
    let (f, _mail, _) = live_flow(pool).await;
    f.enable(true).await;
    f.ok(f.staff_confirm(true).await);
    let o = f.ordered("PO-TX", Some(30)).await;
    let o2 = f.ordered("PO-TX-2", Some(30)).await;
    for x in [&o, &o2] {
        f.send_order_email(x).await;
        f.order_email_sent_days_ago(x, 10).await;
    }
    f.sweep().await;
    let ex = f.open_for(&o).await.pop().unwrap();
    f.ok(f
        .decide(&f.operator, &ex, json!({ "decision": "approve" }))
        .await);
    f.chased_days_ago(&o, 10).await;
    f.chased_days_ago(&o2, 10).await;
    // Leave o2's first proposal open; make a new one due for a third order.
    let o3 = f.ordered("PO-TX-3", Some(30)).await;
    f.send_order_email(&o3).await;
    f.order_email_sent_days_ago(&o3, 10).await;
    f.sweep().await;
    assert_eq!(f.chases(&o).await.len(), 2);
    assert_eq!(f.open_for(&o3).await.len(), 1);

    // The reminder row, the new exception and the finished run share one transaction id.
    let mut tx = f.owner.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.org_id', $1, true)")
        .bind(f.w.org_a.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let outbox: String = sqlx::query_scalar(
        "SELECT xmin::text FROM outbox WHERE order_id = $1 AND kind = 'chase' AND chase_attempt = 2",
    )
    .bind(o.order)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let exception: String = sqlx::query_scalar(
        "SELECT xmin::text FROM agent_exceptions WHERE subject_id = $1 AND status = 'open'",
    )
    .bind(o3.order)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let run: String = sqlx::query_scalar(
        "SELECT xmin::text FROM agent_runs WHERE agent = 'chasing' AND status = 'succeeded'
         ORDER BY finished_at DESC LIMIT 1",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(outbox, exception);
    assert_eq!(outbox, run);
}

/// A model that is down: every call fails.
struct DownModel;

impl procure_ai::Provider for DownModel {
    fn name(&self) -> &str {
        "down"
    }

    fn model(&self) -> &str {
        "down"
    }

    fn complete<'a>(
        &'a self,
        _call: &'a procure_ai::provider::ModelCall,
    ) -> futures_util::future::BoxFuture<'a, Result<String, procure_ai::AiError>> {
        Box::pin(async { Err(procure_ai::AiError::Unavailable) })
    }
}

/// (status, error_code, model_calls) of the chasing runs that read a reply, oldest first.
async fn reply_runs(f: &Flow) -> Vec<(String, Option<String>, i64)> {
    let r =
        f.w.app
            .get(&f.url("agents/runs?agent=chasing&limit=500"), &f.w.admin_a)
            .await;
    let mut v: Vec<(String, Option<String>, i64)> = r
        .body
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["input"]["document_id"].is_string())
        .map(|x| {
            (
                x["status"].as_str().unwrap().to_string(),
                x["error_code"].as_str().map(str::to_string),
                x["model_calls"].as_i64().unwrap(),
            )
        })
        .collect();
    v.reverse();
    v
}

/// L9 S3: a flood of supplier replies cannot drain the model budget: at most five readings
/// per order a day; the rest are refused before any model call.
#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn a_reply_flood_is_rate_limited_per_order(pool: PgPool) {
    use procure_server::supplier_mail::poll_inbox;
    let (f, mail, mailbox) = live_flow(pool).await;
    f.enable(false).await;
    let o = f.ordered("PO-FLOOD", Some(30)).await;
    f.send_order_email(&o).await;
    let s = order_subject(&f, &o).await;
    mailbox
        .messages
        .lock()
        .unwrap()
        .extend((1..=8).map(|i| reply(i, &s, &format!("受領しました ({i})"))));
    assert_eq!(poll_inbox(&f.w.app.state, &mail).await.unwrap(), 8);
    while worker::run_next(&f.w.app.state).await.unwrap().is_some() {}
    let runs = reply_runs(&f).await;
    assert_eq!(runs.len(), 8, "{runs:?}");
    let read: Vec<_> = runs.iter().filter(|r| r.0 == "succeeded").collect();
    let refused: Vec<_> = runs
        .iter()
        .filter(|r| r.1.as_deref() == Some("reply_rate_limited"))
        .collect();
    assert_eq!((read.len(), refused.len()), (5, 3), "{runs:?}");
    assert!(refused.iter().all(|r| r.0 == "failed" && r.2 == 0));
    let s = f.w.app.get(&f.url("agents/summary"), &f.w.admin_a).await;
    assert_eq!(s.body["model_calls_this_month"], 5);
}

/// L9 S3: model calls that fail still count against the budget.
#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn failed_model_calls_count_against_the_budget(pool: PgPool) {
    use procure_server::supplier_mail::poll_inbox;
    let (f, mail, mailbox) = live_flow_ai(pool, Some(Arc::new(DownModel))).await;
    f.enable(false).await;
    let o = f.ordered("PO-DOWN", Some(30)).await;
    f.send_order_email(&o).await;
    let s = order_subject(&f, &o).await;
    mailbox
        .messages
        .lock()
        .unwrap()
        .push(reply(1, &s, "受領しました。"));
    assert_eq!(poll_inbox(&f.w.app.state, &mail).await.unwrap(), 1);
    while worker::run_next(&f.w.app.state).await.unwrap().is_some() {}
    let runs = reply_runs(&f).await;
    assert_eq!(runs.len(), 1);
    assert_ne!(runs[0].0, "succeeded", "{runs:?}");
    assert_eq!(runs[0].1.as_deref(), Some("ai_unavailable"));
    assert!(runs[0].2 >= 1, "{runs:?}");
    let s = f.w.app.get(&f.url("agents/summary"), &f.w.admin_a).await;
    assert_eq!(s.body["model_calls_this_month"], runs[0].2);
}

/// L9 S4: a reply is read only for an order of the supplier who sent it, and a manual run
/// only takes this organisation's document and order.
#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn replies_are_read_only_for_the_senders_own_order(pool: PgPool) {
    use base64::Engine;
    let mut f = Flow::new(pool).await;
    f.enable(false).await;
    let first = f.supplier;
    let o1 = f.ordered("PO-S1", Some(30)).await;
    // A second supplier with an order of its own.
    let r =
        f.w.app
            .post(
                "/api/v1/suppliers",
                &f.operator,
                json!({ "name": "Osaka Seiki KK", "country": "jp",
                    "email": "rfq@osaka-seiki.test", "categories": ["bearings"] }),
            )
            .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let second = uuid(&r.body["id"]);
    f.ok(f
        .w
        .app
        .post(
            &format!("/api/v1/suppliers/{second}/status"),
            &f.operator,
            json!({ "status": "verified" }),
        )
        .await);
    f.ok(f
        .op(&format!("suppliers/{second}/proposal"), json!({}))
        .await);
    f.ok(f
        .w
        .app
        .put(
            &f.url(&format!("suppliers/{second}/decision")),
            &f.manager,
            json!({ "decision": "approved" }),
        )
        .await);
    f.supplier = second;
    let o2 = f.ordered("PO-S2", Some(30)).await;
    f.supplier = first;

    // The second supplier's reply, kept on its own RFQ.
    let eml = "From: sales@osaka-seiki.test\r\nSubject: Re: PO-S2\r\n\
               Content-Type: text/plain; charset=UTF-8\r\n\r\n受領しました。\r\n";
    let r = f
        .op(
            &format!("rfqs/{}/reply-text", o2.rfq),
            json!({ "file_name": "reply.eml", "supplier_id": second,
                    "data_base64": base64::engine::general_purpose::STANDARD.encode(eml) }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let doc = r.body["document_id"].as_str().unwrap().to_string();

    // Filed for the first supplier's order through the API: refused.
    let run = |input: Value| {
        let app = &f.w.app;
        let url = f.url("agents/chasing/run");
        let token = f.w.admin_a.clone();
        async move { app.post(&url, &token, json!({ "input": input })).await }
    };
    let r = run(json!({ "document_id": doc, "order_id": o1.order })).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", r.body);
    // Ids that are not this organisation's: refused.
    let r = run(json!({ "document_id": Uuid::now_v7(), "order_id": o2.order })).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", r.body);
    let r = run(json!({ "document_id": doc, "order_id": Uuid::now_v7() })).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", r.body);

    // Queued some other way, the agent refuses it too: nothing read, nothing proposed.
    {
        let mut tx = f.w.app.pool.begin().await.unwrap();
        procure_server::db::set_org(&mut tx, f.w.org_a)
            .await
            .unwrap();
        procure_server::agents::enqueue(
            &mut tx,
            f.w.org_a,
            "chasing",
            procure_domain::agents::Trigger::Event,
            None,
            json!({ "document_id": doc, "order_id": o1.order }),
            Some("test:mismatch"),
        )
        .await
        .unwrap()
        .unwrap();
        tx.commit().await.unwrap();
    }
    while worker::run_next(&f.w.app.state).await.unwrap().is_some() {}
    let runs = reply_runs(&f).await;
    assert_eq!(runs.len(), 1);
    assert_eq!(
        (runs[0].0.as_str(), runs[0].1.as_deref(), runs[0].2),
        ("failed", Some("supplier_mismatch"), 0)
    );
    assert!(f.open_for(&o1).await.is_empty());

    // Its own order: accepted and read.
    let r = run(json!({ "document_id": doc, "order_id": o2.order })).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.body);
    f.run_all().await;
    assert_eq!(f.open_for(&o2).await.len(), 1);
}
