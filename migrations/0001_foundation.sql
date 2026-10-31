-- Phase 1 foundation: organisations, users, memberships, sessions, assignments, audit.
-- Tenant isolation: RLS keyed on per-transaction settings set by the server:
--   app.org_id  = organisation the current transaction acts on
--   app.user_id = authenticated user (lets a user see their own memberships across orgs)
-- FORCE ROW LEVEL SECURITY makes RLS apply to the table owner too.

CREATE EXTENSION IF NOT EXISTS citext;

CREATE FUNCTION app_org_id() RETURNS uuid LANGUAGE sql STABLE AS
$$ SELECT nullif(current_setting('app.org_id', true), '')::uuid $$;

CREATE FUNCTION app_user_id() RETURNS uuid LANGUAGE sql STABLE AS
$$ SELECT nullif(current_setting('app.user_id', true), '')::uuid $$;

CREATE TABLE organisations (
    id              uuid PRIMARY KEY,
    kind            text NOT NULL CHECK (kind IN ('platform', 'customer')),
    name            text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    default_locale  text NOT NULL DEFAULT 'ja' CHECK (default_locale ~ '^[a-z]{2}(-[A-Za-z]{2,4})?$'),
    timezone        text NOT NULL DEFAULT 'Asia/Tokyo',
    status          text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended')),
    created_at      timestamptz NOT NULL DEFAULT now()
);
-- Exactly one platform (Chuo-Sogo) organisation.
CREATE UNIQUE INDEX organisations_single_platform ON organisations (kind) WHERE kind = 'platform';

CREATE TABLE users (
    id                 uuid PRIMARY KEY,
    email              citext NOT NULL UNIQUE CHECK (length(email) <= 254 AND email LIKE '%_@_%'),
    display_name       text NOT NULL CHECK (length(btrim(display_name)) BETWEEN 1 AND 120),
    -- An account is permanently either Chuo-Sogo staff or a customer user, never both.
    account_kind       text NOT NULL CHECK (account_kind IN ('internal', 'customer')),
    password_hash      text,                 -- NULL until activated
    locale             text CHECK (locale IS NULL OR locale ~ '^[a-z]{2}(-[A-Za-z]{2,4})?$'),
    status             text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'active', 'deactivated')),
    failed_logins      integer NOT NULL DEFAULT 0 CHECK (failed_logins >= 0),
    locked_until       timestamptz,
    created_at         timestamptz NOT NULL DEFAULT now(),
    CHECK (status <> 'active' OR password_hash IS NOT NULL)
);

CREATE TABLE memberships (
    user_id     uuid NOT NULL REFERENCES users (id),
    org_id      uuid NOT NULL REFERENCES organisations (id),
    role        text NOT NULL CHECK (role IN ('platform_super_admin', 'procurement_admin', 'procurement_operator',
                                              'customer_admin', 'procurement_manager', 'requester', 'finance')),
    -- 'invited': an existing account was added and has not accepted yet (no access, profile hidden).
    status      text NOT NULL DEFAULT 'active' CHECK (status IN ('invited', 'active', 'declined', 'deactivated')),
    -- What the inviting admin typed; shown instead of the account's own profile until accepted.
    invited_name  text CHECK (invited_name IS NULL OR length(btrim(invited_name)) BETWEEN 1 AND 120),
    invited_email citext CHECK (invited_email IS NULL OR length(invited_email) <= 254),
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, org_id)
);
CREATE INDEX memberships_org ON memberships (org_id);

-- Role and account kind must match the organisation kind:
-- internal roles and internal accounts only in the platform org, customer ones only in customer orgs.
CREATE FUNCTION memberships_role_matches_kind() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE k text; ak text; prev text;
BEGIN
    -- Exact-key lookups regardless of the caller's RLS context; restored right after.
    prev := current_setting('app.auth', true);
    PERFORM set_config('app.auth', 'on', true);
    SELECT kind INTO k FROM organisations WHERE id = NEW.org_id;
    SELECT account_kind INTO ak FROM users WHERE id = NEW.user_id;
    PERFORM set_config('app.auth', coalesce(prev, ''), true);
    IF (k = 'platform') <> (NEW.role IN ('platform_super_admin', 'procurement_admin', 'procurement_operator')) THEN
        RAISE EXCEPTION 'role % not allowed in % organisation', NEW.role, k USING ERRCODE = 'check_violation';
    END IF;
    IF (k = 'platform') <> (ak = 'internal') THEN
        RAISE EXCEPTION '% account not allowed in % organisation', ak, k USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER memberships_role_kind BEFORE INSERT OR UPDATE ON memberships
    FOR EACH ROW EXECUTE FUNCTION memberships_role_matches_kind();

ALTER TABLE memberships ENABLE ROW LEVEL SECURITY;
ALTER TABLE memberships FORCE ROW LEVEL SECURITY;
CREATE POLICY memberships_tenant ON memberships
    USING (org_id = app_org_id() OR user_id = app_user_id())
    WITH CHECK (org_id = app_org_id());

CREATE TABLE sessions (
    id            uuid PRIMARY KEY,
    token_hash    bytea NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    user_id       uuid NOT NULL REFERENCES users (id),
    created_at    timestamptz NOT NULL DEFAULT now(),
    last_seen_at  timestamptz NOT NULL DEFAULT now(),
    expires_at    timestamptz NOT NULL,
    revoked_at    timestamptz,
    user_agent    text CHECK (length(user_agent) <= 300)
);
CREATE INDEX sessions_user ON sessions (user_id) WHERE revoked_at IS NULL;

-- One-time tokens for activation and password reset (only the hash is stored).
CREATE TABLE user_tokens (
    token_hash  bytea PRIMARY KEY CHECK (length(token_hash) = 32),
    user_id     uuid NOT NULL REFERENCES users (id),
    purpose     text NOT NULL CHECK (purpose IN ('activation', 'password_reset')),
    expires_at  timestamptz NOT NULL,
    used_at     timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX user_tokens_user ON user_tokens (user_id);

-- Internal staff ↔ customer org grants. Scoped to the customer org.
CREATE TABLE service_assignments (
    id           uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES organisations (id),
    user_id     uuid NOT NULL REFERENCES users (id),
    granted_by  uuid NOT NULL REFERENCES users (id),
    created_at  timestamptz NOT NULL DEFAULT now(),
    revoked_at  timestamptz
);
CREATE UNIQUE INDEX service_assignments_active ON service_assignments (org_id, user_id) WHERE revoked_at IS NULL;

ALTER TABLE service_assignments ENABLE ROW LEVEL SECURITY;
ALTER TABLE service_assignments FORCE ROW LEVEL SECURITY;
CREATE POLICY service_assignments_tenant ON service_assignments
    USING (org_id = app_org_id() OR user_id = app_user_id())
    WITH CHECK (org_id = app_org_id());

-- Append-only, hash-chained per organisation.
CREATE TABLE audit_events (
    id             bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    org_id         uuid NOT NULL REFERENCES organisations (id),
    seq            bigint NOT NULL,
    actor_user_id  uuid REFERENCES users (id),
    action         text NOT NULL CHECK (action ~ '^[a-z_]+\.[a-z_]+$'),
    entity_type    text NOT NULL,
    entity_id      text,
    data           jsonb NOT NULL DEFAULT '{}'::jsonb,
    request_id     text,
    created_at     timestamptz NOT NULL DEFAULT now(),
    prev_hash      bytea NOT NULL CHECK (length(prev_hash) = 32),
    hash           bytea NOT NULL CHECK (length(hash) = 32),
    UNIQUE (org_id, seq)
);

CREATE FUNCTION audit_events_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'audit_events is append-only' USING ERRCODE = 'insufficient_privilege';
END $$;
CREATE TRIGGER audit_events_no_update BEFORE UPDATE OR DELETE ON audit_events
    FOR EACH ROW EXECUTE FUNCTION audit_events_immutable();
CREATE TRIGGER audit_events_no_truncate BEFORE TRUNCATE ON audit_events
    FOR EACH STATEMENT EXECUTE FUNCTION audit_events_immutable();

ALTER TABLE audit_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_events FORCE ROW LEVEL SECURITY;
CREATE POLICY audit_events_tenant ON audit_events
    USING (org_id = app_org_id())
    WITH CHECK (org_id = app_org_id());

-- Users and sessions are global, but still not listable without context.
-- app.auth = 'on' is set only inside the server's authentication code paths
-- (login, session validation, token redemption) which look rows up by exact key.
CREATE FUNCTION app_auth_mode() RETURNS boolean LANGUAGE sql STABLE AS
$$ SELECT coalesce(current_setting('app.auth', true), '') = 'on' $$;

ALTER TABLE users ENABLE ROW LEVEL SECURITY;
ALTER TABLE users FORCE ROW LEVEL SECURITY;
CREATE POLICY users_visible ON users
    USING (app_auth_mode()
           OR id = app_user_id()
           OR id IN (SELECT m.user_id FROM memberships m
                     WHERE m.org_id = app_org_id() AND m.status IN ('active', 'deactivated'))
           OR id IN (SELECT a.user_id FROM service_assignments a WHERE a.org_id = app_org_id()));

ALTER TABLE sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE sessions FORCE ROW LEVEL SECURITY;
CREATE POLICY sessions_owner ON sessions
    USING (app_auth_mode() OR user_id = app_user_id());

ALTER TABLE user_tokens ENABLE ROW LEVEL SECURITY;
ALTER TABLE user_tokens FORCE ROW LEVEL SECURITY;
CREATE POLICY user_tokens_auth ON user_tokens
    USING (app_auth_mode() OR user_id IN (SELECT m.user_id FROM memberships m WHERE m.org_id = app_org_id()));

-- Organisations: a user sees the orgs they belong to and the org in context.
-- app.scope = 'platform' is set only after the server verified a platform permission
-- (e.g. listing/creating customer organisations).
CREATE FUNCTION app_platform_scope() RETURNS boolean LANGUAGE sql STABLE AS
$$ SELECT coalesce(current_setting('app.scope', true), '') = 'platform' $$;

ALTER TABLE organisations ENABLE ROW LEVEL SECURITY;
ALTER TABLE organisations FORCE ROW LEVEL SECURITY;
CREATE POLICY organisations_visible ON organisations
    USING (app_auth_mode()
           OR app_platform_scope()
           OR id = app_org_id()
           OR id IN (SELECT m.org_id FROM memberships m WHERE m.user_id = app_user_id()))
    WITH CHECK (app_platform_scope());

-- Runtime privileges. The server connects as a member of procure_runtime, which owns nothing:
-- it cannot alter tables, disable triggers or RLS, or update/delete audit events.
GRANT USAGE ON SCHEMA public TO procure_runtime;
GRANT SELECT, INSERT, UPDATE ON organisations, users, memberships, sessions, user_tokens, service_assignments
    TO procure_runtime;
GRANT SELECT, INSERT ON audit_events TO procure_runtime;
GRANT USAGE ON SEQUENCE audit_events_id_seq TO procure_runtime;
-- Lets `serve` check that the schema is up to date without being able to migrate.
GRANT SELECT ON _sqlx_migrations TO procure_runtime;
