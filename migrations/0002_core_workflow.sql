-- Phase 2 core workflow: requests and items, supplier directory and customer decisions,
-- RFQs (test-mode dispatch only), quotations, comparison runs, recommendations,
-- approval policies and approval records.
--
-- Child tables reference their parent by (id, org_id) so a row can never point at another
-- tenant's parent, independently of RLS.

-- Staff scope: set only after the server verified the caller holds a platform-org role.
CREATE FUNCTION app_staff() RETURNS boolean LANGUAGE sql STABLE AS
$$ SELECT coalesce(current_setting('app.staff', true), '') = 'on' $$;

CREATE FUNCTION app_currency_ok(c text) RETURNS boolean LANGUAGE sql IMMUTABLE AS
$$ SELECT c IN ('JPY', 'USD', 'EUR', 'INR', 'CNY', 'KRW') $$;

-- Human-readable numbers (PR-2026-000001), per organisation, kind and year.
CREATE TABLE org_counters (
    org_id  uuid NOT NULL REFERENCES organisations (id),
    kind    text NOT NULL CHECK (kind IN ('PR', 'RFQ')),
    year    integer NOT NULL,
    last    integer NOT NULL CHECK (last > 0),
    PRIMARY KEY (org_id, kind, year)
);

CREATE FUNCTION next_number(p_org uuid, p_kind text) RETURNS text LANGUAGE sql AS $$
    INSERT INTO org_counters AS c (org_id, kind, year, last)
    VALUES (p_org, p_kind,
            extract(year FROM now() AT TIME ZONE (SELECT timezone FROM organisations WHERE id = p_org))::int, 1)
    ON CONFLICT (org_id, kind, year) DO UPDATE SET last = c.last + 1
    RETURNING p_kind || '-' || year || '-' || lpad(last::text, 6, '0')
$$;

CREATE TABLE procurement_requests (
    id                  uuid PRIMARY KEY,
    org_id              uuid NOT NULL REFERENCES organisations (id),
    number              text NOT NULL,
    title               text NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 200),
    description         text NOT NULL DEFAULT '' CHECK (length(description) <= 4000),
    currency            text NOT NULL CHECK (app_currency_ok(currency)),
    needed_by           date,
    delivery_location   text NOT NULL DEFAULT '' CHECK (length(delivery_location) <= 500),
    status              text NOT NULL DEFAULT 'draft' CHECK (status IN (
        'draft', 'submitted', 'needs_clarification', 'identifying', 'sourcing', 'rfq_sent',
        'quotes_received', 'under_comparison', 'recommended', 'awaiting_approval', 'approved',
        'rejected', 'ordered', 'delivered', 'invoiced', 'disputed', 'reconciled', 'closed', 'cancelled')),
    -- Latest open question from the operator or approver, and the customer's latest answer.
    clarification_question text CHECK (length(clarification_question) <= 2000),
    clarification_answer   text CHECK (length(clarification_answer) <= 2000),
    requested_by        uuid NOT NULL REFERENCES users (id),
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    submitted_at        timestamptz,
    UNIQUE (org_id, number),
    UNIQUE (id, org_id)
);
CREATE INDEX procurement_requests_org_status ON procurement_requests (org_id, status);

CREATE TABLE request_items (
    id                     uuid PRIMARY KEY,
    org_id                 uuid NOT NULL,
    request_id             uuid NOT NULL,
    line_no                integer NOT NULL CHECK (line_no > 0),
    description            text NOT NULL CHECK (length(btrim(description)) BETWEEN 1 AND 1000),
    manufacturer           text NOT NULL DEFAULT '' CHECK (length(manufacturer) <= 200),
    mpn                    text NOT NULL DEFAULT '' CHECK (length(mpn) <= 100),
    -- Whole units in v1 (u32 range in the domain).
    quantity               integer NOT NULL CHECK (quantity BETWEEN 1 AND 1000000000),
    unit                   text NOT NULL DEFAULT 'pcs' CHECK (unit ~ '^[a-z_]{1,16}$'),
    notes                  text NOT NULL DEFAULT '' CHECK (length(notes) <= 2000),
    -- Product identity confirmed by a human operator, with the evidence they relied on.
    verification_status    text NOT NULL DEFAULT 'unverified'
                           CHECK (verification_status IN ('unverified', 'verified')),
    verified_manufacturer  text CHECK (length(verified_manufacturer) <= 200),
    verified_mpn           text CHECK (length(verified_mpn) <= 100),
    verification_evidence  text CHECK (length(verification_evidence) <= 2000),
    verified_by            uuid REFERENCES users (id),
    verified_at            timestamptz,
    created_at             timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (request_id, org_id) REFERENCES procurement_requests (id, org_id),
    UNIQUE (request_id, line_no),
    UNIQUE (id, org_id),
    CHECK (verification_status <> 'verified' OR (
        length(btrim(verified_mpn)) > 0 AND length(btrim(verification_evidence)) > 0
        AND verified_by IS NOT NULL AND verified_at IS NOT NULL))
);

-- Chuo-Sogo's supplier directory (platform-level, maintained by internal staff).
CREATE TABLE suppliers (
    id          uuid PRIMARY KEY,
    name        text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    country     text NOT NULL CHECK (country ~ '^[A-Z]{2}$'),
    email       citext CHECK (email IS NULL OR (length(email) <= 254 AND email LIKE '%_@_%')),
    website     text CHECK (website IS NULL OR (length(website) <= 300 AND website ~ '^https?://')),
    categories  text[] NOT NULL DEFAULT '{}' CHECK (cardinality(categories) <= 20),
    status      text NOT NULL DEFAULT 'discovered'
                CHECK (status IN ('discovered', 'contacted', 'under_review', 'verified', 'inactive')),
    created_by  uuid NOT NULL REFERENCES users (id),
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);

-- Internal notes live apart from the supplier row so that RLS, not just column lists,
-- keeps them from customer transactions.
CREATE TABLE supplier_notes (
    supplier_id  uuid PRIMARY KEY REFERENCES suppliers (id),
    notes        text NOT NULL DEFAULT '' CHECK (length(notes) <= 4000)
);
ALTER TABLE supplier_notes ENABLE ROW LEVEL SECURITY;
ALTER TABLE supplier_notes FORCE ROW LEVEL SECURITY;
CREATE POLICY supplier_notes_staff ON supplier_notes USING (app_staff()) WITH CHECK (app_staff());

-- Per-customer decision on a directory supplier. Staff propose; the customer decides.
CREATE TABLE customer_supplier_approvals (
    org_id       uuid NOT NULL REFERENCES organisations (id),
    supplier_id  uuid NOT NULL REFERENCES suppliers (id),
    status       text NOT NULL DEFAULT 'proposed' CHECK (status IN ('proposed', 'approved', 'rejected')),
    reason       text NOT NULL DEFAULT '' CHECK (length(reason) <= 2000),
    proposed_by  uuid NOT NULL REFERENCES users (id),
    decided_by   uuid REFERENCES users (id),
    decided_at   timestamptz,
    PRIMARY KEY (org_id, supplier_id),
    CHECK ((status = 'proposed') = (decided_by IS NULL))
);

CREATE TABLE rfqs (
    id            uuid PRIMARY KEY,
    org_id        uuid NOT NULL,
    request_id    uuid NOT NULL,
    number        text NOT NULL,
    status        text NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'sent', 'closed')),
    response_due  date,
    message       text NOT NULL DEFAULT '' CHECK (length(message) <= 4000),
    created_by    uuid NOT NULL REFERENCES users (id),
    created_at    timestamptz NOT NULL DEFAULT now(),
    sent_at       timestamptz,
    FOREIGN KEY (request_id, org_id) REFERENCES procurement_requests (id, org_id),
    UNIQUE (org_id, number),
    UNIQUE (id, org_id),
    CHECK ((status = 'draft') = (sent_at IS NULL))
);

CREATE TABLE rfq_recipients (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL,
    rfq_id       uuid NOT NULL,
    supplier_id  uuid NOT NULL REFERENCES suppliers (id),
    status       text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'exported', 'responded', 'declined')),
    FOREIGN KEY (rfq_id, org_id) REFERENCES rfqs (id, org_id),
    UNIQUE (rfq_id, supplier_id),
    UNIQUE (id, org_id)
);

-- Outgoing supplier messages. Only the 'test' channel exists: messages are recorded and
-- exported for manual handling; nothing is ever sent from the server (ARCHITECTURE D9).
CREATE TABLE outbox (
    id                uuid PRIMARY KEY,
    org_id            uuid NOT NULL,
    rfq_recipient_id  uuid NOT NULL,
    channel           text NOT NULL CHECK (channel = 'test'),
    to_address        text,
    subject           text NOT NULL CHECK (length(subject) <= 300),
    body              text NOT NULL CHECK (length(body) <= 20000),
    created_at        timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (rfq_recipient_id, org_id) REFERENCES rfq_recipients (id, org_id)
);

CREATE TABLE quotations (
    id                 uuid PRIMARY KEY,
    org_id             uuid NOT NULL,
    rfq_id             uuid NOT NULL,
    rfq_recipient_id   uuid NOT NULL,
    supplier_reference text NOT NULL DEFAULT '' CHECK (length(supplier_reference) <= 100),
    currency           text NOT NULL CHECK (app_currency_ok(currency)),
    shipping_minor     bigint NOT NULL DEFAULT 0 CHECK (shipping_minor >= 0),
    valid_until        date,
    received_at        timestamptz NOT NULL,
    notes              text NOT NULL DEFAULT '' CHECK (length(notes) <= 2000),
    recorded_by        uuid NOT NULL REFERENCES users (id),
    created_at         timestamptz NOT NULL DEFAULT now(),
    -- A revised quotation from the same supplier replaces the previous one.
    superseded_at      timestamptz,
    FOREIGN KEY (rfq_id, org_id) REFERENCES rfqs (id, org_id),
    FOREIGN KEY (rfq_recipient_id, org_id) REFERENCES rfq_recipients (id, org_id),
    UNIQUE (id, org_id)
);
CREATE UNIQUE INDEX quotations_current ON quotations (rfq_recipient_id) WHERE superseded_at IS NULL;

CREATE TABLE quotation_lines (
    id                    uuid PRIMARY KEY,
    org_id                uuid NOT NULL,
    quotation_id          uuid NOT NULL,
    request_item_id       uuid NOT NULL,
    unit_price_minor      bigint NOT NULL CHECK (unit_price_minor >= 0),
    quantity              integer NOT NULL CHECK (quantity BETWEEN 1 AND 1000000000),
    lead_time_days        integer CHECK (lead_time_days BETWEEN 0 AND 3650),
    offered_manufacturer  text NOT NULL DEFAULT '' CHECK (length(offered_manufacturer) <= 200),
    offered_mpn           text NOT NULL DEFAULT '' CHECK (length(offered_mpn) <= 100),
    alternative           boolean NOT NULL DEFAULT false,
    FOREIGN KEY (quotation_id, org_id) REFERENCES quotations (id, org_id),
    FOREIGN KEY (request_item_id, org_id) REFERENCES request_items (id, org_id)
);
CREATE INDEX quotation_lines_quotation ON quotation_lines (quotation_id);

-- Immutable snapshot of a deterministic comparison (procure_domain::comparison).
CREATE TABLE comparison_runs (
    id        uuid PRIMARY KEY,
    org_id    uuid NOT NULL,
    rfq_id    uuid NOT NULL,
    currency  text NOT NULL CHECK (app_currency_ok(currency)),
    as_of     date NOT NULL,
    -- The exact current quotations compared; a recommendation is stale if this set changed.
    quotation_ids uuid[] NOT NULL,
    results   jsonb NOT NULL,
    ran_by    uuid NOT NULL REFERENCES users (id),
    ran_at    timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (rfq_id, org_id) REFERENCES rfqs (id, org_id),
    UNIQUE (id, org_id)
);

CREATE TABLE approval_policies (
    org_id               uuid NOT NULL REFERENCES organisations (id),
    currency             text NOT NULL CHECK (app_currency_ok(currency)),
    -- [{"min_minor": 100000, "approvals": 2}, ...], validated by procure_domain::approval.
    tiers                jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(tiers) = 'array'),
    allow_self_approval  boolean NOT NULL DEFAULT false,
    version              integer NOT NULL DEFAULT 1 CHECK (version > 0),
    updated_by           uuid NOT NULL REFERENCES users (id),
    updated_at           timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, currency)
);

CREATE TABLE recommendations (
    id                   uuid PRIMARY KEY,
    org_id               uuid NOT NULL,
    request_id           uuid NOT NULL,
    comparison_run_id    uuid NOT NULL,
    quotation_id         uuid NOT NULL,
    currency             text NOT NULL CHECK (app_currency_ok(currency)),
    total_minor          bigint NOT NULL CHECK (total_minor >= 0),
    rank                 integer NOT NULL CHECK (rank > 0),
    reason               text NOT NULL DEFAULT '' CHECK (length(reason) <= 2000),
    status               text NOT NULL DEFAULT 'recommended' CHECK (status IN (
        'recommended', 'awaiting_approval', 'approved', 'rejected', 'clarification_requested',
        'withdrawn')),
    recommended_by       uuid NOT NULL REFERENCES users (id),
    created_at           timestamptz NOT NULL DEFAULT now(),
    -- Policy snapshot taken when sent, so later policy edits never change an open decision.
    required_approvals   integer CHECK (required_approvals BETWEEN 1 AND 5),
    allow_self_approval  boolean,
    policy_version       integer,
    sent_at              timestamptz,
    decided_at           timestamptz,
    FOREIGN KEY (request_id, org_id) REFERENCES procurement_requests (id, org_id),
    FOREIGN KEY (comparison_run_id, org_id) REFERENCES comparison_runs (id, org_id),
    FOREIGN KEY (quotation_id, org_id) REFERENCES quotations (id, org_id),
    UNIQUE (id, org_id),
    CHECK (status IN ('recommended', 'withdrawn') OR (required_approvals IS NOT NULL AND sent_at IS NOT NULL))
);
CREATE UNIQUE INDEX recommendations_open ON recommendations (request_id)
    WHERE status IN ('recommended', 'awaiting_approval');

-- Human decisions only. Append-only for the runtime role.
CREATE TABLE approval_records (
    id                 uuid PRIMARY KEY,
    org_id             uuid NOT NULL,
    recommendation_id  uuid NOT NULL,
    decided_by         uuid NOT NULL REFERENCES users (id),
    decision           text NOT NULL CHECK (decision IN ('approve', 'reject', 'clarify')),
    comment            text NOT NULL DEFAULT '' CHECK (length(comment) <= 2000),
    created_at         timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (recommendation_id, org_id) REFERENCES recommendations (id, org_id),
    UNIQUE (recommendation_id, decided_by),
    CHECK (decision = 'approve' OR length(btrim(comment)) > 0)
);

-- Row-level security: every org-scoped table is limited to the org in context.
DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['org_counters', 'procurement_requests', 'request_items',
                             'customer_supplier_approvals', 'rfqs', 'rfq_recipients', 'outbox',
                             'quotations', 'quotation_lines', 'comparison_runs',
                             'approval_policies', 'recommendations', 'approval_records'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format('CREATE POLICY %I ON %I USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id())',
                       t || '_tenant', t);
    END LOOP;
END $$;

-- The directory is visible to staff; a customer sees only suppliers proposed to it or
-- invited to its RFQs. Only staff write.
ALTER TABLE suppliers ENABLE ROW LEVEL SECURITY;
ALTER TABLE suppliers FORCE ROW LEVEL SECURITY;
CREATE POLICY suppliers_visible ON suppliers
    USING (app_staff()
           OR id IN (SELECT a.supplier_id FROM customer_supplier_approvals a WHERE a.org_id = app_org_id())
           OR id IN (SELECT r.supplier_id FROM rfq_recipients r WHERE r.org_id = app_org_id()))
    WITH CHECK (app_staff());

GRANT SELECT, INSERT, UPDATE ON org_counters, procurement_requests, suppliers, supplier_notes,
    customer_supplier_approvals, rfqs, rfq_recipients, quotations, approval_policies, recommendations
    TO procure_runtime;
-- Draft items can be removed by their owner.
GRANT SELECT, INSERT, UPDATE, DELETE ON request_items TO procure_runtime;
-- Records of what happened: never changed by the application.
GRANT SELECT, INSERT ON outbox, quotation_lines, comparison_runs, approval_records TO procure_runtime;
