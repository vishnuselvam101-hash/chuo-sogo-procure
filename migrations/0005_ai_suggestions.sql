-- Phase 3: AI suggestions. Everything an AI capability produces lands here first, with its
-- provenance; a human accepts (promotes) or rejects it. Nothing else is ever written by AI.

CREATE TABLE ai_suggestions (
    id              uuid PRIMARY KEY,
    org_id          uuid NOT NULL REFERENCES organisations (id),
    capability      text NOT NULL CHECK (capability IN ('extract_items', 'extract_quotation')),
    subject_type    text NOT NULL CHECK (subject_type IN ('procurement_request', 'rfq')),
    subject_id      uuid NOT NULL,
    -- For quotation extraction: the supplier the text came from.
    supplier_id     uuid REFERENCES suppliers (id),
    status          text NOT NULL CHECK (status IN ('pending', 'accepted', 'rejected', 'failed')),
    -- The untrusted text the suggestion was made from (kept for review and audit).
    input_text      text NOT NULL CHECK (length(input_text) <= 20000),
    input_sha256    text NOT NULL CHECK (input_sha256 ~ '^[0-9a-f]{64}$'),
    -- Validated output (procure_ai::extract types); NULL when the call failed.
    output          jsonb,
    error_code      text,
    provider        text NOT NULL,
    model           text NOT NULL,
    prompt_version  text NOT NULL,
    prompt_sha256   text CHECK (prompt_sha256 IS NULL OR prompt_sha256 ~ '^[0-9a-f]{64}$'),
    latency_ms      bigint CHECK (latency_ms >= 0),
    requested_by    uuid NOT NULL REFERENCES users (id),
    created_at      timestamptz NOT NULL DEFAULT now(),
    decided_by      uuid REFERENCES users (id),
    decided_at      timestamptz,
    decision_note   text NOT NULL DEFAULT '' CHECK (length(decision_note) <= 2000),
    CHECK ((status = 'failed') = (output IS NULL)),
    CHECK ((status IN ('accepted', 'rejected')) = (decided_by IS NOT NULL))
);
CREATE INDEX ai_suggestions_subject ON ai_suggestions (subject_id, created_at);

ALTER TABLE ai_suggestions ENABLE ROW LEVEL SECURITY;
ALTER TABLE ai_suggestions FORCE ROW LEVEL SECURITY;
CREATE POLICY ai_suggestions_tenant ON ai_suggestions
    USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id());

GRANT SELECT, INSERT, UPDATE ON ai_suggestions TO procure_runtime;

-- Provenance on promoted records.
ALTER TABLE request_items ADD COLUMN ai_suggestion_id uuid REFERENCES ai_suggestions (id);
ALTER TABLE quotations ADD COLUMN ai_suggestion_id uuid REFERENCES ai_suggestions (id);
