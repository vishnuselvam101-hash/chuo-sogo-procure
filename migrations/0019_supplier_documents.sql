-- Electronic transaction records (電子帳簿保存法). What a supplier sent (an email, PDF,
-- spreadsheet or FAX scan of its quotation) is kept as received, with its SHA-256, and can
-- never be changed or deleted: the runtime role may only add and read these rows. The
-- records register (api/records.rs) finds quotations, orders and invoices by date, amount
-- and counterparty, and links each quotation to the file it was read from.

CREATE TABLE supplier_documents (
    id            uuid PRIMARY KEY,
    org_id        uuid NOT NULL,
    rfq_id        uuid NOT NULL,
    supplier_id   uuid REFERENCES suppliers (id),
    -- How it arrived: uploaded by staff, or read from the mailbox (email, or FAX via mail).
    channel       text NOT NULL DEFAULT 'upload' CHECK (channel IN ('upload', 'email', 'fax')),
    file_name     text NOT NULL CHECK (length(btrim(file_name)) BETWEEN 1 AND 200),
    content_type  text NOT NULL CHECK (length(content_type) BETWEEN 1 AND 100),
    size_bytes    bigint NOT NULL CHECK (size_bytes BETWEEN 1 AND 10485760),
    sha256        bytea NOT NULL CHECK (length(sha256) = 32),
    received_at   timestamptz NOT NULL DEFAULT now(),
    uploaded_by   uuid REFERENCES users (id),
    created_at    timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (rfq_id, org_id) REFERENCES rfqs (id, org_id),
    UNIQUE (id, org_id)
);
CREATE INDEX supplier_documents_rfq ON supplier_documents (rfq_id, received_at);

ALTER TABLE supplier_documents ENABLE ROW LEVEL SECURITY;
ALTER TABLE supplier_documents FORCE ROW LEVEL SECURITY;
CREATE POLICY supplier_documents_tenant ON supplier_documents
    USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id());
GRANT SELECT, INSERT ON supplier_documents TO procure_runtime;

ALTER TABLE quotations ADD COLUMN source_document_id uuid;
ALTER TABLE quotations ADD FOREIGN KEY (source_document_id, org_id)
    REFERENCES supplier_documents (id, org_id);
