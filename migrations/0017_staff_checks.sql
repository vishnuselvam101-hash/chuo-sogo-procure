-- A person on our side checks the AI's work twice (CEO, 2026-11-02): before a request for
-- quotation goes to suppliers, and after the quotes are compared, before the factory sees
-- the recommendation. Each check is a recorded, audited step; the send refuses without it.

ALTER TABLE rfqs
    ADD COLUMN checked_by  uuid REFERENCES users (id),
    ADD COLUMN checked_at  timestamptz,
    ADD COLUMN check_note  text NOT NULL DEFAULT '' CHECK (length(check_note) <= 2000);
ALTER TABLE recommendations
    ADD COLUMN checked_by  uuid REFERENCES users (id),
    ADD COLUMN checked_at  timestamptz,
    ADD COLUMN check_note  text NOT NULL DEFAULT '' CHECK (length(check_note) <= 2000);

-- Sent before this rule existed: the person who sent them is recorded as the checker, so
-- later updates of these rows (approval, withdrawal, closing) still satisfy the rule.
UPDATE rfqs SET checked_by = created_by, checked_at = sent_at,
                check_note = 'Sent before the check step existed'
WHERE status <> 'draft';
UPDATE recommendations SET checked_by = recommended_by, checked_at = sent_at,
                           check_note = 'Sent before the check step existed'
WHERE sent_at IS NOT NULL;

ALTER TABLE rfqs
    ADD CONSTRAINT rfqs_check_pair CHECK ((checked_at IS NULL) = (checked_by IS NULL)),
    ADD CONSTRAINT rfqs_checked_before_sent CHECK (status = 'draft' OR checked_at IS NOT NULL);
ALTER TABLE recommendations
    ADD CONSTRAINT recommendations_check_pair CHECK ((checked_at IS NULL) = (checked_by IS NULL)),
    ADD CONSTRAINT recommendations_checked_before_sent CHECK (sent_at IS NULL OR checked_at IS NOT NULL);
