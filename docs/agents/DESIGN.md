# Agent layer — design contract (L1, 2026-11-05)

All lanes (L2–L7) build on this. Changing it needs the main session.

## Principles (from CLAUDE.md, restated for agents)
1. An **agent** is Rust code that runs a unit of routine work for one org and ends with *proposals*, never with a consequential action.
2. Deterministic findings (e.g. a match result) are written as **agent exceptions** with a `proposed_action`.
   Anything a model generated is stored in `ai_suggestions` first; the exception references it (`suggestion_id`).
3. A human **decides** an exception (approve / edit / reject). Only on approve/edit does the server execute the action, through the
   same domain functions and permission checks a human would use, in one transaction with an audit event.
4. Agents never approve recommendations, issue POs, pay, substitute products, or send live email. Outgoing supplier mail is queued in
   `outbox` (test mode unless live mail is configured **and** the org has `auto_send_supplier_mail = true`; default false).
5. Supplier text is untrusted: fenced in prompts, schema-validated output, no tool calls from its content.

## Migration 0023_agents.sql (L1). Lane migrations: L2 0024, L3 0025, L5 0026, L6 0027, L7 0028. L8 UI needs none.
- `agent_settings` (org_id PK, enabled bool default false, auto_send_supplier_mail bool default false,
  monthly_model_call_budget int default 2000, updated_by, updated_at). RLS by org.
- `agent_runs` — the job queue. id uuid v7, org_id, agent text (CHECK list, lanes extend by dropping/re-adding the CHECK),
  trigger text (`schedule` | `event` | `manual`), subject_type text, subject_id uuid NULL, status
  (`queued` | `running` | `succeeded` | `failed` | `skipped`), attempts int, run_after timestamptz, locked_until timestamptz NULL,
  dedupe_key text NULL (unique per org among queued/running), input jsonb, output jsonb, error_code text, model_calls int default 0,
  created_by uuid NULL, created_at, started_at, finished_at. Claimed with `FOR UPDATE SKIP LOCKED` by the worker.
- `agent_exceptions` — the inbox. id, org_id, agent, run_id, kind text, severity (`info` | `warning` | `critical`), subject_type,
  subject_id, request_id NULL (for linking to UI), title_key text (locale key, never prose), params jsonb (values for the title/body),
  proposed_action jsonb NULL (`{"type": "...", ...}`), suggestion_id NULL → ai_suggestions, status (`open` | `approved` | `edited` |
  `rejected` | `resolved_elsewhere`), dedupe_key text (unique per org while open), decided_by, decided_at, decision_note, created_at.
- `ai_suggestions.capability` CHECK widened by each lane that adds a capability.
- RLS ENABLE + FORCE, `org_id = app_org_id()`, staff scope like the other tenant tables; runtime grants minimal (column-level UPDATE).

## Rust
- `crates/domain/src/agents.rs` (pure): `RunStatus`, `ExceptionStatus`, transition fns, `Decision { Approve, Edit(Value), Reject }`,
  budget check `within_budget(used, budget)`, backoff schedule.
- `crates/server/src/agents/mod.rs`: 
  ```rust
  pub trait Agent: Send + Sync {
      fn name(&self) -> &'static str;                          // matches agent_runs.agent
      fn run<'a>(&'a self, cx: &'a AgentCx, run: &'a AgentRun) -> BoxFuture<'a, Result<Outcome, AgentError>>;
      // Executes an approved/edited proposed_action. Default: no action types → Err(UnknownAction).
      fn apply<'a>(&'a self, tx: &'a mut Tx, ex: &'a AgentException, action: &'a Value, actor: &'a Principal)
          -> BoxFuture<'a, Result<(), ApiError>>;
      fn schedule(&self) -> Option<Duration> { None }          // periodic sweep per enabled org
  }
  pub struct Outcome { pub output: Value, pub exceptions: Vec<NewException>, pub model_calls: i32 }
  ```
  `registry()` returns `Vec<Arc<dyn Agent>>`; lanes add one line each. `AgentCx` holds the pool, the AI provider, the org id and a
  tenant transaction helper.
- `agents/queue.rs`: `enqueue(tx, org_id, agent, trigger, subject, input, dedupe_key)`, `claim(pool) -> Option<AgentRun>`,
  `finish`, `fail` (retry with backoff up to 3 attempts). Event hooks call `enqueue` inside the business transaction
  (e.g. `record_invoice` enqueues `matching`).
- `agents/exceptions.rs`: `raise(tx, NewException)` (upsert on dedupe_key, audit `agent.exception_raised`), `decide(...)`
  (audit `agent.exception_decided`, then `agent.apply` for approve/edit).
- `agents/worker.rs`: `spawn(state)` from main.rs: loop claims runs, also enqueues scheduled sweeps for orgs with `enabled`.
  Skips (status `skipped`, error `budget_exhausted`) when the org's monthly model calls ≥ budget.
- An `echo` test agent exists only under `#[cfg(test)]`.

## API (`crates/server/src/api/agents.rs`)
- `GET/PUT /api/v1/orgs/{org_id}/agents/settings` (ManageAgents to PUT, ViewAgents to GET)
- `GET /api/v1/orgs/{org_id}/agents/runs?agent=&status=` (ViewAgents)
- `POST /api/v1/orgs/{org_id}/agents/{agent}/run` manual trigger (ManageAgents)
- `GET /api/v1/orgs/{org_id}/agents/exceptions?status=open` (ViewAgents)
- `POST /api/v1/orgs/{org_id}/agents/exceptions/{id}/decide` `{decision, action?, note?}` (DecideAgentExceptions + the permission the
  action itself needs, checked inside `apply`)
- `GET /api/v1/orgs/{org_id}/agents/summary` counts for the console (runs by status, open exceptions by severity, model calls this month)
- `GET /api/admin/v1/ops/agents/exceptions` staff view across assigned orgs.
- New permissions in `domain::rbac`: `ViewAgents`, `ManageAgents`, `DecideAgentExceptions`; mapped to roles alongside the closest existing
  permissions (owner/admin manage; buyers/finance decide; viewers view). Staff (operators) may decide for assigned orgs.

## Lanes
| Lane | Agents (name) | Capabilities added | Action types |
|---|---|---|---|
| L2 | `matching` | `extract_invoice` | `resolve_discrepancy`, `request_credit_note` (outbox draft) |
| L3 | `chasing` | `parse_supplier_reply` | `send_chase` (queue outbox), `confirm_delivery_date`, `record_promised_date` |
| L5 | `cost_analyst` | `negotiation_brief` | `save_brief`, `open_negotiation_task` |
| L6 | `supplier_watch` | — | `request_document_update`, `flag_supplier_review` |
| L7 | `import` (runs connector jobs) | — | `apply_import` |
Each lane: domain logic + tests in `crates/domain`, server module `crates/server/src/agents/<name>.rs`, its own API routes if needed,
locale keys for titles in `apps/desktop/locales/{ja,en}/agents.json` (ja is primary; add its own top-level key `<lane>` to avoid conflicts),
`#[sqlx::test]` tests incl. tenant denial. No UI screens (L8 does them).
