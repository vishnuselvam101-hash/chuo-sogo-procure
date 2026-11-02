-- Item names written differently for the same thing ("6204ZZ" / "6204-2Z", "深溝玉軸受 6204" /
-- "ベアリング6204ZZ"). A person confirms that two matching keys (procure_domain::history::item_key)
-- are the same item; the price reference and the quote comparison then treat them as one.
-- One level only: a canonical key is never itself an alias (enforced by the API in the same
-- transaction that writes the row).

CREATE TABLE item_aliases (
    id             uuid PRIMARY KEY,
    org_id         uuid NOT NULL REFERENCES organisations (id),
    alias_key      text NOT NULL CHECK (alias_key ~ '^[md]:.+' AND length(alias_key) <= 1100),
    canonical_key  text NOT NULL CHECK (canonical_key ~ '^[md]:.+' AND length(canonical_key) <= 1100),
    note           text NOT NULL DEFAULT '' CHECK (length(note) <= 500),
    created_by     uuid NOT NULL REFERENCES users (id),
    created_at     timestamptz NOT NULL DEFAULT now(),
    CHECK (alias_key <> canonical_key),
    UNIQUE (org_id, alias_key)
);
CREATE INDEX item_aliases_canonical ON item_aliases (org_id, canonical_key);

ALTER TABLE item_aliases ENABLE ROW LEVEL SECURITY;
ALTER TABLE item_aliases FORCE ROW LEVEL SECURITY;
CREATE POLICY item_aliases_tenant ON item_aliases
    USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id());

GRANT SELECT, INSERT, DELETE ON item_aliases TO procure_runtime;
