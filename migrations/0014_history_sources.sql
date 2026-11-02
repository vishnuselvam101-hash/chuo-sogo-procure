-- Where a block of purchase history came from: a spreadsheet or text PDF the customer handed
-- over, a scanned paper / FAX record read by OCR on this server, or an order completed in
-- this system (recorded automatically when the order reconciles, so the history keeps
-- growing as the service is used). Every source can be withdrawn the same way.

ALTER TABLE history_imports
    ADD COLUMN source   text NOT NULL DEFAULT 'file' CHECK (source IN ('file', 'ocr', 'order')),
    ADD COLUMN order_id uuid,
    ADD FOREIGN KEY (order_id, org_id) REFERENCES purchase_orders (id, org_id),
    ADD CHECK ((source = 'order') = (order_id IS NOT NULL)),
    -- An order is recorded once.
    ADD UNIQUE (order_id);
