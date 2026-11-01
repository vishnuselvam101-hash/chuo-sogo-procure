-- Phase A: admin console. Sessions know which app they belong to; admin sessions require a
-- second factor (TOTP). Platform-scoped reads of users for account search. Per-organisation
-- rules editable by Chuo-Sogo administrators.

-- 'app' = customer/business apps (/api/v1), 'admin' = admin console (/api/admin/v1).
ALTER TABLE sessions ADD COLUMN audience text NOT NULL DEFAULT 'app'
    CHECK (audience IN ('app', 'admin'));
-- When the second factor was verified for this session (required for 'admin').
ALTER TABLE sessions ADD COLUMN mfa_at timestamptz;
ALTER TABLE sessions ADD CONSTRAINT sessions_admin_mfa CHECK (audience <> 'admin' OR mfa_at IS NOT NULL);

-- TOTP second factor. The secret is encrypted by the server (AES-256-GCM, key outside the
-- database); `totp_last_step` prevents reusing a code within its time window.
ALTER TABLE users ADD COLUMN totp_secret_enc bytea;
ALTER TABLE users ADD COLUMN totp_enabled_at timestamptz;
ALTER TABLE users ADD COLUMN totp_last_step bigint;
ALTER TABLE users ADD CONSTRAINT users_totp_enabled CHECK (totp_enabled_at IS NULL OR totp_secret_enc IS NOT NULL);

-- One-time tokens for the steps between password and second factor.
ALTER TABLE user_tokens DROP CONSTRAINT user_tokens_purpose_check;
ALTER TABLE user_tokens ADD CONSTRAINT user_tokens_purpose_check
    CHECK (purpose IN ('activation', 'password_reset', 'mfa_challenge', 'totp_enrollment'));

-- Account search in the admin console: only after the server verified a platform permission
-- and set app.scope = 'platform' (as for organisations).
DROP POLICY users_visible ON users;
CREATE POLICY users_visible ON users
    USING (app_auth_mode()
           OR app_platform_scope()
           OR id = app_user_id()
           OR id IN (SELECT m.user_id FROM memberships m
                     WHERE m.org_id = app_org_id() AND m.status IN ('active', 'deactivated'))
           OR id IN (SELECT a.user_id FROM service_assignments a WHERE a.org_id = app_org_id()));

-- Per-organisation rules (defaults apply when no row exists).
CREATE TABLE org_settings (
    org_id                uuid PRIMARY KEY REFERENCES organisations (id),
    identify_sla_hours    integer NOT NULL DEFAULT 24 CHECK (identify_sla_hours BETWEEN 1 AND 720),
    chase_default_hours   integer NOT NULL DEFAULT 72 CHECK (chase_default_hours BETWEEN 1 AND 2160),
    ai_enabled            boolean NOT NULL DEFAULT true,
    default_tax_rate_bps  integer NOT NULL DEFAULT 1000 CHECK (default_tax_rate_bps BETWEEN 0 AND 10000),
    updated_by            uuid NOT NULL REFERENCES users (id),
    updated_at            timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE org_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE org_settings FORCE ROW LEVEL SECURITY;
CREATE POLICY org_settings_tenant ON org_settings
    USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id());

GRANT SELECT, INSERT, UPDATE ON org_settings TO procure_runtime;
