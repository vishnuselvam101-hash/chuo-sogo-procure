-- Agent layer L2 (docs/agents/DESIGN.md): the matching agent. Three-way match stays exact;
-- per-organisation tolerances only classify each discrepancy afterwards, so the agent can
-- propose accepting a small one or asking the supplier for a credit note. A person decides.

-- Per-organisation match tolerances. No row = exact (every limit 0 = not set). Integer only:
-- shares in basis points, amounts in minor units of `currency` (never applied to another).
CREATE TABLE match_tolerances (
    org_id                uuid PRIMARY KEY REFERENCES organisations (id),
    currency              text NOT NULL DEFAULT 'JPY' CHECK (app_currency_ok(currency)),
    -- Unit price may differ from the ordered price by at most this share of it.
    price_bps             integer NOT NULL DEFAULT 0 CHECK (price_bps BETWEEN 0 AND 10000),
    -- A line's price difference (unit difference × invoiced quantity) at most this.
    price_line_abs_minor  bigint NOT NULL DEFAULT 0
                          CHECK (price_line_abs_minor BETWEEN 0 AND 1000000000000),
    quantity_bps          integer NOT NULL DEFAULT 0 CHECK (quantity_bps BETWEEN 0 AND 10000),
    quantity_abs          integer NOT NULL DEFAULT 0 CHECK (quantity_abs BETWEEN 0 AND 1000000000),
    -- Printed tax may differ from the recomputed tax by at most this (rounding).
    rounding_minor        bigint NOT NULL DEFAULT 0 CHECK (rounding_minor BETWEEN 0 AND 1000000),
    updated_by            uuid REFERENCES users (id),
    updated_at            timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE match_tolerances ENABLE ROW LEVEL SECURITY;
ALTER TABLE match_tolerances FORCE ROW LEVEL SECURITY;
CREATE POLICY match_tolerances_tenant ON match_tolerances
    USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id());
GRANT SELECT, INSERT ON match_tolerances TO procure_runtime;
GRANT UPDATE (currency, price_bps, price_line_abs_minor, quantity_bps, quantity_abs,
              rounding_minor, updated_by, updated_at)
    ON match_tolerances TO procure_runtime;

-- An approved "request a credit note" proposal writes a message to the supplier through the
-- outbox (test mode unless live mail is configured and the organisation allows agent mail).
-- One request per discrepancy, ever.
ALTER TABLE discrepancies ADD CONSTRAINT discrepancies_id_org UNIQUE (id, org_id);
ALTER TABLE outbox DROP CONSTRAINT outbox_kind_check;
ALTER TABLE outbox
    ADD CONSTRAINT outbox_kind_check
        CHECK (kind IN ('rfq', 'reminder', 'order', 'credit_note_request')),
    ADD COLUMN discrepancy_id uuid,
    ADD FOREIGN KEY (discrepancy_id, org_id) REFERENCES discrepancies (id, org_id),
    ADD CONSTRAINT outbox_credit_note_discrepancy
        CHECK ((kind = 'credit_note_request') = (discrepancy_id IS NOT NULL)),
    -- Written because a named person approved it.
    ADD CONSTRAINT outbox_credit_note_checked
        CHECK (kind <> 'credit_note_request' OR checked_by IS NOT NULL);
CREATE UNIQUE INDEX outbox_credit_note_once ON outbox (discrepancy_id)
    WHERE kind = 'credit_note_request';

-- Invoice extraction from pasted supplier invoice text, for the order it bills.
ALTER TABLE ai_suggestions DROP CONSTRAINT ai_suggestions_capability_check;
ALTER TABLE ai_suggestions ADD CONSTRAINT ai_suggestions_capability_check
    CHECK (capability IN ('extract_items', 'extract_quotation', 'extract_listing',
                          'translate_listing', 'extract_invoice'));
ALTER TABLE ai_suggestions DROP CONSTRAINT ai_suggestions_subject_type_check;
ALTER TABLE ai_suggestions ADD CONSTRAINT ai_suggestions_subject_type_check
    CHECK (subject_type IN ('procurement_request', 'rfq', 'listing', 'supplier_org',
                            'purchase_order'));
