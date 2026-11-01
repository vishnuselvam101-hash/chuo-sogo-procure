-- Phase 4: managed operations. Work tasks with due dates (SLA), comments shared with the
-- customer or internal to Chuo-Sogo, a log of supplier communications, and service time.

-- Work items for Chuo-Sogo staff on a customer's requests. Some are created automatically
-- on workflow transitions (kind identify / chase_quotes), others by hand.
CREATE TABLE tasks (
    id            uuid PRIMARY KEY,
    org_id        uuid NOT NULL,
    request_id    uuid,
    -- Set for tasks about one RFQ (e.g. collecting its quotations).
    rfq_id        uuid,
    kind          text NOT NULL CHECK (kind IN ('identify', 'source', 'chase_quotes', 'compare',
                                                'follow_up', 'exception', 'other')),
    title         text NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 200),
    status        text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'in_progress', 'done', 'cancelled')),
    priority      text NOT NULL DEFAULT 'normal' CHECK (priority IN ('low', 'normal', 'high', 'urgent')),
    assignee_id   uuid REFERENCES users (id),
    due_at        timestamptz,
    created_by    uuid REFERENCES users (id),
    created_at    timestamptz NOT NULL DEFAULT now(),
    completed_at  timestamptz,
    FOREIGN KEY (request_id, org_id) REFERENCES procurement_requests (id, org_id),
    FOREIGN KEY (rfq_id, org_id) REFERENCES rfqs (id, org_id),
    CHECK ((status IN ('done', 'cancelled')) = (completed_at IS NOT NULL))
);
CREATE INDEX tasks_open ON tasks (org_id, status, due_at) WHERE status IN ('open', 'in_progress');
-- Automatic tasks (created_by IS NULL): at most one open "identify" per request and one open
-- "chase_quotes" per RFQ. Manual tasks of any kind are not limited.
CREATE UNIQUE INDEX tasks_identify_once ON tasks (request_id)
    WHERE kind = 'identify' AND created_by IS NULL AND status IN ('open', 'in_progress');
CREATE UNIQUE INDEX tasks_chase_once ON tasks (rfq_id)
    WHERE kind = 'chase_quotes' AND created_by IS NULL AND status IN ('open', 'in_progress');

-- Comments on a request. 'internal' comments are Chuo-Sogo notes: never visible to customer
-- users, enforced by RLS as well as by the API (staff scope is set only for assigned staff).
CREATE TABLE comments (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL,
    request_id  uuid NOT NULL,
    author_id   uuid NOT NULL REFERENCES users (id),
    visibility  text NOT NULL CHECK (visibility IN ('customer', 'internal')),
    body        text NOT NULL CHECK (length(btrim(body)) BETWEEN 1 AND 4000),
    created_at  timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (request_id, org_id) REFERENCES procurement_requests (id, org_id)
);
CREATE INDEX comments_request ON comments (request_id, created_at);

-- Record of contacts with suppliers (calls, emails sent by hand, meetings). Internal.
CREATE TABLE communications (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL,
    request_id   uuid NOT NULL,
    supplier_id  uuid REFERENCES suppliers (id),
    direction    text NOT NULL CHECK (direction IN ('inbound', 'outbound')),
    channel      text NOT NULL CHECK (channel IN ('email', 'phone', 'meeting', 'chat', 'other')),
    summary      text NOT NULL CHECK (length(btrim(summary)) BETWEEN 1 AND 4000),
    occurred_at  timestamptz NOT NULL,
    recorded_by  uuid NOT NULL REFERENCES users (id),
    created_at   timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (request_id, org_id) REFERENCES procurement_requests (id, org_id)
);
CREATE INDEX communications_request ON communications (request_id, occurred_at);

-- Service time spent by staff on a customer's request. Internal.
CREATE TABLE time_entries (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL,
    request_id  uuid NOT NULL,
    task_id     uuid REFERENCES tasks (id),
    user_id     uuid NOT NULL REFERENCES users (id),
    work_date   date NOT NULL,
    minutes     integer NOT NULL CHECK (minutes BETWEEN 1 AND 1440),
    note        text NOT NULL DEFAULT '' CHECK (length(note) <= 1000),
    created_at  timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (request_id, org_id) REFERENCES procurement_requests (id, org_id)
);
CREATE INDEX time_entries_request ON time_entries (request_id);

ALTER TABLE tasks ENABLE ROW LEVEL SECURITY;
ALTER TABLE tasks FORCE ROW LEVEL SECURITY;
CREATE POLICY tasks_staff ON tasks
    USING (org_id = app_org_id() AND app_staff()) WITH CHECK (org_id = app_org_id() AND app_staff());

ALTER TABLE comments ENABLE ROW LEVEL SECURITY;
ALTER TABLE comments FORCE ROW LEVEL SECURITY;
CREATE POLICY comments_visible ON comments
    USING (org_id = app_org_id() AND (visibility = 'customer' OR app_staff()))
    WITH CHECK (org_id = app_org_id() AND (visibility = 'customer' OR app_staff()));

ALTER TABLE communications ENABLE ROW LEVEL SECURITY;
ALTER TABLE communications FORCE ROW LEVEL SECURITY;
CREATE POLICY communications_staff ON communications
    USING (org_id = app_org_id() AND app_staff()) WITH CHECK (org_id = app_org_id() AND app_staff());

ALTER TABLE time_entries ENABLE ROW LEVEL SECURITY;
ALTER TABLE time_entries FORCE ROW LEVEL SECURITY;
CREATE POLICY time_entries_staff ON time_entries
    USING (org_id = app_org_id() AND app_staff()) WITH CHECK (org_id = app_org_id() AND app_staff());

GRANT SELECT, INSERT, UPDATE ON tasks TO procure_runtime;
-- Comments, communications and time entries are records: never edited by the application.
GRANT SELECT, INSERT ON comments, communications, time_entries TO procure_runtime;
