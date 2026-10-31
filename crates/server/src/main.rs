//! procure-server
//!   procure-server serve                       run the API (default)
//!   procure-server migrate                     apply database migrations
//!   procure-server openapi                     print the OpenAPI document
//!   procure-server bootstrap <platform-name> <admin-email> <admin-name>
//!
//! Configuration (environment): DATABASE_URL (owner role: migrate/bootstrap),
//! PROCURE_RUNTIME_DATABASE_URL (restricted role: serve), PROCURE_BIND (default 127.0.0.1:8088),
//! PROCURE_OUTBOX_DIR (test-mode mail, default ./var/outbox), RUST_LOG, PROCURE_LOG_FORMAT=json.

use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;

use procure_server::mailer::FileMailer;
use procure_server::{AppState, MIGRATOR, bootstrap, router};
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("procure_server=info,tower_http=info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if std::env::var("PROCURE_LOG_FORMAT").as_deref() == Ok("json") {
        builder.json().init();
    } else {
        builder.init();
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Only used to pick a subcommand; argv[0] (the untrustworthy part) is skipped.
    // nosemgrep: rust.lang.security.args.args
    let args: Vec<String> = std::env::args().skip(1).collect();
    async fn connect(var: &str) -> Result<sqlx::PgPool, Box<dyn std::error::Error>> {
        let url = std::env::var(var).map_err(|_| format!("{var} is not set"))?;
        Ok(PgPoolOptions::new()
            .max_connections(20)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(&url)
            .await?)
    }

    match args.first().map(String::as_str) {
        // API contract; needs no database.
        Some("openapi") => {
            print!("{}", procure_server::openapi::json()?);
        }
        // Schema changes run as the owner role (DATABASE_URL) ...
        Some("migrate") => {
            MIGRATOR.run(&connect("DATABASE_URL").await?).await?;
            tracing::info!("migrations applied");
        }
        Some("bootstrap") => {
            let [_, platform, email, name] = args.as_slice() else {
                return Err(
                    "usage: procure-server bootstrap <platform-name> <admin-email> <admin-name>"
                        .into(),
                );
            };
            let pool = connect("DATABASE_URL").await?;
            MIGRATOR.run(&pool).await?;
            let token = bootstrap(&pool, platform, email, name).await?;
            // Printed once for the operator; it is stored only as a hash.
            println!("Platform created. Activation token for {email} (valid 7 days):\n{token}");
        }
        // ... the server itself runs as the restricted runtime role and never migrates.
        None | Some("serve") => {
            let pool = connect("PROCURE_RUNTIME_DATABASE_URL").await?;
            let applied: Option<i64> =
                sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success")
                    .fetch_one(&pool)
                    .await?;
            let expected = MIGRATOR.iter().map(|m| m.version).max();
            if applied != expected {
                return Err(format!(
                    "database schema is at {applied:?}, this build needs {expected:?}: run `procure-server migrate`"
                )
                .into());
            }
            let outbox =
                std::env::var("PROCURE_OUTBOX_DIR").unwrap_or_else(|_| "./var/outbox".into());
            tracing::warn!(dir = %outbox, "mail is in TEST MODE: messages are written to files, nothing is sent");
            let state = AppState::new(pool, Arc::new(FileMailer { dir: outbox.into() }));
            let bind = std::env::var("PROCURE_BIND").unwrap_or_else(|_| "127.0.0.1:8088".into());
            let listener = tokio::net::TcpListener::bind(&bind).await?;
            tracing::info!(%bind, "listening");
            axum::serve(
                listener,
                router(state).into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await?;
        }
        Some(other) => return Err(format!("unknown command: {other}").into()),
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "fatal");
            ExitCode::FAILURE
        }
    }
}
