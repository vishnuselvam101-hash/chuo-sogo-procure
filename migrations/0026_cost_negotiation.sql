-- Agent layer L5 (docs/agents/DESIGN.md): should-cost models and negotiation briefs.
-- A cost model says what one unit of an item ought to cost (material, processes, overhead,
-- logistics, margin; procure_domain::should_cost). A negotiation brief collects the
-- deterministic facts about one supplier's quotation; an AI narrative may be drafted from
-- those facts only (ai_suggestions), and a person writes and finalises the brief.

-- Versioned per item: a change is a new version, earlier versions stay as they were.
CREATE TABLE cost_models (
    id               uuid PRIMARY KEY,
    org_id           uuid NOT NULL REFERENCES organisations (id),
    -- procure_domain::history::item_key (normalised part number or description).
    item_key         text NOT NULL CHECK (item_key ~ '^[md]:.+' AND length(item_key) <= 1100),
    version          integer NOT NULL CHECK (version >= 1),
    name             text NOT NULL DEFAULT '' CHECK (length(name) <= 200),
    currency         text NOT NULL CHECK (app_currency_ok(currency)),
    -- Validated in Rust (api::cost::ModelInputs) before it is stored.
    inputs           jsonb NOT NULL CHECK (jsonb_typeof(inputs) = 'object'
                                           AND pg_column_size(inputs) <= 65536),
    -- The unit should-cost computed from `inputs` when saved (thousandths of a minor unit).
    unit_cost_milli  bigint NOT NULL CHECK (unit_cost_milli >= 0),
    notes            text NOT NULL DEFAULT '' CHECK (length(notes) <= 2000),
    created_by       uuid NOT NULL REFERENCES users (id),
    created_at       timestamptz NOT NULL DEFAULT now(),
    UNIQUE (org_id, item_key, version),
    UNIQUE (id, org_id)
);
CREATE INDEX cost_models_key ON cost_models (org_id, item_key, version DESC);

CREATE TABLE negotiation_briefs (
    id                  uuid PRIMARY KEY,
    org_id              uuid NOT NULL REFERENCES organisations (id),
    request_id          uuid NOT NULL,
    rfq_id              uuid NOT NULL,
    supplier_id         uuid NOT NULL REFERENCES suppliers (id),
    quotation_id        uuid NOT NULL,
    -- The agent exception whose approval saved it, if any.
    exception_id        uuid REFERENCES agent_exceptions (id),
    currency            text NOT NULL CHECK (app_currency_ok(currency)),
    -- Deterministic facts (api::cost::QuotationFacts), refreshed while a draft.
    facts               jsonb NOT NULL CHECK (jsonb_typeof(facts) = 'object'
                                              AND pg_column_size(facts) <= 262144),
    quoted_total_minor  bigint NOT NULL CHECK (quoted_total_minor >= 0),
    -- The latest AI narrative drafted from the facts (a pending suggestion until finalised).
    suggestion_id       uuid,
    -- What a person wrote (possibly from the suggestion).
    summary             text NOT NULL DEFAULT '' CHECK (length(summary) <= 4000),
    arguments           jsonb NOT NULL DEFAULT '[]'::jsonb
                        CHECK (jsonb_typeof(arguments) = 'array' AND pg_column_size(arguments) <= 16384),
    target_price_minor  bigint CHECK (target_price_minor >= 0),
    status              text NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'final')),
    created_by          uuid NOT NULL REFERENCES users (id),
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    finalised_by        uuid REFERENCES users (id),
    finalised_at        timestamptz,
    FOREIGN KEY (request_id, org_id) REFERENCES procurement_requests (id, org_id),
    FOREIGN KEY (rfq_id, org_id) REFERENCES rfqs (id, org_id),
    FOREIGN KEY (quotation_id, org_id) REFERENCES quotations (id, org_id),
    FOREIGN KEY (suggestion_id, org_id) REFERENCES ai_suggestions (id, org_id),
    UNIQUE (id, org_id),
    CHECK ((status = 'final') = (finalised_at IS NOT NULL)),
    CHECK ((status = 'final') = (finalised_by IS NOT NULL)),
    -- A target is never above what was quoted.
    CHECK (target_price_minor IS NULL OR target_price_minor <= quoted_total_minor)
);
-- One draft per supplier and RFQ; final briefs are kept.
CREATE UNIQUE INDEX negotiation_briefs_one_draft ON negotiation_briefs (rfq_id, supplier_id)
    WHERE status = 'draft';
CREATE INDEX negotiation_briefs_list ON negotiation_briefs (org_id, created_at DESC);

-- A final brief is a record: it never changes again.
CREATE FUNCTION negotiation_briefs_final_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.status = 'final' THEN
        RAISE EXCEPTION 'negotiation brief % is final', OLD.id USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER negotiation_briefs_final_guard BEFORE UPDATE ON negotiation_briefs
    FOR EACH ROW EXECUTE FUNCTION negotiation_briefs_final_guard();

ALTER TABLE cost_models ENABLE ROW LEVEL SECURITY;
ALTER TABLE cost_models FORCE ROW LEVEL SECURITY;
CREATE POLICY cost_models_tenant ON cost_models
    USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id());

ALTER TABLE negotiation_briefs ENABLE ROW LEVEL SECURITY;
ALTER TABLE negotiation_briefs FORCE ROW LEVEL SECURITY;
CREATE POLICY negotiation_briefs_tenant ON negotiation_briefs
    USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id());

-- Versions are never edited or deleted.
GRANT SELECT, INSERT ON cost_models TO procure_runtime;
GRANT SELECT, INSERT ON negotiation_briefs TO procure_runtime;
GRANT UPDATE (quotation_id, facts, quoted_total_minor, suggestion_id, summary, arguments,
              target_price_minor, status, updated_at, finalised_by, finalised_at, exception_id)
    ON negotiation_briefs TO procure_runtime;

-- The narrative capability. Listed with every lane's capability (DESIGN.md) so the order in
-- which lane migrations run does not matter.
ALTER TABLE ai_suggestions DROP CONSTRAINT ai_suggestions_capability_check;
ALTER TABLE ai_suggestions ADD CONSTRAINT ai_suggestions_capability_check
    CHECK (capability IN ('extract_items', 'extract_quotation', 'extract_listing',
                          'translate_listing', 'extract_invoice', 'parse_supplier_reply',
                          'negotiation_brief'));

-- Staff task for an approved negotiation. Appended to whatever kinds the check allows now,
-- so kinds added by other lanes' migrations are kept.
DO $$
DECLARE def text;
BEGIN
    SELECT pg_get_constraintdef(oid) INTO def FROM pg_constraint
     WHERE conrelid = 'tasks'::regclass AND conname = 'tasks_kind_check';
    IF def IS NULL OR position('''negotiate''' IN def) > 0 THEN
        RETURN;
    END IF;
    -- e.g. CHECK ((kind = ANY (ARRAY['identify'::text, ...])))
    IF def !~ '\]\)\)\)$' THEN
        RAISE EXCEPTION 'unexpected tasks_kind_check: %', def;
    END IF;
    EXECUTE 'ALTER TABLE tasks DROP CONSTRAINT tasks_kind_check';
    EXECUTE 'ALTER TABLE tasks ADD CONSTRAINT tasks_kind_check '
         || regexp_replace(def, '\]\)\)\)$', ', ''negotiate''::text])))');
END $$;
