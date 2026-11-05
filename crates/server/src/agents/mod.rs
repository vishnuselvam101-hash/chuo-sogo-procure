//! Agent runtime (docs/agents/DESIGN.md, L1). An agent is Rust code that does a unit of
//! routine work for one organisation and ends with *proposals*: exceptions in the inbox,
//! each with an optional proposed action. Nothing consequential happens until a person
//! approves (or edits) the proposal; the server then executes it through [`Agent::apply`],
//! inside the decision's transaction, with the actor's own permissions.
//!
//! - [`queue`]: `agent_runs`, the job queue (enqueue, claim, finish, retry with backoff).
//! - [`exceptions`]: `agent_exceptions`, the inbox (raise with dedupe, decide).
//! - [`worker`]: the background loop that claims and runs queued work and schedules sweeps.

pub mod exceptions;
pub mod queue;
pub mod worker;

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use futures_util::future::BoxFuture;
use procure_ai::provider::{AiError, ModelCall, Provider};
use procure_domain::rbac::{self, Access, Permission};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::AppState;
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiResult};
use crate::principal::Principal;

pub use exceptions::{AgentException, NewException, decide, raise, resolve_elsewhere};
pub use queue::{AgentRun, enqueue};

/// The agents this server runs. Lanes add one line each, e.g.
/// `Arc::new(matching::Matching),`. Test agents are never listed here; tests put their own
/// registry into `AppState::agents`.
pub fn registry() -> Vec<Arc<dyn Agent>> {
    vec![]
}

/// The agent called `name` in this server's registry.
pub fn find(state: &AppState, name: &str) -> Option<Arc<dyn Agent>> {
    state.agents.iter().find(|a| a.name() == name).cloned()
}

/// What a finished run produced. `exceptions` are raised in the transaction that marks the
/// run succeeded. `model_calls` counts calls not made through [`AgentCx::call_model`]
/// (which counts its own); the run records the larger of the two.
#[derive(Debug, Default)]
pub struct Outcome {
    pub output: Value,
    pub exceptions: Vec<NewException>,
    pub model_calls: i32,
}

impl Outcome {
    pub fn new(output: Value) -> Self {
        Outcome {
            output,
            ..Default::default()
        }
    }
}

/// Why a run did not finish. `Retry` and transport errors are tried again with backoff (up
/// to `procure_domain::agents::MAX_ATTEMPTS`); `Fatal` ends the run at once.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("retry: {0}")]
    Retry(&'static str),
    #[error("fatal: {0}")]
    Fatal(&'static str),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Ai(#[from] AiError),
}

impl AgentError {
    /// Stored in `agent_runs.error_code`.
    pub fn code(&self) -> &'static str {
        match self {
            AgentError::Retry(c) | AgentError::Fatal(c) => c,
            AgentError::Db(_) => "database_error",
            AgentError::Ai(e) => e.code(),
        }
    }

    pub fn retryable(&self) -> bool {
        match self {
            AgentError::Retry(_) | AgentError::Db(_) => true,
            AgentError::Fatal(_) => false,
            AgentError::Ai(e) => e.retryable(),
        }
    }
}

/// The person deciding an exception, with their access to its organisation. `apply`
/// checks the permission its action needs with [`Actor::require`].
pub struct Actor<'a> {
    pub principal: &'a Principal,
    pub access: Access,
    pub state: &'a AppState,
}

impl Actor<'_> {
    pub fn require(&self, perm: Permission) -> ApiResult<()> {
        if rbac::allows(self.access, perm) {
            Ok(())
        } else {
            Err(ApiError::Forbidden)
        }
    }
}

pub trait Agent: Send + Sync {
    /// Matches `agent_runs.agent` and `agent_exceptions.agent`.
    fn name(&self) -> &'static str;

    /// Does the work of one run. Reads through `cx.tx()`; writes nothing consequential;
    /// returns what it found as exceptions.
    fn run<'a>(
        &'a self,
        cx: &'a AgentCx,
        run: &'a AgentRun,
    ) -> BoxFuture<'a, Result<Outcome, AgentError>>;

    /// Executes an approved or edited `proposed_action` inside the decision's transaction
    /// (already in the exception's organisation). Must check the action's own permission
    /// with `actor.require(..)` and audit what it changes. Default: no action types.
    fn apply<'a>(
        &'a self,
        tx: &'a mut Tx,
        ex: &'a AgentException,
        action: &'a Value,
        actor: &'a Actor<'a>,
    ) -> BoxFuture<'a, Result<(), ApiError>> {
        let _ = (tx, ex, action, actor);
        Box::pin(async { Err(ApiError::Validation("unknown_action")) })
    }

    /// Periodic sweep for every organisation with agents enabled.
    fn schedule(&self) -> Option<Duration> {
        None
    }
}

/// What a run may use: the database, the AI provider, the organisation it runs for.
pub struct AgentCx {
    pub pool: PgPool,
    pub ai: Arc<dyn Provider>,
    pub org_id: Uuid,
    pub run_id: Uuid,
    /// The whole application state (e.g. `supplier_mail`, `ai_permits`).
    pub state: AppState,
    model_calls: AtomicI32,
}

impl AgentCx {
    pub fn new(state: &AppState, org_id: Uuid, run_id: Uuid) -> Self {
        AgentCx {
            pool: state.pool.clone(),
            ai: state.ai.clone(),
            org_id,
            run_id,
            state: state.clone(),
            model_calls: AtomicI32::new(0),
        }
    }

    /// A transaction in this organisation's context (RLS limits it to this tenant). No user
    /// is set: the agent acts as the system.
    pub async fn tx(&self) -> Result<Tx, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        db::set_org(&mut tx, self.org_id).await?;
        Ok(tx)
    }

    /// Same, also seeing staff-only rows of this organisation (tasks, internal notes).
    pub async fn staff_tx(&self) -> Result<Tx, sqlx::Error> {
        let mut tx = self.tx().await?;
        db::enable_staff_scope(&mut tx).await?;
        Ok(tx)
    }

    /// One validated model call (bounded by the server's AI permits), counted against the
    /// organisation's monthly budget.
    pub async fn call_model<T>(
        &self,
        call: &ModelCall,
        timeout: Duration,
        retries: u32,
        parse: impl Fn(&str) -> Result<T, AiError>,
    ) -> Result<T, AiError> {
        let _permit = self
            .state
            .ai_permits
            .acquire()
            .await
            .map_err(|_| AiError::Unavailable)?;
        self.model_calls.fetch_add(1, Ordering::Relaxed);
        procure_ai::provider::call_validated(self.ai.as_ref(), call, timeout, retries, parse).await
    }

    /// Model calls made through [`AgentCx::call_model`] so far.
    pub fn model_calls(&self) -> i32 {
        self.model_calls.load(Ordering::Relaxed)
    }
}
