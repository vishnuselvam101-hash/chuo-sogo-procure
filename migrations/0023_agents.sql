-- Agent layer L1 (docs/agents/DESIGN.md, 2026-11-05): the runtime that later lanes build on.
-- An agent does routine work for one organisation and ends with proposals (exceptions with a
-- proposed action); a person decides each one, and only then does the server act, through
-- the same checks a person's own action goes through. Off by default for every organisation.

-- Per-organisation switch, budget and mail rule. No row = defaults (off).
CREATE TABLE agent_settings (
    org_id                      uuid PRIMARY KEY REFERENCES organisations (id),
    enabled                     boolean NOT NULL DEFAULT false,
    -- Supplier mail an approved action queues is still test mode unless live mail is
    -- configured on the server AND this is on.
    auto_send_supplier_mail     boolean NOT NULL DEFAULT false,
    monthly_model_call_budget   integer NOT NULL DEFAULT 2000
                                CHECK (monthly_model_call_budget BETWEEN 0 AND 1000000),
    updated_by                  uuid REFERENCES users (id),
    updated_at                  timestamptz NOT NULL DEFAULT now()
);

-- The job queue. Lanes add their agent names by dropping and re-adding agent_runs_agent_check
-- and agent_exceptions_agent_check. `echo` is the runtime's test agent: it is in no
-- production registry, so nothing can start it outside the test suite.
CREATE TABLE agent_runs (
    id              uuid PRIMARY KEY,
    org_id          uuid NOT NULL REFERENCES organisations (id),
    agent           text NOT NULL CONSTRAINT agent_runs_agent_check CHECK (agent IN (
                        'echo', 'matching', 'chasing', 'cost_analyst', 'supplier_watch', 'import')),
    trigger         text NOT NULL CHECK (trigger IN ('schedule', 'event', 'manual')),
    subject_type    text CHECK (subject_type IS NULL OR subject_type ~ '^[a-z_]{1,40}$'),
    subject_id      uuid,
    status          text NOT NULL DEFAULT 'queued'
                    CHECK (status IN ('queued', 'running', 'succeeded', 'failed', 'skipped')),
    attempts        integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    run_after       timestamptz NOT NULL DEFAULT now(),
    -- Lease of the worker running it; a run whose lease ran out is claimed again.
    locked_until    timestamptz,
    dedupe_key      text CHECK (dedupe_key IS NULL OR length(dedupe_key) BETWEEN 1 AND 200),
    input           jsonb NOT NULL DEFAULT '{}'::jsonb,
    output          jsonb,
    error_code      text CHECK (error_code IS NULL OR length(error_code) <= 100),
    model_calls     integer NOT NULL DEFAULT 0 CHECK (model_calls >= 0),
    created_by      uuid REFERENCES users (id),
    created_at      timestamptz NOT NULL DEFAULT now(),
    started_at      timestamptz,
    finished_at     timestamptz,
    UNIQUE (id, org_id),
    CHECK ((status IN ('succeeded', 'failed', 'skipped')) = (finished_at IS NOT NULL)),
    CHECK (status <> 'running' OR locked_until IS NOT NULL),
    CHECK (pg_column_size(input) <= 65536),
    CHECK (output IS NULL OR pg_column_size(output) <= 262144)
);
-- At most one waiting or running run per key.
CREATE UNIQUE INDEX agent_runs_dedupe ON agent_runs (org_id, dedupe_key)
    WHERE dedupe_key IS NOT NULL AND status IN ('queued', 'running');
CREATE INDEX agent_runs_due ON agent_runs (org_id, run_after) WHERE status = 'queued';
CREATE INDEX agent_runs_list ON agent_runs (org_id, created_at DESC);

-- Tenant-checked references to suggestions.
ALTER TABLE ai_suggestions ADD CONSTRAINT ai_suggestions_id_org UNIQUE (id, org_id);

-- The inbox: what agents found and propose, waiting for a person.
CREATE TABLE agent_exceptions (
    id              uuid PRIMARY KEY,
    org_id          uuid NOT NULL REFERENCES organisations (id),
    agent           text NOT NULL CONSTRAINT agent_exceptions_agent_check CHECK (agent IN (
                        'echo', 'matching', 'chasing', 'cost_analyst', 'supplier_watch', 'import')),
    run_id          uuid,
    kind            text NOT NULL CHECK (kind ~ '^[a-z_]{1,60}$'),
    severity        text NOT NULL CHECK (severity IN ('info', 'warning', 'critical')),
    subject_type    text CHECK (subject_type IS NULL OR subject_type ~ '^[a-z_]{1,40}$'),
    subject_id      uuid,
    request_id      uuid,
    -- A locale key, never prose; `params` holds the values the text needs.
    title_key       text NOT NULL CHECK (title_key ~ '^[a-z][A-Za-z0-9_.]{0,119}$'),
    params          jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(params) = 'object'),
    proposed_action jsonb CHECK (proposed_action IS NULL
                                 OR (jsonb_typeof(proposed_action) = 'object'
                                     AND jsonb_typeof(proposed_action -> 'type') = 'string')),
    suggestion_id   uuid,
    status          text NOT NULL DEFAULT 'open'
                    CHECK (status IN ('open', 'approved', 'edited', 'rejected', 'resolved_elsewhere')),
    dedupe_key      text NOT NULL CHECK (length(dedupe_key) BETWEEN 1 AND 200),
    decided_by      uuid REFERENCES users (id),
    decided_at      timestamptz,
    -- The action that was executed (the proposal, or the person's edit of it).
    decided_action  jsonb,
    decision_note   text NOT NULL DEFAULT '' CHECK (length(decision_note) <= 2000),
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (run_id, org_id) REFERENCES agent_runs (id, org_id),
    FOREIGN KEY (request_id, org_id) REFERENCES procurement_requests (id, org_id),
    FOREIGN KEY (suggestion_id, org_id) REFERENCES ai_suggestions (id, org_id),
    CHECK ((status = 'open') = (decided_at IS NULL)),
    CHECK (status NOT IN ('approved', 'edited', 'rejected') OR decided_by IS NOT NULL),
    CHECK (pg_column_size(params) <= 16384),
    CHECK (proposed_action IS NULL OR pg_column_size(proposed_action) <= 16384)
);
-- One open exception per key; a later run with the same finding refreshes it.
CREATE UNIQUE INDEX agent_exceptions_dedupe ON agent_exceptions (org_id, dedupe_key)
    WHERE status = 'open';
CREATE INDEX agent_exceptions_list ON agent_exceptions (org_id, status, created_at DESC);

ALTER TABLE agent_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent_settings FORCE ROW LEVEL SECURITY;
CREATE POLICY agent_settings_tenant ON agent_settings
    USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id());
-- The background worker reads which organisations have agents on (platform scope only).
CREATE POLICY agent_settings_platform_read ON agent_settings FOR SELECT
    USING (app_platform_scope());

ALTER TABLE agent_runs ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent_runs FORCE ROW LEVEL SECURITY;
CREATE POLICY agent_runs_tenant ON agent_runs
    USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id());

ALTER TABLE agent_exceptions ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent_exceptions FORCE ROW LEVEL SECURITY;
CREATE POLICY agent_exceptions_tenant ON agent_exceptions
    USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id());

GRANT SELECT, INSERT ON agent_settings, agent_runs, agent_exceptions TO procure_runtime;
GRANT UPDATE (enabled, auto_send_supplier_mail, monthly_model_call_budget, updated_by, updated_at)
    ON agent_settings TO procure_runtime;
-- What a run was asked to do (agent, subject, input) never changes.
GRANT UPDATE (status, attempts, run_after, locked_until, output, error_code, model_calls,
              started_at, finished_at)
    ON agent_runs TO procure_runtime;
-- What it is about (agent, kind, subject, key) never changes.
GRANT UPDATE (run_id, severity, title_key, params, proposed_action, suggestion_id, status,
              decided_by, decided_at, decided_action, decision_note, updated_at)
    ON agent_exceptions TO procure_runtime;
