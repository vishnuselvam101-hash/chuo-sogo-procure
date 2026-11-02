-- Factory-side fee model (CEO, 2026-11-02). We buy for the factory; the factory picks the
-- supplier and the price and pays the supplier itself. We charge no monthly fee: we receive
-- an agreed share (50%) of the amount the price went down from the price the factory paid
-- before, for an agreed term (36 months). No saving, no fee (procure_domain::savings).

-- The terms agreed with one factory (customer organisation). Written by the admin console.
CREATE TABLE savings_agreements (
    org_id        uuid PRIMARY KEY REFERENCES organisations (id),
    share_bps     integer NOT NULL DEFAULT 5000 CHECK (share_bps BETWEEN 1 AND 10000),
    term_months   integer NOT NULL DEFAULT 36 CHECK (term_months BETWEEN 1 AND 120),
    starts_on     date NOT NULL,
    -- Computed by the server (procure_domain::savings::term_end), last day covered.
    ends_on       date NOT NULL CHECK (ends_on >= starts_on),
    notes         text NOT NULL DEFAULT '' CHECK (length(notes) <= 2000),
    updated_by    uuid NOT NULL REFERENCES users (id),
    updated_at    timestamptz NOT NULL DEFAULT now()
);

-- The price paid before, per item, fixed once. Either taken from the factory's own purchase
-- records dated before the agreement started (never from orders placed through us, which
-- would erase the saving), or agreed with the factory for an item without such a record.
-- A baseline counts only once confirmed: automatically when it comes from a record the
-- factory's own users imported and states its unit, otherwise by a factory approver.
CREATE TABLE savings_baselines (
    id                 uuid PRIMARY KEY,
    org_id             uuid NOT NULL REFERENCES organisations (id),
    -- Canonical matching key of the item (procure_domain::history::item_key, after aliases).
    item_key           text NOT NULL CHECK (item_key ~ '^[md]:.+' AND length(item_key) <= 1100),
    source             text NOT NULL CHECK (source IN ('history', 'agreed')),
    history_id         uuid,
    purchased_on       date,
    supplier_name      text NOT NULL DEFAULT '' CHECK (length(supplier_name) <= 200),
    unit_price_milli   bigint NOT NULL CHECK (unit_price_milli >= 0),
    currency           text NOT NULL CHECK (app_currency_ok(currency)),
    unit               text NOT NULL DEFAULT '' CHECK (length(unit) <= 16),
    note               text NOT NULL DEFAULT '' CHECK (length(note) <= 1000),
    set_by             uuid NOT NULL REFERENCES users (id),
    set_at             timestamptz NOT NULL DEFAULT now(),
    confirmed_by       uuid REFERENCES users (id),
    confirmed_at       timestamptz,
    UNIQUE (org_id, item_key),
    UNIQUE (id, org_id),
    CHECK ((source = 'history') = (history_id IS NOT NULL)),
    CHECK ((confirmed_at IS NULL) = (confirmed_by IS NULL))
);

-- One row per order line, written when the order reconciles (the price was really paid).
-- `status` says why a line earns a fee or not; only 'counted' lines are billed.
CREATE TABLE savings_entries (
    id                         uuid PRIMARY KEY,
    org_id                     uuid NOT NULL,
    order_id                   uuid NOT NULL,
    order_item_id              uuid NOT NULL UNIQUE,
    item_key                   text,
    baseline_id                uuid,
    ordered_on                 date NOT NULL,
    quantity                   integer NOT NULL CHECK (quantity > 0),
    unit_price_minor           bigint NOT NULL CHECK (unit_price_minor >= 0),
    baseline_unit_price_milli  bigint CHECK (baseline_unit_price_milli >= 0),
    currency                   text NOT NULL CHECK (app_currency_ok(currency)),
    share_bps                  integer CHECK (share_bps BETWEEN 1 AND 10000),
    saving_minor               bigint NOT NULL DEFAULT 0 CHECK (saving_minor >= 0),
    fee_minor                  bigint NOT NULL DEFAULT 0 CHECK (fee_minor >= 0 AND fee_minor <= saving_minor),
    status                     text NOT NULL CHECK (status IN (
        'counted', 'no_saving', 'no_baseline', 'unconfirmed_baseline', 'mismatch',
        'outside_term', 'no_agreement')),
    -- The Chuo-Sogo invoice (draft or issued) that bills this fee.
    invoice_id                 uuid,
    created_at                 timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (order_id, org_id) REFERENCES purchase_orders (id, org_id),
    FOREIGN KEY (order_item_id, org_id) REFERENCES order_items (id, org_id),
    FOREIGN KEY (baseline_id, org_id) REFERENCES savings_baselines (id, org_id),
    FOREIGN KEY (invoice_id, org_id) REFERENCES issued_invoices (id, org_id) ON DELETE SET NULL (invoice_id),
    CHECK (status = 'counted' OR (fee_minor = 0 AND invoice_id IS NULL)),
    CHECK (status <> 'counted' OR (fee_minor > 0 AND baseline_id IS NOT NULL AND share_bps IS NOT NULL))
);
CREATE INDEX savings_entries_org ON savings_entries (org_id, ordered_on DESC);
CREATE INDEX savings_entries_open ON savings_entries (org_id) WHERE status = 'counted' AND invoice_id IS NULL;

-- The factory and its assigned staff read all three; terms and agreed baselines are written
-- by the admin console (platform scope); baselines from history and ledger rows are written
-- by the reconciling transaction of the organisation itself; only the admin console links
-- ledger rows to invoices.
ALTER TABLE savings_agreements ENABLE ROW LEVEL SECURITY;
ALTER TABLE savings_agreements FORCE ROW LEVEL SECURITY;
CREATE POLICY savings_agreements_read ON savings_agreements FOR SELECT
    USING (app_platform_scope() OR org_id = app_org_id());
CREATE POLICY savings_agreements_write ON savings_agreements FOR ALL
    USING (app_platform_scope() AND org_id = app_org_id())
    WITH CHECK (app_platform_scope() AND org_id = app_org_id());

ALTER TABLE savings_baselines ENABLE ROW LEVEL SECURITY;
ALTER TABLE savings_baselines FORCE ROW LEVEL SECURITY;
CREATE POLICY savings_baselines_tenant ON savings_baselines
    USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id());

ALTER TABLE savings_entries ENABLE ROW LEVEL SECURITY;
ALTER TABLE savings_entries FORCE ROW LEVEL SECURITY;
CREATE POLICY savings_entries_read ON savings_entries FOR SELECT
    USING (org_id = app_org_id());
CREATE POLICY savings_entries_insert ON savings_entries FOR INSERT
    WITH CHECK (org_id = app_org_id() AND invoice_id IS NULL);
CREATE POLICY savings_entries_bill ON savings_entries FOR UPDATE
    USING (app_platform_scope() AND org_id = app_org_id())
    WITH CHECK (app_platform_scope() AND org_id = app_org_id());
-- A line waiting for its baseline to be confirmed is priced when the factory confirms it.
CREATE POLICY savings_entries_price ON savings_entries FOR UPDATE
    USING (org_id = app_org_id() AND status = 'unconfirmed_baseline')
    WITH CHECK (org_id = app_org_id() AND invoice_id IS NULL);

-- Ledger rows are facts: only the invoice link may change after they are written, and a
-- line waiting for its baseline's confirmation gets its result (once).
CREATE FUNCTION savings_entry_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    priced text[] := ARRAY['status', 'saving_minor', 'fee_minor'];
BEGIN
    IF OLD.status = 'unconfirmed_baseline'
       AND NEW.status IN ('counted', 'no_saving', 'mismatch')
       AND (to_jsonb(NEW) - priced) IS NOT DISTINCT FROM (to_jsonb(OLD) - priced) THEN
        RETURN NEW;
    END IF;
    IF (to_jsonb(NEW) - 'invoice_id') IS DISTINCT FROM (to_jsonb(OLD) - 'invoice_id') THEN
        RAISE EXCEPTION 'savings entries are immutable' USING ERRCODE = 'P0001';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER savings_entries_guard BEFORE UPDATE ON savings_entries
    FOR EACH ROW EXECUTE FUNCTION savings_entry_guard();

-- A baseline's price is never edited: only its confirmation is added, once. A confirmed or
-- used baseline is never deleted.
CREATE FUNCTION savings_baseline_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    confirm text[] := ARRAY['confirmed_by', 'confirmed_at'];
BEGIN
    IF TG_OP = 'DELETE' THEN
        IF OLD.confirmed_at IS NOT NULL
           OR EXISTS (SELECT 1 FROM savings_entries WHERE baseline_id = OLD.id) THEN
            RAISE EXCEPTION 'baseline is fixed' USING ERRCODE = 'P0001';
        END IF;
        RETURN OLD;
    END IF;
    IF OLD.confirmed_at IS NOT NULL
       OR (to_jsonb(NEW) - confirm) IS DISTINCT FROM (to_jsonb(OLD) - confirm) THEN
        RAISE EXCEPTION 'baseline is fixed' USING ERRCODE = 'P0001';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER savings_baselines_guard BEFORE UPDATE OR DELETE ON savings_baselines
    FOR EACH ROW EXECUTE FUNCTION savings_baseline_guard();

GRANT SELECT, INSERT, UPDATE, DELETE ON savings_agreements TO procure_runtime;
GRANT SELECT, INSERT, DELETE ON savings_baselines TO procure_runtime;
GRANT UPDATE (confirmed_by, confirmed_at) ON savings_baselines TO procure_runtime;
GRANT SELECT, INSERT, UPDATE ON savings_entries TO procure_runtime;
