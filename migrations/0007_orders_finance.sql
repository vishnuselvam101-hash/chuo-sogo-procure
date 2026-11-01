-- Phase 5: orders and finance. The purchase order is the customer's own (we record its
-- number); deliveries are confirmed by the customer; the supplier's invoice is matched
-- three ways (ordered / received / invoiced) and every difference is a discrepancy a human
-- resolves.

CREATE TABLE purchase_orders (
    id                 uuid PRIMARY KEY,
    org_id             uuid NOT NULL,
    request_id         uuid NOT NULL,
    recommendation_id  uuid NOT NULL,
    supplier_id        uuid NOT NULL REFERENCES suppliers (id),
    -- The customer's own PO number (their ERP / paper system).
    po_number          text NOT NULL CHECK (length(btrim(po_number)) BETWEEN 1 AND 100),
    currency           text NOT NULL CHECK (app_currency_ok(currency)),
    shipping_minor     bigint NOT NULL CHECK (shipping_minor >= 0),
    total_minor        bigint NOT NULL CHECK (total_minor >= 0),
    notes              text NOT NULL DEFAULT '' CHECK (length(notes) <= 2000),
    ordered_by         uuid NOT NULL REFERENCES users (id),
    ordered_at         timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (request_id, org_id) REFERENCES procurement_requests (id, org_id),
    FOREIGN KEY (recommendation_id, org_id) REFERENCES recommendations (id, org_id),
    UNIQUE (request_id),
    UNIQUE (org_id, po_number),
    UNIQUE (id, org_id)
);

-- Snapshot of what was ordered (from the approved quotation).
CREATE TABLE order_items (
    id                uuid PRIMARY KEY,
    org_id            uuid NOT NULL,
    order_id          uuid NOT NULL,
    request_item_id   uuid NOT NULL,
    quantity          integer NOT NULL CHECK (quantity BETWEEN 1 AND 1000000000),
    unit_price_minor  bigint NOT NULL CHECK (unit_price_minor >= 0),
    offered_mpn       text NOT NULL DEFAULT '',
    alternative       boolean NOT NULL DEFAULT false,
    FOREIGN KEY (order_id, org_id) REFERENCES purchase_orders (id, org_id),
    FOREIGN KEY (request_item_id, org_id) REFERENCES request_items (id, org_id),
    UNIQUE (order_id, request_item_id),
    UNIQUE (id, org_id)
);

CREATE TABLE deliveries (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL,
    order_id     uuid NOT NULL,
    received_at  timestamptz NOT NULL,
    note         text NOT NULL DEFAULT '' CHECK (length(note) <= 2000),
    received_by  uuid NOT NULL REFERENCES users (id),
    created_at   timestamptz NOT NULL DEFAULT now(),
    -- A mistaken receipt is reversed, never deleted; reversed receipts do not count.
    reversed_at      timestamptz,
    reversed_by      uuid REFERENCES users (id),
    reversal_reason  text CHECK (length(reversal_reason) <= 2000),
    FOREIGN KEY (order_id, org_id) REFERENCES purchase_orders (id, org_id),
    UNIQUE (id, org_id),
    CHECK ((reversed_at IS NULL) = (reversed_by IS NULL))
);

CREATE TABLE delivery_lines (
    id             uuid PRIMARY KEY,
    org_id         uuid NOT NULL,
    delivery_id    uuid NOT NULL,
    order_item_id  uuid NOT NULL,
    quantity       integer NOT NULL CHECK (quantity BETWEEN 1 AND 1000000000),
    FOREIGN KEY (delivery_id, org_id) REFERENCES deliveries (id, org_id),
    FOREIGN KEY (order_item_id, org_id) REFERENCES order_items (id, org_id)
);

CREATE TABLE invoices (
    id                       uuid PRIMARY KEY,
    org_id                   uuid NOT NULL,
    order_id                 uuid NOT NULL,
    supplier_id              uuid NOT NULL REFERENCES suppliers (id),
    supplier_invoice_number  text NOT NULL CHECK (length(btrim(supplier_invoice_number)) BETWEEN 1 AND 100),
    invoice_date             date NOT NULL,
    due_date                 date,
    currency                 text NOT NULL CHECK (app_currency_ok(currency)),
    shipping_minor           bigint NOT NULL CHECK (shipping_minor >= 0),
    -- Rate for shipping and the default for lines (each line carries its own rate).
    tax_rate_bps             integer NOT NULL CHECK (tax_rate_bps BETWEEN 0 AND 10000),
    tax_rounding             text NOT NULL CHECK (tax_rounding IN ('floor', 'half_up', 'ceil')),
    tax_minor                bigint NOT NULL CHECK (tax_minor >= 0),
    -- Computed deterministically from the lines when recorded.
    subtotal_minor           bigint NOT NULL CHECK (subtotal_minor >= 0),
    total_minor              bigint NOT NULL CHECK (total_minor >= 0),
    -- 'matched' or 'disputed' at recording; 'reconciled' once every discrepancy is resolved.
    match_status             text NOT NULL CHECK (match_status IN ('matched', 'disputed', 'reconciled')),
    recorded_by              uuid NOT NULL REFERENCES users (id),
    created_at               timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (order_id, org_id) REFERENCES purchase_orders (id, org_id),
    -- One invoice per order in v1; a supplier's invoice number is booked only once.
    UNIQUE (order_id),
    UNIQUE (org_id, supplier_id, supplier_invoice_number),
    UNIQUE (id, org_id)
);

CREATE TABLE invoice_lines (
    id                uuid PRIMARY KEY,
    org_id            uuid NOT NULL,
    invoice_id        uuid NOT NULL,
    -- NULL: the supplier billed something that is not on the order (a discrepancy).
    order_item_id     uuid,
    description       text NOT NULL DEFAULT '' CHECK (length(description) <= 500),
    quantity          integer NOT NULL CHECK (quantity BETWEEN 1 AND 1000000000),
    unit_price_minor  bigint NOT NULL CHECK (unit_price_minor >= 0),
    tax_rate_bps      integer NOT NULL CHECK (tax_rate_bps BETWEEN 0 AND 10000),
    CHECK (order_item_id IS NOT NULL OR length(btrim(description)) > 0),
    FOREIGN KEY (invoice_id, org_id) REFERENCES invoices (id, org_id),
    FOREIGN KEY (order_item_id, org_id) REFERENCES order_items (id, org_id)
);

CREATE TABLE discrepancies (
    id              uuid PRIMARY KEY,
    org_id          uuid NOT NULL,
    invoice_id      uuid NOT NULL,
    kind            text NOT NULL CHECK (kind IN ('currency_mismatch', 'unknown_item', 'duplicate_line',
        'price_mismatch', 'quantity_over_ordered', 'quantity_over_received',
        'quantity_under_received', 'not_invoiced',
        'shipping_mismatch', 'tax_mismatch', 'amount_overflow')),
    order_item_id   uuid,
    -- What was expected and what the invoice says (minor units or quantities), for display.
    expected        bigint,
    actual          bigint,
    status          text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'resolved')),
    resolution      text CHECK (resolution IN ('accepted', 'credit_note', 'supplier_corrected', 'written_off', 'other')),
    resolution_note text NOT NULL DEFAULT '' CHECK (length(resolution_note) <= 2000),
    resolved_by     uuid REFERENCES users (id),
    resolved_at     timestamptz,
    FOREIGN KEY (invoice_id, org_id) REFERENCES invoices (id, org_id),
    FOREIGN KEY (order_item_id, org_id) REFERENCES order_items (id, org_id),
    CHECK ((status = 'resolved') = (resolution IS NOT NULL AND resolved_by IS NOT NULL AND resolved_at IS NOT NULL))
);

CREATE INDEX order_items_order ON order_items (order_id);
CREATE INDEX delivery_lines_item ON delivery_lines (order_item_id);
CREATE INDEX discrepancies_invoice ON discrepancies (invoice_id);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['purchase_orders', 'order_items', 'deliveries', 'delivery_lines',
                             'invoices', 'invoice_lines', 'discrepancies'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format('CREATE POLICY %I ON %I USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id())',
                       t || '_tenant', t);
    END LOOP;
END $$;

-- Records of fact: never edited by the application, except an invoice's match status and
-- a discrepancy's resolution.
GRANT SELECT, INSERT ON purchase_orders, order_items, delivery_lines, invoice_lines
    TO procure_runtime;
-- Updates only: invoice match status, discrepancy resolution, delivery reversal.
GRANT SELECT, INSERT, UPDATE ON invoices, discrepancies, deliveries TO procure_runtime;
