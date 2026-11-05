-- Agent layer L6 (docs/agents/DESIGN.md): supplier lifecycle. Compliance documents with
-- expiry kept by our staff in the platform-level supplier directory, and the `supplier_watch`
-- agent that proposes asking a supplier for a renewed document or flagging a supplier for
-- review. A person decides; the customer's own supplier approval is never changed by it.
--
-- Scorecards (on-time delivery, discrepancy rate, RFQ responsiveness, price rank) are
-- computed on read from the customer's own records (procure_domain::scorecard,
-- agents/supplier_watch.rs), not stored: every input is already a record of fact in the
-- customer's tenant tables, a month's volume is small, and a stored snapshot would go stale
-- whenever a delivery is reversed or a late invoice is recorded. No table is needed, so
-- there is nothing to keep in step and nothing extra for RLS to protect.

-- One document per row (renewals are new rows; `retired_at` hides superseded ones).
-- Platform-level like `suppliers`: written only by our staff. Customers see a document only
-- when `customer_visible` and the supplier is proposed to them or invited to their RFQs,
-- the same rule as for the supplier itself. Internal notes live in a staff-only table.
CREATE TABLE supplier_compliance_docs (
    id                uuid PRIMARY KEY,
    supplier_id       uuid NOT NULL REFERENCES suppliers (id),
    kind              text NOT NULL CHECK (kind IN ('iso9001', 'iso14001', 'iatf16949',
                          'qualified_invoice_registration', 'anti_social_check', 'insurance',
                          'nda', 'other')),
    -- Certificate / policy number; for qualified_invoice_registration the 適格請求書発行事業者
    -- 登録番号 (T + 13 digits, check digit verified by procure_domain::compliance).
    reference         text NOT NULL DEFAULT '' CHECK (length(reference) <= 100),
    title             text NOT NULL DEFAULT '' CHECK (length(title) <= 200),
    issued_on         date,
    -- Valid through this date (inclusive); NULL = does not expire.
    expires_on        date CHECK (expires_on IS NULL OR issued_on IS NULL OR expires_on >= issued_on),
    customer_visible  boolean NOT NULL DEFAULT false,
    -- The file, in the content-addressed attachment storage (same type allowlist).
    file_name         text CHECK (file_name IS NULL OR length(btrim(file_name)) BETWEEN 1 AND 200),
    content_type      text CHECK (content_type IS NULL OR content_type IN (
        'application/pdf', 'image/png', 'image/jpeg', 'image/webp', 'text/plain', 'text/csv',
        'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
        'application/vnd.openxmlformats-officedocument.wordprocessingml.document')),
    size_bytes        bigint CHECK (size_bytes IS NULL OR size_bytes BETWEEN 1 AND 10485760),
    sha256            bytea CHECK (sha256 IS NULL OR length(sha256) = 32),
    verified_by       uuid REFERENCES users (id),
    verified_at       timestamptz,
    retired_at        timestamptz,
    created_by        uuid NOT NULL REFERENCES users (id),
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    CHECK ((verified_by IS NULL) = (verified_at IS NULL)),
    CHECK ((file_name IS NULL) = (sha256 IS NULL)
           AND (file_name IS NULL) = (content_type IS NULL)
           AND (file_name IS NULL) = (size_bytes IS NULL)),
    CHECK (kind <> 'qualified_invoice_registration' OR reference ~ '^T[0-9]{13}$')
);
CREATE INDEX supplier_compliance_docs_supplier ON supplier_compliance_docs (supplier_id, kind);
CREATE INDEX supplier_compliance_docs_expiry ON supplier_compliance_docs (expires_on)
    WHERE retired_at IS NULL AND expires_on IS NOT NULL;

ALTER TABLE supplier_compliance_docs ENABLE ROW LEVEL SECURITY;
ALTER TABLE supplier_compliance_docs FORCE ROW LEVEL SECURITY;
CREATE POLICY supplier_compliance_docs_visible ON supplier_compliance_docs
    USING (app_staff()
           OR (customer_visible AND retired_at IS NULL AND (
                  supplier_id IN (SELECT a.supplier_id FROM customer_supplier_approvals a
                                  WHERE a.org_id = app_org_id())
               OR supplier_id IN (SELECT r.supplier_id FROM rfq_recipients r
                                  WHERE r.org_id = app_org_id()))))
    WITH CHECK (app_staff());

-- Internal notes (e.g. how an anti-social-forces check was done): staff only, by RLS.
CREATE TABLE supplier_compliance_doc_notes (
    doc_id  uuid PRIMARY KEY REFERENCES supplier_compliance_docs (id),
    notes   text NOT NULL DEFAULT '' CHECK (length(notes) <= 4000)
);
ALTER TABLE supplier_compliance_doc_notes ENABLE ROW LEVEL SECURITY;
ALTER TABLE supplier_compliance_doc_notes FORCE ROW LEVEL SECURITY;
CREATE POLICY supplier_compliance_doc_notes_staff ON supplier_compliance_doc_notes
    USING (app_staff()) WITH CHECK (app_staff());

GRANT SELECT, INSERT ON supplier_compliance_docs, supplier_compliance_doc_notes TO procure_runtime;
-- What the file is and who added it never changes.
GRANT UPDATE (reference, title, issued_on, expires_on, customer_visible, verified_by,
              verified_at, retired_at, updated_at)
    ON supplier_compliance_docs TO procure_runtime;
GRANT UPDATE (notes) ON supplier_compliance_doc_notes TO procure_runtime;

-- An approved `request_document_update` writes a draft to the supplier into the outbox
-- (test mode: recorded, never sent by the server). It belongs to no RFQ.
ALTER TABLE outbox ALTER COLUMN rfq_recipient_id DROP NOT NULL;
ALTER TABLE outbox
    ADD COLUMN supplier_id uuid REFERENCES suppliers (id),
    ADD COLUMN compliance_doc_id uuid REFERENCES supplier_compliance_docs (id);
ALTER TABLE outbox
    ADD CONSTRAINT outbox_document_request_shape CHECK (
        (kind = 'document_request') = (compliance_doc_id IS NOT NULL)
        AND (kind <> 'document_request' OR (supplier_id IS NOT NULL AND channel = 'test')));
ALTER TABLE outbox
    ADD CONSTRAINT outbox_recipient_present CHECK (
        rfq_recipient_id IS NOT NULL OR kind = 'document_request');
-- Adds 'document_request' to the allowed kinds, keeping whatever other lanes added before
-- (lanes migrate in parallel, so the list is extended rather than restated).
DO $$
DECLARE kinds text[];
BEGIN
    SELECT array_agg(DISTINCT m[1]) INTO kinds
    FROM pg_constraint c,
         regexp_matches(pg_get_constraintdef(c.oid), '''([a-z_]+)''', 'g') AS m
    WHERE c.conrelid = 'outbox'::regclass AND c.conname = 'outbox_kind_check';
    kinds := ARRAY(SELECT DISTINCT k FROM unnest(kinds || ARRAY['document_request']) AS k ORDER BY k);
    ALTER TABLE outbox DROP CONSTRAINT outbox_kind_check;
    EXECUTE format('ALTER TABLE outbox ADD CONSTRAINT outbox_kind_check CHECK (kind = ANY (%L::text[]))',
                   kinds);
END $$;
CREATE INDEX outbox_compliance_doc ON outbox (compliance_doc_id) WHERE compliance_doc_id IS NOT NULL;
