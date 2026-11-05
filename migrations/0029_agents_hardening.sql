-- Agent layer hardening after the L9 review (docs/agents/L9_FINDINGS.md, 2026-11-05).

-- S5: automatic supplier mail is two-party. The customer's admin requests it
-- (`auto_send_supplier_mail`); Chuo-Sogo staff confirm it. It takes effect only while both
-- are set; a change of the customer's request clears the confirmation.
ALTER TABLE agent_settings
    ADD COLUMN auto_send_confirmed_by uuid REFERENCES users (id),
    ADD COLUMN auto_send_confirmed_at timestamptz,
    ADD CONSTRAINT agent_settings_auto_send_confirmation
        CHECK ((auto_send_confirmed_by IS NULL) = (auto_send_confirmed_at IS NULL)),
    ADD CONSTRAINT agent_settings_auto_send_confirmed_requested
        CHECK (auto_send_confirmed_at IS NULL OR auto_send_supplier_mail);
GRANT UPDATE (auto_send_confirmed_by, auto_send_confirmed_at) ON agent_settings TO procure_runtime;

-- S3 / S6: the monthly budget counts runs by when they started; reply runs are counted per
-- order and day.
CREATE INDEX agent_runs_started ON agent_runs (org_id, started_at) WHERE started_at IS NOT NULL;
CREATE INDEX agent_runs_reply_order ON agent_runs (org_id, (input ->> 'order_id'), started_at)
    WHERE agent = 'chasing' AND input ? 'document_id';
