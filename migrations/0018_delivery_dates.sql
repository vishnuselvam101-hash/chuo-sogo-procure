-- After the factory approves, we arrange the order and check the delivery date (CEO,
-- 2026-11-02). The promised date comes from the quotation's lead time when the order is
-- recorded; the supplier's confirmed date is entered by our staff; an automatic
-- "check_delivery" task is due on the expected date and closes when everything arrived.

ALTER TABLE purchase_orders
    ADD COLUMN promised_on         date,
    ADD COLUMN delivery_confirmed_on  date,
    ADD COLUMN delivery_note       text NOT NULL DEFAULT '' CHECK (length(delivery_note) <= 2000),
    ADD COLUMN delivery_confirmed_by  uuid REFERENCES users (id),
    ADD COLUMN delivery_confirmed_at  timestamptz,
    ADD CONSTRAINT purchase_orders_delivery_confirm CHECK (
        (delivery_confirmed_on IS NULL) = (delivery_confirmed_by IS NULL)
        AND (delivery_confirmed_on IS NULL) = (delivery_confirmed_at IS NULL));

ALTER TABLE tasks DROP CONSTRAINT tasks_kind_check;
ALTER TABLE tasks ADD CONSTRAINT tasks_kind_check CHECK (kind IN ('identify', 'source',
    'chase_quotes', 'compare', 'check_delivery', 'follow_up', 'exception', 'other'));
CREATE UNIQUE INDEX tasks_check_delivery_once ON tasks (request_id)
    WHERE kind = 'check_delivery' AND created_by IS NULL AND status IN ('open', 'in_progress');

-- Orders stay records of fact; only the delivery-date follow-up columns may change.
GRANT UPDATE (delivery_confirmed_on, delivery_note, delivery_confirmed_by, delivery_confirmed_at)
    ON purchase_orders TO procure_runtime;
