-- Phase B: invoices Chuo-Sogo issues to its customers (Japanese qualified invoices) and
-- credit notes. Issued documents are immutable in the database itself: only the payment
-- status can change afterwards; corrections are credit notes, never edits.

-- The issuer's details printed on every invoice (one row, owned by the platform organisation).
CREATE TABLE issuer_profile (
    org_id               uuid PRIMARY KEY REFERENCES organisations (id),
    legal_name           text NOT NULL CHECK (length(btrim(legal_name)) BETWEEN 1 AND 200),
    -- 適格請求書発行事業者登録番号: T + 13 digits (check digit verified by the server).
    registration_number  text NOT NULL CHECK (registration_number ~ '^T[0-9]{13}$'),
    address              text NOT NULL CHECK (length(btrim(address)) BETWEEN 1 AND 500),
    phone                text NOT NULL DEFAULT '' CHECK (length(phone) <= 50),
    email                text NOT NULL DEFAULT '' CHECK (length(email) <= 200),
    bank_details         text NOT NULL DEFAULT '' CHECK (length(bank_details) <= 1000),
    payment_terms_days   integer NOT NULL DEFAULT 30 CHECK (payment_terms_days BETWEEN 0 AND 365),
    tax_rounding         text NOT NULL DEFAULT 'floor' CHECK (tax_rounding IN ('floor', 'half_up', 'ceil')),
    -- Invoice numbers: <prefix>-<year>-<sequence>; credit notes <prefix>-CN-<year>-<sequence>.
    number_prefix        text NOT NULL DEFAULT 'CS' CHECK (number_prefix ~ '^[A-Z0-9]{1,8}$'),
    updated_by           uuid NOT NULL REFERENCES users (id),
    updated_at           timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE issued_invoices (
    id                 uuid PRIMARY KEY,
    -- The customer organisation the document is addressed to (tenant of the row).
    org_id             uuid NOT NULL REFERENCES organisations (id),
    kind               text NOT NULL CHECK (kind IN ('invoice', 'credit_note')),
    original_id        uuid,
    status             text NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'issued', 'paid')),
    number             text UNIQUE,
    issue_date         date,
    due_date           date,
    currency           text NOT NULL CHECK (app_currency_ok(currency)),
    recipient_name     text NOT NULL CHECK (length(btrim(recipient_name)) BETWEEN 1 AND 200),
    recipient_address  text NOT NULL DEFAULT '' CHECK (length(recipient_address) <= 500),
    -- Optional link to the procurement request this fee is for.
    request_id         uuid,
    notes              text NOT NULL DEFAULT '' CHECK (length(notes) <= 2000),
    -- Credit notes: why the amount is returned.
    reason             text NOT NULL DEFAULT '' CHECK (length(reason) <= 2000),
    tax_rounding       text NOT NULL CHECK (tax_rounding IN ('floor', 'half_up', 'ceil')),
    -- Computed by the server from the lines (domain::invoicing), stored for listing.
    subtotal_minor     bigint NOT NULL CHECK (subtotal_minor >= 0),
    tax_minor          bigint NOT NULL CHECK (tax_minor >= 0),
    total_minor        bigint NOT NULL CHECK (total_minor >= 0),
    -- [{rate_bps, base_minor, tax_minor}] highest rate first.
    per_rate           jsonb NOT NULL,
    -- Snapshot of issuer_profile at issue time (the printed issuer, even if it changes later).
    issuer             jsonb,
    pdf_sha256         bytea CHECK (pdf_sha256 IS NULL OR length(pdf_sha256) = 32),
    created_by         uuid NOT NULL REFERENCES users (id),
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now(),
    issued_by          uuid REFERENCES users (id),
    issued_at          timestamptz,
    paid_on            date,
    paid_by            uuid REFERENCES users (id),
    paid_note          text NOT NULL DEFAULT '' CHECK (length(paid_note) <= 500),
    UNIQUE (id, org_id),
    FOREIGN KEY (original_id, org_id) REFERENCES issued_invoices (id, org_id),
    FOREIGN KEY (request_id, org_id) REFERENCES procurement_requests (id, org_id),
    CHECK ((kind = 'credit_note') = (original_id IS NOT NULL)),
    CHECK (status = 'draft' OR (number IS NOT NULL AND issue_date IS NOT NULL AND issued_at IS NOT NULL
                                AND issued_by IS NOT NULL AND issuer IS NOT NULL AND pdf_sha256 IS NOT NULL)),
    CHECK ((status = 'paid') = (paid_on IS NOT NULL AND paid_by IS NOT NULL)),
    CHECK (kind = 'invoice' OR status <> 'paid'),
    CHECK (due_date IS NULL OR issue_date IS NULL OR due_date >= issue_date)
);

CREATE TABLE issued_invoice_lines (
    id                uuid PRIMARY KEY,
    org_id            uuid NOT NULL,
    invoice_id        uuid NOT NULL,
    position          integer NOT NULL CHECK (position BETWEEN 1 AND 200),
    description       text NOT NULL CHECK (length(btrim(description)) BETWEEN 1 AND 500),
    quantity          integer NOT NULL CHECK (quantity BETWEEN 1 AND 1000000000),
    unit              text NOT NULL DEFAULT '' CHECK (length(unit) <= 20),
    unit_price_minor  bigint NOT NULL CHECK (unit_price_minor >= 0),
    -- 10 % standard, 8 % reduced (軽減税率, marked ※ on the invoice), 0 % exempt.
    tax_rate_bps      integer NOT NULL CHECK (tax_rate_bps IN (0, 800, 1000)),
    FOREIGN KEY (invoice_id, org_id) REFERENCES issued_invoices (id, org_id) ON DELETE CASCADE,
    UNIQUE (invoice_id, position)
);

-- Gap-free numbering: the counter row is locked and advanced inside the issuing transaction.
CREATE TABLE invoice_counters (
    kind   text NOT NULL CHECK (kind IN ('invoice', 'credit_note')),
    year   integer NOT NULL CHECK (year BETWEEN 2000 AND 9999),
    last   integer NOT NULL CHECK (last >= 1),
    PRIMARY KEY (kind, year)
);

CREATE INDEX issued_invoices_org ON issued_invoices (org_id, created_at DESC);
CREATE INDEX issued_invoices_original ON issued_invoices (original_id);

-- Immutability after issue. Drafts can be edited and deleted; issued documents only move
-- between 'issued' and 'paid' (with the payment fields), nothing else.
CREATE FUNCTION issued_invoice_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    payment text[] := ARRAY['status', 'paid_on', 'paid_by', 'paid_note', 'updated_at'];
BEGIN
    IF TG_OP = 'DELETE' THEN
        IF OLD.status <> 'draft' THEN
            RAISE EXCEPTION 'issued invoice % cannot be deleted', OLD.number USING ERRCODE = 'P0001';
        END IF;
        RETURN OLD;
    END IF;
    IF OLD.status = 'draft' THEN
        RETURN NEW;
    END IF;
    IF NEW.status NOT IN ('issued', 'paid')
       OR (to_jsonb(NEW) - payment) IS DISTINCT FROM (to_jsonb(OLD) - payment) THEN
        RAISE EXCEPTION 'issued invoice % is immutable', OLD.number USING ERRCODE = 'P0001';
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER issued_invoices_guard BEFORE UPDATE OR DELETE ON issued_invoices
    FOR EACH ROW EXECUTE FUNCTION issued_invoice_guard();

-- Lines belong to their draft; once the document is issued they are frozen.
CREATE FUNCTION issued_invoice_line_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    parent uuid := CASE WHEN TG_OP = 'DELETE' THEN OLD.invoice_id ELSE NEW.invoice_id END;
    st text;
BEGIN
    SELECT status INTO st FROM issued_invoices WHERE id = parent;
    -- st IS NULL: the parent draft itself is being deleted (cascade).
    IF st IS NOT NULL AND st <> 'draft' THEN
        RAISE EXCEPTION 'lines of an issued invoice are immutable' USING ERRCODE = 'P0001';
    END IF;
    IF TG_OP = 'UPDATE' AND NEW.invoice_id <> OLD.invoice_id THEN
        RAISE EXCEPTION 'lines cannot move between invoices' USING ERRCODE = 'P0001';
    END IF;
    RETURN CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
END $$;

CREATE TRIGGER issued_invoice_lines_guard BEFORE INSERT OR UPDATE OR DELETE ON issued_invoice_lines
    FOR EACH ROW EXECUTE FUNCTION issued_invoice_line_guard();

ALTER TABLE issuer_profile ENABLE ROW LEVEL SECURITY;
ALTER TABLE issuer_profile FORCE ROW LEVEL SECURITY;
CREATE POLICY issuer_profile_platform ON issuer_profile
    USING (org_id = app_org_id() AND app_platform_scope())
    WITH CHECK (org_id = app_org_id() AND app_platform_scope());

-- Customers see their own issued documents (never drafts); only platform-scoped
-- transactions (admin console, after a ManageBilling check) see drafts or write anything.
ALTER TABLE issued_invoices ENABLE ROW LEVEL SECURITY;
ALTER TABLE issued_invoices FORCE ROW LEVEL SECURITY;
CREATE POLICY issued_invoices_read ON issued_invoices FOR SELECT
    USING (app_platform_scope() OR (org_id = app_org_id() AND status <> 'draft'));
CREATE POLICY issued_invoices_insert ON issued_invoices FOR INSERT
    WITH CHECK (app_platform_scope() AND org_id = app_org_id());
CREATE POLICY issued_invoices_update ON issued_invoices FOR UPDATE
    USING (app_platform_scope() AND org_id = app_org_id())
    WITH CHECK (app_platform_scope() AND org_id = app_org_id());
CREATE POLICY issued_invoices_delete ON issued_invoices FOR DELETE
    USING (app_platform_scope() AND org_id = app_org_id());

ALTER TABLE issued_invoice_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE issued_invoice_lines FORCE ROW LEVEL SECURITY;
CREATE POLICY issued_invoice_lines_read ON issued_invoice_lines FOR SELECT
    USING (app_platform_scope()
           OR (org_id = app_org_id()
               AND invoice_id IN (SELECT id FROM issued_invoices WHERE status <> 'draft')));
CREATE POLICY issued_invoice_lines_write ON issued_invoice_lines FOR ALL
    USING (app_platform_scope() AND org_id = app_org_id())
    WITH CHECK (app_platform_scope() AND org_id = app_org_id());

ALTER TABLE invoice_counters ENABLE ROW LEVEL SECURITY;
ALTER TABLE invoice_counters FORCE ROW LEVEL SECURITY;
CREATE POLICY invoice_counters_platform ON invoice_counters
    USING (app_platform_scope()) WITH CHECK (app_platform_scope());

GRANT SELECT, INSERT, UPDATE ON issuer_profile, invoice_counters TO procure_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE ON issued_invoices, issued_invoice_lines TO procure_runtime;
