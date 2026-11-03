//! Test harness: in-process router, real PostgreSQL (one fresh database per test
//! via `#[sqlx::test]`), in-memory test-mode mailer.

#![allow(dead_code)]
// Test-only harness: a failed unwrap is a failed test, which is the intent.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use procure_server::mailer::MemoryMailer;
use procure_server::{AppState, bootstrap, router};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

/// The invoice PDF font, read once per test binary (`fonts-noto-cjk` must be installed).
fn pdf_font() -> Option<procure_server::invoice_pdf::PdfFont> {
    static FONT: std::sync::OnceLock<procure_server::invoice_pdf::PdfFont> =
        std::sync::OnceLock::new();
    Some(
        FONT.get_or_init(|| {
            procure_server::invoice_pdf::PdfFont::load(
                procure_server::invoice_pdf::DEFAULT_FONT_PATH,
                0,
            )
            .unwrap()
        })
        .clone(),
    )
}

pub const PASSWORD: &str = "correct-horse-battery-staple";

pub struct App {
    pub router: Router,
    pub pool: PgPool,
    pub mailer: Arc<MemoryMailer>,
    /// The application state the router runs with (for driving background jobs).
    pub state: AppState,
}

pub struct Resp {
    pub status: StatusCode,
    pub body: Value,
    pub request_id: Option<String>,
}

impl Resp {
    pub fn code(&self) -> &str {
        self.body["error"]["code"].as_str().unwrap_or("")
    }
}

impl App {
    /// `owner_pool` is the per-test database created by `#[sqlx::test]` (owner role).
    /// The app under test runs as the restricted runtime role, exactly like production.
    pub async fn new(owner_pool: PgPool) -> Self {
        Self::with_ai(owner_pool, Arc::new(procure_ai::mock::MockProvider)).await
    }

    /// Same, with a chosen AI provider (e.g. a scripted adversarial one).
    pub async fn with_ai(owner_pool: PgPool, ai: Arc<dyn procure_ai::Provider>) -> Self {
        Self::configured(owner_pool, move |s| s.ai = ai).await
    }

    /// Same, with any change to the application state (e.g. admin network allow-list).
    pub async fn configured(owner_pool: PgPool, configure: impl FnOnce(&mut AppState)) -> Self {
        let options = (*owner_pool.connect_options()).clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(10)
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::Executor::execute(conn, "SET ROLE procure_runtime")
                        .await
                        .map(|_| ())
                })
            })
            .connect_with(options)
            .await
            .unwrap();
        let mailer = Arc::new(MemoryMailer::default());
        let mut state = AppState::new(pool.clone(), mailer.clone());
        // Admin console: a fixed test key, and no network restriction (in-process requests
        // have no peer address); a dedicated test checks the restriction.
        state.secret_box = procure_server::mfa::SecretBox::from_hex(&"42".repeat(32)).map(Arc::new);
        state.admin_networks = None;
        state.pdf_font = pdf_font();
        configure(&mut state);
        // Content-addressed, so tests running in parallel can share one directory.
        state.storage = Arc::new(procure_server::storage::Storage::new(
            std::env::temp_dir().join("procure-test-storage"),
        ));
        let router = router(state.clone());
        App {
            router,
            pool,
            mailer,
            state,
        }
    }

    pub async fn call(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Resp {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let res = self.router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let request_id = res
            .headers()
            .get("x-request-id")
            .map(|v| v.to_str().unwrap().to_string());
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        Resp {
            status,
            body,
            request_id,
        }
    }

    /// GET returning the raw body (files): (status, content type, bytes).
    pub async fn get_raw(&self, uri: &str, token: &str) -> (StatusCode, String, Vec<u8>) {
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = self.router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let ct = res
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default();
        let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
        (status, ct, bytes)
    }

    pub async fn get(&self, uri: &str, token: &str) -> Resp {
        self.call(Method::GET, uri, Some(token), None).await
    }

    pub async fn post(&self, uri: &str, token: &str, body: Value) -> Resp {
        self.call(Method::POST, uri, Some(token), Some(body)).await
    }

    pub async fn patch(&self, uri: &str, token: &str, body: Value) -> Resp {
        self.call(Method::PATCH, uri, Some(token), Some(body)).await
    }

    pub async fn put(&self, uri: &str, token: &str, body: Value) -> Resp {
        self.call(Method::PUT, uri, Some(token), Some(body)).await
    }

    pub async fn delete(&self, uri: &str, token: &str) -> Resp {
        self.call(Method::DELETE, uri, Some(token), None).await
    }

    /// Latest token mailed to `email` (test-mode mailer).
    pub fn has_mail(&self, email: &str) -> bool {
        self.mailer
            .sent
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.to == email)
    }

    pub fn last_token(&self, email: &str) -> String {
        let sent = self.mailer.sent.lock().unwrap();
        let mail = sent
            .iter()
            .rev()
            .find(|m| m.to == email)
            .expect("no mail for address");
        mail.vars
            .iter()
            .find(|(k, _)| *k == "token")
            .unwrap()
            .1
            .clone()
    }

    pub async fn set_password(&self, token: &str, password: &str) -> Resp {
        self.call(
            Method::POST,
            "/api/v1/auth/set-password",
            None,
            Some(json!({ "token": token, "password": password })),
        )
        .await
    }

    pub async fn login(&self, email: &str, password: &str) -> Resp {
        self.call(
            Method::POST,
            "/api/v1/auth/login",
            None,
            Some(json!({ "email": email, "password": password })),
        )
        .await
    }

    pub async fn accept(&self, org: Uuid, session: &str) {
        let r = self
            .call(
                Method::POST,
                &format!("/api/v1/invitations/{org}/accept"),
                Some(session),
                None,
            )
            .await;
        assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
    }

    /// Activates via the mailed token and logs in. Returns the session token.
    pub async fn activate(&self, email: &str) -> String {
        let t = self.last_token(email);
        assert_eq!(
            self.set_password(&t, PASSWORD).await.status,
            StatusCode::NO_CONTENT
        );
        let r = self.login(email, PASSWORD).await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
        r.body["token"].as_str().unwrap().to_string()
    }
}

pub struct World {
    pub app: App,
    pub platform_org: Uuid,
    pub super_admin: String,
    pub org_a: Uuid,
    pub admin_a: String,
    pub org_b: Uuid,
    pub admin_b: String,
}

pub fn uuid(v: &Value) -> Uuid {
    v.as_str().unwrap().parse().unwrap()
}

/// Platform + two customer orgs (A, B), each with an active admin.
pub async fn world(pool: PgPool) -> World {
    world_with(App::new(pool).await).await
}

pub async fn world_with(app: App) -> World {
    let token = bootstrap(
        &app.pool,
        "Chuo-Sogo Inc.",
        "root@chuo-sogo.test",
        "Root Admin",
    )
    .await
    .unwrap();
    assert_eq!(
        app.set_password(&token, PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );
    let login = app.login("root@chuo-sogo.test", PASSWORD).await;
    let super_admin = login.body["token"].as_str().unwrap().to_string();
    let platform_org = uuid(&login.body["memberships"][0]["org_id"]);

    let mut orgs = vec![];
    for (name, email) in [
        ("株式会社エー製作所", "admin@a.test"),
        ("B Industries", "admin@b.test"),
    ] {
        let r = app
            .post(
                "/api/v1/platform/organisations",
                &super_admin,
                json!({ "name": name, "admin_email": email, "admin_name": "Admin" }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let org = uuid(&r.body["org_id"]);
        let session = app.activate(email).await;
        app.accept(org, &session).await;
        orgs.push((org, session));
    }
    let (org_b, admin_b) = orgs.pop().unwrap();
    let (org_a, admin_a) = orgs.pop().unwrap();
    World {
        app,
        platform_org,
        super_admin,
        org_a,
        admin_a,
        org_b,
        admin_b,
    }
}

impl World {
    /// Adds a member to `org` (as `admin`) and returns their session token.
    pub async fn member(&self, org: Uuid, admin: &str, email: &str, role: &str) -> String {
        let r = self
            .app
            .post(
                &format!("/api/v1/orgs/{org}/members"),
                admin,
                json!({ "email": email, "display_name": email, "role": role }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let session = self.app.activate(email).await;
        self.app.accept(org, &session).await;
        session
    }

    pub async fn user_id(&self, token: &str) -> Uuid {
        uuid(&self.app.get("/api/v1/me", token).await.body["user"]["id"])
    }
}
