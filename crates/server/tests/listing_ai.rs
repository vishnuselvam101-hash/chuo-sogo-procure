//! Phase D acceptance: AI help for listings — typed notes, voice notes (speech-to-text on
//! this server) and catalogue files become suggestions only; drafts in the other language;
//! nothing is saved or published by the AI.

// Test helpers: a failed unwrap is a failed test, which is the intent.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use base64::Engine;
use common::*;
use procure_ai::provider::BoxFuture;
use procure_server::intake::{IntakeError, Transcriber};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

/// Stands in for whisper.cpp: "hears" a fixed narration.
struct FakeEars;

impl Transcriber for FakeEars {
    fn name(&self) -> &str {
        "fake"
    }
    fn transcribe<'a>(
        &'a self,
        _audio: &'a [u8],
        kind: &'a str,
        language: Option<&'a str>,
    ) -> BoxFuture<'a, Result<String, IntakeError>> {
        Box::pin(async move {
            assert_eq!(kind, "wav");
            assert_eq!(language, Some("ja"));
            Ok("メーカー: NSK\n型番: 6204-2RS\n品名: 深溝玉軸受\n状態: 新品\n内径: 20 mm".into())
        })
    }
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

struct Sup {
    w: World,
    org: Uuid,
    token: String,
}

async fn setup(app: App) -> Sup {
    let w = world_with(app).await;
    let r = w
        .app
        .post(
            "/api/v1/platform/organisations",
            &w.super_admin,
            json!({ "name": "大阪ベアリング株式会社", "kind": "supplier",
                    "admin_email": "sales@bearing.test", "admin_name": "営業" }),
        )
        .await;
    let org = uuid(&r.body["org_id"]);
    let token = w.app.activate("sales@bearing.test").await;
    w.app.accept(org, &token).await;
    Sup { w, org, token }
}

impl Sup {
    fn path(&self, rest: &str) -> String {
        if rest.is_empty() {
            format!("/api/v1/orgs/{}/listings", self.org)
        } else {
            format!("/api/v1/orgs/{}/listings/{rest}", self.org)
        }
    }
    async fn post(&self, rest: &str, body: Value) -> Resp {
        self.w.app.post(&self.path(rest), &self.token, body).await
    }
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn typed_notes_and_files_become_suggestions_only(pool: PgPool) {
    let s = setup(App::new(pool).await).await;
    let r = s
        .post(
            "ai/from-text",
            json!({ "text": "型番: HG-KR43\nメーカー: MITSUBISHI\n品名: サーボモータ\n定格出力: 400 W" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["status"], "pending");
    assert_eq!(r.body["input_source"], "typed");
    assert_eq!(r.body["output"]["model_number"], "HG-KR43");
    assert_eq!(r.body["output"]["specs"][0]["value"], "400 W");
    assert_eq!(r.body["provider"], "mock");
    let sid = uuid(&r.body["id"]);
    // Nothing was created: suggestions are not listings.
    assert_eq!(s.w.app.get(&s.path(""), &s.token).await.body, json!([]));
    // Applying is recorded once.
    let d = s
        .post(
            &format!("ai/suggestions/{sid}/decision"),
            json!({ "accepted": true }),
        )
        .await;
    assert_eq!(d.status, StatusCode::OK);
    assert_eq!(d.body["status"], "accepted");
    assert_eq!(
        s.post(
            &format!("ai/suggestions/{sid}/decision"),
            json!({ "accepted": false })
        )
        .await
        .status,
        StatusCode::CONFLICT
    );

    // A catalogue exported from Japanese Excel (Shift_JIS CSV).
    let (csv, _, _) =
        encoding_rs::SHIFT_JIS.encode("型番: 6204-2RS\n品名: 深溝玉軸受\n外径: 47 mm\n");
    let r = s
        .post(
            "ai/from-file",
            json!({ "file_name": "カタログ.csv", "data_base64": b64(&csv) }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["input_source"], "file");
    assert_eq!(r.body["output"]["model_number"], "6204-2RS");
    assert!(
        r.body["input_text"]
            .as_str()
            .unwrap()
            .contains("深溝玉軸受")
    );
    // Executables and empty files are refused before any AI call.
    let r = s
        .post(
            "ai/from-file",
            json!({ "file_name": "setup.exe", "data_base64": b64(b"MZ\x90\0") }),
        )
        .await;
    assert_eq!(r.body["error"]["detail"], "file_type");

    // Drafting the other language for a saved listing.
    let listing = s
        .post(
            "",
            json!({ "condition": "new", "title": "深溝玉軸受 6204", "description": "内径 20 mm",
                    "tax_rate_bps": 1000 }),
        )
        .await;
    let lid = uuid(&listing.body["id"]);
    let t = s.post(&format!("{lid}/ai/translate"), json!({})).await;
    assert_eq!(t.status, StatusCode::CREATED, "{:?}", t.body);
    assert_eq!(t.body["output"]["target"], "en");
    assert!(
        t.body["output"]["flags"]
            .as_array()
            .unwrap()
            .contains(&json!("machine_translation"))
    );

    // Every AI call is in the supplier's audit log with its source.
    let audit =
        s.w.app
            .get(&format!("/api/v1/orgs/{}/audit?limit=50", s.org), &s.token)
            .await;
    let sources: Vec<&str> = audit.body["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["action"] == "ai.suggested")
        .map(|e| e["data"]["source"].as_str().unwrap())
        .collect();
    assert!(sources.contains(&"typed") && sources.contains(&"file"));
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn voice_needs_speech_to_text_on_the_server(pool: PgPool) {
    let s = setup(App::new(pool).await).await;
    let wav = b"RIFF\x24\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0";
    let r = s
        .post(
            "ai/from-voice",
            json!({ "file_name": "memo.wav", "data_base64": b64(wav), "language": "ja" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.body["error"]["detail"], "speech_unavailable");
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn voice_notes_are_transcribed_and_suggested(pool: PgPool) {
    let app = App::configured(pool, |state| {
        state.transcriber = Some(Arc::new(FakeEars));
    })
    .await;
    let s = setup(app).await;
    let wav = b"RIFF\x24\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0";
    let r = s
        .post(
            "ai/from-voice",
            json!({ "file_name": "memo.wav", "data_base64": b64(wav), "language": "ja" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["input_source"], "voice");
    assert!(r.body["input_text"].as_str().unwrap().contains("6204-2RS"));
    assert_eq!(r.body["output"]["manufacturer"], "NSK");
    assert_eq!(r.body["output"]["condition"], "new");
    // Not audio, or an unknown language.
    let r = s
        .post(
            "ai/from-voice",
            json!({ "file_name": "x.wav", "data_base64": b64(b"%PDF-1.7 not audio at all") }),
        )
        .await;
    assert_eq!(r.body["error"]["detail"], "file_type");
    let r = s
        .post(
            "ai/from-voice",
            json!({ "file_name": "x.wav", "data_base64": b64(wav), "language": "fr" }),
        )
        .await;
    assert_eq!(r.body["error"]["detail"], "language");
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn only_the_supplier_and_only_when_allowed(pool: PgPool) {
    let s = setup(App::new(pool).await).await;
    let app = &s.w.app;
    // Customers have no listing assistant.
    assert_eq!(
        app.post(
            &format!("/api/v1/orgs/{}/listings/ai/from-text", s.w.org_a),
            &s.w.admin_a,
            json!({ "text": "型番: 6204-2RS" })
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    // Another company's listing cannot be the subject.
    let r = app
        .post(
            "/api/v1/platform/organisations",
            &s.w.super_admin,
            json!({ "name": "Other", "kind": "supplier", "admin_email": "o@o.test", "admin_name": "O" }),
        )
        .await;
    let other_org = uuid(&r.body["org_id"]);
    let other = app.activate("o@o.test").await;
    app.accept(other_org, &other).await;
    let theirs = uuid(
        &app.post(
            &format!("/api/v1/orgs/{other_org}/listings"),
            &other,
            json!({ "condition": "new", "title": "x", "tax_rate_bps": 1000 }),
        )
        .await
        .body["id"],
    );
    assert_eq!(
        s.post(
            "ai/from-text",
            json!({ "text": "型番: 6204-2RS", "listing_id": theirs })
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.post(&format!("{theirs}/ai/translate"), json!({}))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    // Chuo-Sogo can switch AI off for the supplier.
    let mut tx = s.w.app.pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.org_id', $1, true), set_config('app.user_id', $2, true)")
        .bind(s.org.to_string())
        .bind(s.w.user_id(&s.w.super_admin).await.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO org_settings (org_id, ai_enabled, updated_by) VALUES ($1, false, app_user_id())",
    )
    .bind(s.org)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let r = s
        .post("ai/from-text", json!({ "text": "型番: 6204-2RS" }))
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.body["error"]["detail"], "ai_disabled");
}
