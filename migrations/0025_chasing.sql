-- Agents lane L3 (docs/agents/DESIGN.md): the chasing agent. It finds orders that need a
-- follow-up with the supplier (no acknowledgement, no delivery date, delivery overdue,
-- delivered but not invoiced) and proposes a polite follow-up email; a person approves it
-- and it goes through the outbox like every other supplier message (test mode unless live
-- mail is configured and the organisation allows agent mail). Replies are read by the
-- `parse_supplier_reply` capability into suggestions that a person applies.

-- Follow-ups are outbox messages of their own kind, tied to the order, with their type and
-- attempt number.
ALTER TABLE outbox DROP CONSTRAINT outbox_kind_check;
ALTER TABLE outbox ADD CONSTRAINT outbox_kind_check
    CHECK (kind IN ('rfq', 'reminder', 'order', 'chase'));
-- 0022 tied order_id to kind 'order' only (an unnamed CHECK): widen it to follow-ups.
DO $$
DECLARE c text;
BEGIN
    FOR c IN SELECT conname FROM pg_constraint
             WHERE conrelid = 'outbox'::regclass AND contype = 'c'
               AND pg_get_constraintdef(oid) LIKE '%''order''::text) = (order_id IS NOT NULL)%'
    LOOP
        EXECUTE format('ALTER TABLE outbox DROP CONSTRAINT %I', c);
    END LOOP;
END $$;
ALTER TABLE outbox
    ADD COLUMN chase_type text CHECK (chase_type IS NULL OR chase_type IN
        ('order_ack', 'delivery_date', 'overdue_delivery', 'missing_invoice')),
    ADD COLUMN chase_attempt integer CHECK (chase_attempt IS NULL OR chase_attempt BETWEEN 1 AND 10),
    ADD CONSTRAINT outbox_order_link CHECK ((kind IN ('order', 'chase')) = (order_id IS NOT NULL)),
    ADD CONSTRAINT outbox_chase_fields CHECK (
        (kind = 'chase') = (chase_type IS NOT NULL)
        AND (kind = 'chase') = (chase_attempt IS NOT NULL));
-- One message per order, type and attempt.
CREATE UNIQUE INDEX outbox_chase_once ON outbox (order_id, chase_type, chase_attempt)
    WHERE kind = 'chase';

-- The supplier acknowledged the order (recorded by a person from its reply).
ALTER TABLE purchase_orders
    ADD COLUMN acknowledged_at timestamptz,
    ADD COLUMN acknowledged_by uuid REFERENCES users (id),
    ADD CONSTRAINT purchase_orders_acknowledged
        CHECK ((acknowledged_at IS NULL) = (acknowledged_by IS NULL));
-- A promised date the quotation did not give may be recorded once from the supplier's reply;
-- after that it is a record of fact (a later change is a confirmed date instead).
CREATE FUNCTION purchase_orders_promised_once() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.promised_on IS NOT NULL AND NEW.promised_on IS DISTINCT FROM OLD.promised_on THEN
        RAISE EXCEPTION 'promised_on is already recorded' USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER purchase_orders_promised_once BEFORE UPDATE OF promised_on ON purchase_orders
    FOR EACH ROW EXECUTE FUNCTION purchase_orders_promised_once();
GRANT UPDATE (promised_on, acknowledged_at, acknowledged_by) ON purchase_orders TO procure_runtime;

-- Per-organisation cadence, in Japan business days. No row = defaults.
CREATE TABLE chase_settings (
    org_id              uuid PRIMARY KEY REFERENCES organisations (id),
    ack_days            integer NOT NULL DEFAULT 2 CHECK (ack_days BETWEEN 1 AND 30),
    delivery_date_days  integer NOT NULL DEFAULT 3 CHECK (delivery_date_days BETWEEN 1 AND 30),
    overdue_days        integer NOT NULL DEFAULT 1 CHECK (overdue_days BETWEEN 1 AND 30),
    invoice_days        integer NOT NULL DEFAULT 10 CHECK (invoice_days BETWEEN 1 AND 60),
    repeat_days         integer NOT NULL DEFAULT 3 CHECK (repeat_days BETWEEN 1 AND 30),
    max_attempts        integer NOT NULL DEFAULT 3 CHECK (max_attempts BETWEEN 1 AND 10),
    updated_by          uuid REFERENCES users (id),
    updated_at          timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE chase_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE chase_settings FORCE ROW LEVEL SECURITY;
CREATE POLICY chase_settings_tenant ON chase_settings
    USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id());
GRANT SELECT, INSERT ON chase_settings TO procure_runtime;
GRANT UPDATE (ack_days, delivery_date_days, overdue_days, invoice_days, repeat_days,
              max_attempts, updated_by, updated_at)
    ON chase_settings TO procure_runtime;

-- Suggestions made by agents: the capabilities of all agent lanes, subjects they refer to,
-- and no requesting user (the agent run made it; the exception links it).
ALTER TABLE ai_suggestions DROP CONSTRAINT ai_suggestions_capability_check;
ALTER TABLE ai_suggestions ADD CONSTRAINT ai_suggestions_capability_check
    CHECK (capability IN ('extract_items', 'extract_quotation', 'extract_listing',
                          'translate_listing', 'extract_invoice', 'parse_supplier_reply',
                          'negotiation_brief'));
ALTER TABLE ai_suggestions DROP CONSTRAINT ai_suggestions_subject_type_check;
ALTER TABLE ai_suggestions ADD CONSTRAINT ai_suggestions_subject_type_check
    CHECK (subject_type IN ('procurement_request', 'rfq', 'listing', 'supplier_org',
                            'purchase_order', 'invoice', 'supplier_document'));
ALTER TABLE ai_suggestions ALTER COLUMN requested_by DROP NOT NULL;
ALTER TABLE ai_suggestions ADD CONSTRAINT ai_suggestions_requester
    CHECK (requested_by IS NOT NULL
           OR capability IN ('extract_invoice', 'parse_supplier_reply', 'negotiation_brief'));
