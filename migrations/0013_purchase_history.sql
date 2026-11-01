-- A customer's decision material, brought in on day one: past purchases imported from the
-- spreadsheets / ERP exports they already keep, and their written buying criteria. Without
-- these the service starts every request from nothing; with them, every request line shows
-- what was paid before, to whom, and the operator works to the customer's own rules.

CREATE TABLE history_imports (
    id             uuid PRIMARY KEY,
    org_id         uuid NOT NULL REFERENCES organisations (id),
    file_name      text NOT NULL CHECK (length(btrim(file_name)) BETWEEN 1 AND 255),
    -- Column → field mapping that was applied (field codes, null = ignored).
    mapping        jsonb NOT NULL,
    rows_imported  integer NOT NULL CHECK (rows_imported >= 0),
    rows_skipped   integer NOT NULL CHECK (rows_skipped >= 0),
    imported_by    uuid NOT NULL REFERENCES users (id),
    imported_at    timestamptz NOT NULL DEFAULT now(),
    -- An import can be withdrawn as a whole (wrong file, wrong columns); its rows go with it.
    withdrawn_by   uuid REFERENCES users (id),
    withdrawn_at   timestamptz,
    UNIQUE (id, org_id),
    CHECK ((withdrawn_at IS NULL) = (withdrawn_by IS NULL))
);
CREATE INDEX history_imports_org ON history_imports (org_id, imported_at DESC);

CREATE TABLE purchase_history (
    id                 uuid PRIMARY KEY,
    org_id             uuid NOT NULL,
    import_id          uuid NOT NULL,
    source_row         integer NOT NULL CHECK (source_row > 0),
    purchased_on       date NOT NULL,
    supplier_name      text NOT NULL DEFAULT '' CHECK (length(supplier_name) <= 200),
    description        text NOT NULL DEFAULT '' CHECK (length(description) <= 1000),
    manufacturer       text NOT NULL DEFAULT '' CHECK (length(manufacturer) <= 200),
    mpn                text NOT NULL DEFAULT '' CHECK (length(mpn) <= 100),
    -- Matching key (procure_domain::history::item_key): normalised part number or description.
    item_key           text NOT NULL CHECK (item_key ~ '^[md]:.+'),
    -- Thousandths of a unit, and of a minor currency unit (exact; no floats).
    quantity_milli     bigint NOT NULL CHECK (quantity_milli > 0),
    unit               text NOT NULL DEFAULT '' CHECK (length(unit) <= 16),
    unit_price_milli   bigint NOT NULL CHECK (unit_price_milli >= 0),
    price_derived      boolean NOT NULL,
    currency           text NOT NULL CHECK (app_currency_ok(currency)),
    lead_time_days     integer CHECK (lead_time_days BETWEEN 0 AND 3650),
    notes              text NOT NULL DEFAULT '' CHECK (length(notes) <= 1000),
    FOREIGN KEY (import_id, org_id) REFERENCES history_imports (id, org_id) ON DELETE CASCADE,
    UNIQUE (import_id, source_row)
);
CREATE INDEX purchase_history_key ON purchase_history (org_id, item_key, purchased_on DESC);
CREATE INDEX purchase_history_date ON purchase_history (org_id, purchased_on DESC);

-- One set of criteria per customer organisation.
CREATE TABLE buying_criteria (
    org_id                   uuid PRIMARY KEY REFERENCES organisations (id),
    -- Ordered priorities, most important first.
    priorities               text[] NOT NULL DEFAULT '{}'
                             CHECK (priorities <@ ARRAY['price', 'lead_time', 'quality', 'existing_supplier', 'domestic']::text[]
                                    AND cardinality(priorities) <= 5),
    preferred_suppliers      text[] NOT NULL DEFAULT '{}' CHECK (cardinality(preferred_suppliers) <= 50),
    excluded_suppliers       text[] NOT NULL DEFAULT '{}' CHECK (cardinality(excluded_suppliers) <= 50),
    required_certifications  text[] NOT NULL DEFAULT '{}' CHECK (cardinality(required_certifications) <= 20),
    max_lead_time_days       integer CHECK (max_lead_time_days BETWEEN 1 AND 3650),
    -- Above this change against the last price paid, the operator must explain the quote.
    price_alert_bps          integer CHECK (price_alert_bps BETWEEN 0 AND 100000),
    notes                    text NOT NULL DEFAULT '' CHECK (length(notes) <= 4000),
    updated_by               uuid NOT NULL REFERENCES users (id),
    updated_at               timestamptz NOT NULL DEFAULT now()
);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['history_imports', 'purchase_history', 'buying_criteria'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format('CREATE POLICY %I ON %I USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id())',
                       t || '_tenant', t);
    END LOOP;
END $$;

-- Imported rows are records of fact: never edited, only removed with their whole import.
GRANT SELECT, INSERT, UPDATE ON history_imports TO procure_runtime;
GRANT SELECT, INSERT, DELETE ON purchase_history TO procure_runtime;
GRANT SELECT, INSERT, UPDATE ON buying_criteria TO procure_runtime;
