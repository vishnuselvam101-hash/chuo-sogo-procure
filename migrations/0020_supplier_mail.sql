-- Live supplier mail and FAX (founder approval 2026-11-03). Test mode stays the default: a
-- message is only sent when the server is configured with a mail account
-- (PROCURE_SUPPLIER_MAIL=live, src/supplier_mail.rs). FAX goes out and comes in through a
-- mail-to-FAX gateway, so both use the same queue and the same inbox.

-- How a supplier wants quote requests: by email (default) or by FAX.
ALTER TABLE suppliers
    ADD COLUMN fax text CHECK (fax IS NULL OR fax ~ '^\+?[0-9]{6,20}$'),
    ADD COLUMN rfq_channel text NOT NULL DEFAULT 'email' CHECK (rfq_channel IN ('email', 'fax')),
    ADD CHECK (rfq_channel <> 'fax' OR fax IS NOT NULL);

-- The outbox becomes a queue: 'test' messages are only recorded (as before); 'email' and
-- 'fax' messages are sent by the server's sender, retried, and marked sent or failed.
ALTER TABLE outbox DROP CONSTRAINT outbox_channel_check;
ALTER TABLE outbox
    ADD CONSTRAINT outbox_channel_check CHECK (channel IN ('test', 'email', 'fax')),
    ADD COLUMN kind text NOT NULL DEFAULT 'rfq' CHECK (kind IN ('rfq', 'reminder')),
    -- Written into the subject; a reply quoting it is matched to its RFQ and supplier.
    ADD COLUMN reply_token text CHECK (reply_token IS NULL OR reply_token ~ '^[0-9a-f]{12}$'),
    ADD COLUMN status text NOT NULL DEFAULT 'recorded'
        CHECK (status IN ('recorded', 'queued', 'sent', 'failed')),
    ADD COLUMN attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    ADD COLUMN next_attempt_at timestamptz,
    ADD COLUMN sent_at timestamptz,
    ADD COLUMN message_id text CHECK (message_id IS NULL OR length(message_id) <= 300),
    ADD COLUMN last_error text CHECK (last_error IS NULL OR length(last_error) <= 1000),
    ADD CHECK ((channel = 'test') = (status = 'recorded')),
    ADD CHECK ((status = 'sent') = (sent_at IS NOT NULL)),
    ADD CHECK (channel = 'test' OR (to_address IS NOT NULL AND reply_token IS NOT NULL));
CREATE INDEX outbox_queued ON outbox (next_attempt_at) WHERE status = 'queued';
CREATE UNIQUE INDEX outbox_reply_token ON outbox (reply_token) WHERE reply_token IS NOT NULL;
-- Only the sender's bookkeeping changes; what was sent, to whom, never does.
GRANT UPDATE (status, attempts, next_attempt_at, sent_at, message_id, last_error)
    ON outbox TO procure_runtime;

-- Every message read from the mailbox, before we know whose it is. A reply quoting a token
-- is assigned to its RFQ at once; anything else (a FAX, a new thread) waits for our staff to
-- assign it or set it aside. Platform-level: only internal staff (platform scope) see it.
CREATE TABLE mail_inbox (
    id              uuid PRIMARY KEY,
    -- IMAP identity, so a message is read once.
    uid_validity    bigint NOT NULL,
    uid             bigint NOT NULL,
    channel         text NOT NULL CHECK (channel IN ('email', 'fax')),
    from_address    text NOT NULL DEFAULT '' CHECK (length(from_address) <= 320),
    subject         text NOT NULL DEFAULT '' CHECK (length(subject) <= 500),
    received_at     timestamptz NOT NULL,
    size_bytes      bigint NOT NULL CHECK (size_bytes >= 0),
    -- NULL for a message not downloaded (empty or over 10 MB): recorded so it is not
    -- fetched again, and listed for staff to look at in the mail client.
    sha256          bytea CHECK (length(sha256) = 32),
    status          text NOT NULL DEFAULT 'unassigned'
                    CHECK (status IN ('unassigned', 'assigned', 'set_aside', 'skipped')),
    org_id          uuid REFERENCES organisations (id),
    document_id     uuid,
    decided_by      uuid REFERENCES users (id),
    decided_at      timestamptz,
    created_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (uid_validity, uid),
    FOREIGN KEY (document_id, org_id) REFERENCES supplier_documents (id, org_id),
    CHECK ((status = 'assigned') = (document_id IS NOT NULL)),
    CHECK ((status = 'skipped') = (sha256 IS NULL))
);
CREATE INDEX mail_inbox_open ON mail_inbox (received_at) WHERE status = 'unassigned';
ALTER TABLE mail_inbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE mail_inbox FORCE ROW LEVEL SECURITY;
CREATE POLICY mail_inbox_platform ON mail_inbox
    USING (app_platform_scope()) WITH CHECK (app_platform_scope());
GRANT SELECT, INSERT ON mail_inbox TO procure_runtime;
GRANT UPDATE (status, org_id, document_id, decided_by, decided_at) ON mail_inbox TO procure_runtime;
