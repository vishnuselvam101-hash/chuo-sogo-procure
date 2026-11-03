-- CEO specification 2026-11-03.
-- 1. The order email: after the factory approves and records its own PO number, the system
--    prepares the order to the chosen supplier; a person on our side checks it and sends it
--    through the same outbox as quote requests (test mode unless live mail is configured).
-- 2. Exceptions go to our staff as tasks: suppliers that did not reply by the response date,
--    and request lines whose model number we do not know.

ALTER TABLE outbox DROP CONSTRAINT outbox_kind_check;
ALTER TABLE outbox
    ADD CONSTRAINT outbox_kind_check CHECK (kind IN ('rfq', 'reminder', 'order')),
    -- The order an `order` message belongs to (its recipient is the quotation's supplier).
    ADD COLUMN order_id uuid,
    ADD COLUMN checked_by uuid REFERENCES users (id),
    ADD FOREIGN KEY (order_id, org_id) REFERENCES purchase_orders (id, org_id),
    ADD CHECK ((kind = 'order') = (order_id IS NOT NULL)),
    -- An order message is always checked by a named person before it is written.
    ADD CHECK (kind <> 'order' OR checked_by IS NOT NULL);
-- One order email per order.
CREATE UNIQUE INDEX outbox_order_once ON outbox (order_id) WHERE kind = 'order';

ALTER TABLE tasks DROP CONSTRAINT tasks_kind_check;
ALTER TABLE tasks ADD CONSTRAINT tasks_kind_check CHECK (kind IN ('identify', 'source',
    'chase_quotes', 'compare', 'check_delivery', 'follow_up', 'exception', 'other',
    'send_order', 'no_reply', 'unknown_item'));
-- At most one open automatic "send the order email" task per request.
CREATE UNIQUE INDEX tasks_send_order_once ON tasks (request_id)
    WHERE kind = 'send_order' AND created_by IS NULL AND status IN ('open', 'in_progress');
-- One automatic no-reply task per RFQ, ever: once our staff handled it, it stays handled.
CREATE UNIQUE INDEX tasks_no_reply_once ON tasks (rfq_id)
    WHERE kind = 'no_reply' AND created_by IS NULL;
