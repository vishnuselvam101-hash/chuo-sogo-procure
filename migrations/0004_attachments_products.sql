-- Phase 2c: request attachments (files stored outside the database, content-addressed) and
-- Chuo-Sogo's product identity records with evidence.

-- Files attached to a procurement request (optionally to one item). The bytes live in the
-- server's storage directory under their SHA-256; the row is the only way to reach them.
CREATE TABLE attachments (
    id            uuid PRIMARY KEY,
    org_id        uuid NOT NULL,
    request_id    uuid NOT NULL,
    item_id       uuid,
    file_name     text NOT NULL CHECK (length(btrim(file_name)) BETWEEN 1 AND 200),
    content_type  text NOT NULL CHECK (content_type IN (
        'application/pdf', 'image/png', 'image/jpeg', 'image/webp', 'text/plain', 'text/csv',
        'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
        'application/vnd.openxmlformats-officedocument.wordprocessingml.document')),
    size_bytes    bigint NOT NULL CHECK (size_bytes BETWEEN 1 AND 10485760),
    sha256        bytea NOT NULL CHECK (length(sha256) = 32),
    uploaded_by   uuid NOT NULL REFERENCES users (id),
    created_at    timestamptz NOT NULL DEFAULT now(),
    -- Removal hides the attachment; the record (and audit trail) stays.
    removed_at    timestamptz,
    removed_by    uuid REFERENCES users (id),
    FOREIGN KEY (request_id, org_id) REFERENCES procurement_requests (id, org_id),
    FOREIGN KEY (item_id, org_id) REFERENCES request_items (id, org_id),
    CHECK ((removed_at IS NULL) = (removed_by IS NULL))
);
CREATE INDEX attachments_request ON attachments (request_id) WHERE removed_at IS NULL;

ALTER TABLE attachments ENABLE ROW LEVEL SECURITY;
ALTER TABLE attachments FORCE ROW LEVEL SECURITY;
CREATE POLICY attachments_tenant ON attachments
    USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id());

-- Platform-level product knowledge maintained by Chuo-Sogo staff (like the supplier
-- directory). Only public facts and public sources belong here; customer documents stay
-- in the customer's attachments.
CREATE TABLE product_identities (
    id            uuid PRIMARY KEY,
    manufacturer  text NOT NULL CHECK (length(btrim(manufacturer)) BETWEEN 1 AND 200),
    mpn           text NOT NULL CHECK (length(btrim(mpn)) BETWEEN 1 AND 100),
    description   text NOT NULL DEFAULT '' CHECK (length(description) <= 1000),
    category      text NOT NULL DEFAULT '' CHECK (length(category) <= 100),
    -- Flat string key/value specifications, e.g. {"bore_mm": "20", "seal": "ZZ"}.
    specs         jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(specs) = 'object'),
    status        text NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'verified', 'retired')),
    created_by    uuid NOT NULL REFERENCES users (id),
    created_at    timestamptz NOT NULL DEFAULT now(),
    verified_by   uuid REFERENCES users (id),
    verified_at   timestamptz,
    CHECK (status <> 'verified' OR (verified_by IS NOT NULL AND verified_at IS NOT NULL))
);
-- One record per manufacturer part number (case-insensitive, trimmed).
CREATE UNIQUE INDEX product_identities_mpn
    ON product_identities (lower(btrim(manufacturer)), lower(btrim(mpn)));

-- Append-only evidence for a product identity. Only URL evidence (public sources) is ever
-- shown to customers; notes and supplier confirmations stay internal.
CREATE TABLE product_evidence (
    id          uuid PRIMARY KEY,
    product_id  uuid NOT NULL REFERENCES product_identities (id),
    kind        text NOT NULL CHECK (kind IN ('datasheet', 'catalogue', 'manufacturer_page',
                                              'supplier_confirmation', 'other')),
    url         text CHECK (url IS NULL OR (length(url) <= 500 AND url ~ '^https://')),
    note        text NOT NULL DEFAULT '' CHECK (length(note) <= 2000),
    added_by    uuid NOT NULL REFERENCES users (id),
    created_at  timestamptz NOT NULL DEFAULT now(),
    CHECK (url IS NOT NULL OR length(btrim(note)) > 0)
);
CREATE INDEX product_evidence_product ON product_evidence (product_id);

ALTER TABLE request_items ADD COLUMN product_identity_id uuid REFERENCES product_identities (id);
-- A verified item is always backed by a product record. Items verified before this
-- migration (free-text evidence only) go back to unverified so the rule holds for all rows.
UPDATE request_items SET verification_status = 'unverified', verified_manufacturer = NULL,
       verified_mpn = NULL, verification_evidence = NULL, verified_by = NULL, verified_at = NULL
 WHERE verification_status = 'verified';
ALTER TABLE request_items ADD CONSTRAINT request_items_verified_product
    CHECK (verification_status <> 'verified' OR product_identity_id IS NOT NULL);

-- Staff see the whole catalogue; a customer sees only the records its own items point to.
ALTER TABLE product_identities ENABLE ROW LEVEL SECURITY;
ALTER TABLE product_identities FORCE ROW LEVEL SECURITY;
CREATE POLICY product_identities_visible ON product_identities
    USING (app_staff()
           OR id IN (SELECT i.product_identity_id FROM request_items i WHERE i.org_id = app_org_id()))
    WITH CHECK (app_staff());

ALTER TABLE product_evidence ENABLE ROW LEVEL SECURITY;
ALTER TABLE product_evidence FORCE ROW LEVEL SECURITY;
CREATE POLICY product_evidence_visible ON product_evidence
    USING (app_staff()
           OR product_id IN (SELECT i.product_identity_id FROM request_items i WHERE i.org_id = app_org_id()))
    WITH CHECK (app_staff());

GRANT SELECT, INSERT, UPDATE ON attachments, product_identities TO procure_runtime;
GRANT SELECT, INSERT ON product_evidence TO procure_runtime;
