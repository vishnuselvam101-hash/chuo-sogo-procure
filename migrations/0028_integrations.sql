-- Agent layer L7 (docs/agents/DESIGN.md, docs/agents/INTEGRATIONS.md): the factory's purchase
-- orders, goods receipts and supplier invoices come in as files exported from its ERP or
-- accounting system. A file is read into a batch (preview: nothing else changes); only when a
-- person applies the batch (directly, or by approving the `import` agent's proposal) are the
-- records written, through the same server code as manual entry, all or nothing.

-- Named column-mapping profiles, saved per organisation (built-in presets live in code).
CREATE TABLE integration_profiles (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES organisations (id),
    name        text NOT NULL CHECK (name ~ '^[a-z0-9_]{1,60}$'),
    kind        text NOT NULL CHECK (kind IN ('po', 'receipt', 'invoice')),
    -- procure_server::api::integrations::ProfileConfig (columns, date format, separators,
    -- encoding, default tax rate); validated by procure_domain::connector::Profile.
    config      jsonb NOT NULL CHECK (jsonb_typeof(config) = 'object'
                                      AND pg_column_size(config) <= 16384),
    created_by  uuid NOT NULL REFERENCES users (id),
    updated_by  uuid NOT NULL REFERENCES users (id),
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (org_id, name)
);

-- One uploaded file. `previewed`: read, nothing applied; `applied`: every record written;
-- `failed`: nothing applicable in it, or discarded by a person. A file (by SHA-256) is
-- imported once per organisation; after a failure the same file may be tried again.
CREATE TABLE import_batches (
    id              uuid PRIMARY KEY,
    org_id          uuid NOT NULL REFERENCES organisations (id),
    kind            text NOT NULL CHECK (kind IN ('po', 'receipt', 'invoice')),
    file_name       text NOT NULL CHECK (length(btrim(file_name)) BETWEEN 1 AND 255),
    file_sha256     text NOT NULL CHECK (file_sha256 ~ '^[0-9a-f]{64}$'),
    file_size       integer NOT NULL CHECK (file_size > 0),
    -- The profile used (a preset or a saved profile's name) and its settings at the time.
    profile         text NOT NULL CHECK (profile ~ '^[a-z0-9_]{1,60}$'),
    profile_config  jsonb NOT NULL CHECK (jsonb_typeof(profile_config) = 'object'),
    currency        text NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    -- Column → field code (null = ignored), as detected.
    mapping         jsonb NOT NULL CHECK (jsonb_typeof(mapping) = 'array'),
    status          text NOT NULL DEFAULT 'previewed'
                    CHECK (status IN ('previewed', 'applied', 'failed')),
    failure_code    text CHECK (failure_code IS NULL OR failure_code ~ '^[a-z_]{1,60}$'),
    data_rows       integer NOT NULL CHECK (data_rows >= 0),
    records         integer NOT NULL CHECK (records >= 0),
    error_rows      integer NOT NULL CHECK (error_rows >= 0),
    -- [{row, field, code}], at most a few hundred (the rest are counted in error_rows).
    errors          jsonb NOT NULL DEFAULT '[]'::jsonb
                    CHECK (jsonb_typeof(errors) = 'array' AND pg_column_size(errors) <= 262144),
    created_by      uuid NOT NULL REFERENCES users (id),
    created_at      timestamptz NOT NULL DEFAULT now(),
    decided_by      uuid REFERENCES users (id),
    decided_at      timestamptz,
    UNIQUE (id, org_id),
    CHECK ((status = 'previewed') = (decided_at IS NULL)),
    CHECK ((status = 'failed') = (failure_code IS NOT NULL))
);
CREATE UNIQUE INDEX import_batches_file ON import_batches (org_id, file_sha256)
    WHERE status <> 'failed';
CREATE INDEX import_batches_list ON import_batches (org_id, created_at DESC);

-- Staging: every record read from the file, waiting to be applied. Never changed once
-- written; what was applied is in the audit log (one event per record).
CREATE TABLE import_rows (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL,
    batch_id    uuid NOT NULL,
    seq         integer NOT NULL CHECK (seq >= 0),
    -- File rows (1-based) the record came from.
    source_rows integer[] NOT NULL CHECK (cardinality(source_rows) BETWEEN 1 AND 200),
    po_number   text NOT NULL CHECK (length(po_number) BETWEEN 1 AND 100),
    -- procure_server::integrations::Staged.
    record      jsonb NOT NULL CHECK (jsonb_typeof(record) = 'object'
                                      AND pg_column_size(record) <= 131072),
    FOREIGN KEY (batch_id, org_id) REFERENCES import_batches (id, org_id),
    UNIQUE (batch_id, seq)
);

ALTER TABLE integration_profiles ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration_profiles FORCE ROW LEVEL SECURITY;
CREATE POLICY integration_profiles_tenant ON integration_profiles
    USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id());

ALTER TABLE import_batches ENABLE ROW LEVEL SECURITY;
ALTER TABLE import_batches FORCE ROW LEVEL SECURITY;
CREATE POLICY import_batches_tenant ON import_batches
    USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id());

ALTER TABLE import_rows ENABLE ROW LEVEL SECURITY;
ALTER TABLE import_rows FORCE ROW LEVEL SECURITY;
CREATE POLICY import_rows_tenant ON import_rows
    USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id());

GRANT SELECT, INSERT, DELETE ON integration_profiles TO procure_runtime;
GRANT UPDATE (kind, config, updated_by, updated_at) ON integration_profiles TO procure_runtime;
GRANT SELECT, INSERT ON import_batches, import_rows TO procure_runtime;
-- What was uploaded never changes; only the decision on it.
GRANT UPDATE (status, failure_code, decided_by, decided_at) ON import_batches TO procure_runtime;
