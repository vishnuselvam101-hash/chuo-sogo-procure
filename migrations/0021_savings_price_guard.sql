-- Security review 2026-11-03: a line waiting for its baseline may be priced by the
-- organisation's own transaction only once that baseline is confirmed, and never with a fee
-- above the saving (the application did both; now the database does too).
DROP POLICY savings_entries_price ON savings_entries;
CREATE POLICY savings_entries_price ON savings_entries FOR UPDATE
    USING (org_id = app_org_id() AND status = 'unconfirmed_baseline')
    WITH CHECK (
        org_id = app_org_id() AND invoice_id IS NULL AND fee_minor <= saving_minor
        AND EXISTS (SELECT 1 FROM savings_baselines b
                    WHERE b.id = savings_entries.baseline_id AND b.org_id = savings_entries.org_id
                      AND b.confirmed_at IS NOT NULL));
