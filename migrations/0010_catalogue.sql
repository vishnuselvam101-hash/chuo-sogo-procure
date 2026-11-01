-- Phase C: supplier organisations and their product listings, reviewed by Chuo-Sogo before
-- they appear in the catalogue.
--
-- Supplier members are external accounts (account_kind 'customer' means "not Chuo-Sogo
-- staff"); their roles exist only in supplier organisations.

ALTER TABLE organisations DROP CONSTRAINT organisations_kind_check;
ALTER TABLE organisations ADD CONSTRAINT organisations_kind_check
    CHECK (kind IN ('platform', 'customer', 'supplier'));

ALTER TABLE memberships DROP CONSTRAINT memberships_role_check;
ALTER TABLE memberships ADD CONSTRAINT memberships_role_check
    CHECK (role IN ('platform_super_admin', 'procurement_admin', 'procurement_operator',
                    'customer_admin', 'procurement_manager', 'requester', 'finance',
                    'supplier_admin', 'supplier_staff'));

-- Role ↔ organisation kind, and internal accounts only in the platform organisation.
CREATE OR REPLACE FUNCTION memberships_role_matches_kind() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE k text; ak text; prev text; role_kind text;
BEGIN
    prev := current_setting('app.auth', true);
    PERFORM set_config('app.auth', 'on', true);
    SELECT kind INTO k FROM organisations WHERE id = NEW.org_id;
    SELECT account_kind INTO ak FROM users WHERE id = NEW.user_id;
    PERFORM set_config('app.auth', coalesce(prev, ''), true);
    role_kind := CASE
        WHEN NEW.role IN ('platform_super_admin', 'procurement_admin', 'procurement_operator') THEN 'platform'
        WHEN NEW.role IN ('supplier_admin', 'supplier_staff') THEN 'supplier'
        ELSE 'customer' END;
    IF k IS DISTINCT FROM role_kind THEN
        RAISE EXCEPTION 'role % not allowed in % organisation', NEW.role, k USING ERRCODE = 'check_violation';
    END IF;
    IF (k = 'platform') <> (ak = 'internal') THEN
        RAISE EXCEPTION '% account not allowed in % organisation', ak, k USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END $$;

CREATE TABLE product_listings (
    id                    uuid PRIMARY KEY,
    -- The supplier organisation that owns the listing.
    org_id                uuid NOT NULL REFERENCES organisations (id),
    status                text NOT NULL DEFAULT 'draft'
                          CHECK (status IN ('draft', 'submitted', 'approved', 'rejected', 'retired')),
    manufacturer          text NOT NULL DEFAULT '' CHECK (length(manufacturer) <= 200),
    model_number          text NOT NULL DEFAULT '' CHECK (length(model_number) <= 100),
    -- Normalised model number (no case, spaces or punctuation) for duplicate checks.
    model_key             text NOT NULL DEFAULT '',
    -- Only for individual units (used, refurbished, one-off); catalogue products use the model.
    serial_number         text CHECK (serial_number IS NULL OR length(btrim(serial_number)) BETWEEN 1 AND 100),
    condition             text NOT NULL DEFAULT 'new' CHECK (condition IN ('new', 'used', 'refurbished')),
    title                 text NOT NULL DEFAULT '' CHECK (length(title) <= 200),
    title_en              text NOT NULL DEFAULT '' CHECK (length(title_en) <= 200),
    description           text NOT NULL DEFAULT '' CHECK (length(description) <= 5000),
    description_en        text NOT NULL DEFAULT '' CHECK (length(description_en) <= 5000),
    -- [{"key": "...", "value": "..."}], at most 50.
    specs                 jsonb NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(specs) = 'array'),
    currency              text NOT NULL DEFAULT 'JPY' CHECK (app_currency_ok(currency)),
    -- Business price, tax excluded; consumer price, tax included (総額表示). Either may be unset.
    business_price_minor  bigint CHECK (business_price_minor IS NULL OR business_price_minor >= 0),
    consumer_price_minor  bigint CHECK (consumer_price_minor IS NULL OR consumer_price_minor >= 0),
    tax_rate_bps          integer NOT NULL DEFAULT 1000 CHECK (tax_rate_bps IN (0, 800, 1000)),
    -- NULL: on request.
    stock                 integer CHECK (stock IS NULL OR stock BETWEEN 0 AND 1000000000),
    review_note           text NOT NULL DEFAULT '' CHECK (length(review_note) <= 2000),
    submitted_at          timestamptz,
    reviewed_by           uuid REFERENCES users (id),
    reviewed_at           timestamptz,
    published_at          timestamptz,
    created_by            uuid NOT NULL REFERENCES users (id),
    created_at            timestamptz NOT NULL DEFAULT now(),
    updated_at            timestamptz NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    -- A serial number identifies one unit.
    CHECK (serial_number IS NULL OR stock IS NULL OR stock <= 1),
    CHECK (status <> 'rejected' OR length(btrim(review_note)) > 0)
);

CREATE TABLE listing_media (
    id            uuid PRIMARY KEY,
    org_id        uuid NOT NULL,
    listing_id    uuid NOT NULL,
    kind          text NOT NULL CHECK (kind IN ('photo', 'document')),
    -- Photos: 1 = cover.
    position      integer NOT NULL CHECK (position BETWEEN 1 AND 10),
    file_name     text NOT NULL CHECK (length(btrim(file_name)) BETWEEN 1 AND 200),
    content_type  text NOT NULL CHECK (content_type IN ('image/jpeg', 'image/png', 'image/webp', 'application/pdf')),
    size_bytes    bigint NOT NULL CHECK (size_bytes BETWEEN 1 AND 10485760),
    sha256        bytea NOT NULL CHECK (length(sha256) = 32),
    width         integer,
    height        integer,
    created_by    uuid NOT NULL REFERENCES users (id),
    created_at    timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (listing_id, org_id) REFERENCES product_listings (id, org_id),
    UNIQUE (listing_id, kind, position),
    CHECK ((kind = 'photo') = (content_type LIKE 'image/%')),
    CHECK (kind <> 'photo' OR (width >= 1 AND height >= 1))
);

CREATE INDEX product_listings_org ON product_listings (org_id, updated_at DESC);
CREATE INDEX product_listings_status ON product_listings (status, submitted_at);
CREATE INDEX product_listings_model ON product_listings (model_key);
CREATE INDEX listing_media_listing ON listing_media (listing_id);

-- The owner sees and edits its listings; Chuo-Sogo (platform scope, after a moderation
-- permission check) sees all and records reviews; everyone else sees published ones only.
ALTER TABLE product_listings ENABLE ROW LEVEL SECURITY;
ALTER TABLE product_listings FORCE ROW LEVEL SECURITY;
CREATE POLICY product_listings_read ON product_listings FOR SELECT
    USING (org_id = app_org_id() OR app_platform_scope() OR status = 'approved');
CREATE POLICY product_listings_insert ON product_listings FOR INSERT
    WITH CHECK (org_id = app_org_id() AND status = 'draft');
CREATE POLICY product_listings_update ON product_listings FOR UPDATE
    USING (org_id = app_org_id() OR app_platform_scope())
    WITH CHECK (org_id = app_org_id() OR app_platform_scope());
CREATE POLICY product_listings_delete ON product_listings FOR DELETE
    USING (org_id = app_org_id() AND status = 'draft');

ALTER TABLE listing_media ENABLE ROW LEVEL SECURITY;
ALTER TABLE listing_media FORCE ROW LEVEL SECURITY;
CREATE POLICY listing_media_read ON listing_media FOR SELECT
    USING (org_id = app_org_id() OR app_platform_scope()
           OR listing_id IN (SELECT id FROM product_listings WHERE status = 'approved'));
CREATE POLICY listing_media_write ON listing_media FOR ALL
    USING (org_id = app_org_id()) WITH CHECK (org_id = app_org_id());

GRANT SELECT, INSERT, UPDATE, DELETE ON product_listings, listing_media TO procure_runtime;

-- A supplier with published listings is publicly named in the catalogue.
CREATE POLICY organisations_published_suppliers ON organisations FOR SELECT
    USING (kind = 'supplier' AND id IN (SELECT org_id FROM product_listings WHERE status = 'approved'));

-- The review workflow in the database itself: the owner can only take a listing back to
-- draft, submit a draft, or retire it; only a platform-scoped (moderation) transaction can
-- approve or reject, and the review fields are written only there.
CREATE FUNCTION product_listing_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE moderator boolean := app_platform_scope();
BEGIN
    IF OLD.status = 'retired' THEN
        RAISE EXCEPTION 'retired listings cannot change' USING ERRCODE = 'P0001';
    END IF;
    IF moderator THEN
        IF NOT ((OLD.status = 'submitted' AND NEW.status IN ('approved', 'rejected'))
                OR (OLD.status = 'approved' AND NEW.status = 'retired')) THEN
            RAISE EXCEPTION 'moderation cannot move % to %', OLD.status, NEW.status USING ERRCODE = 'P0001';
        END IF;
        -- Moderators never change the supplier's content.
        IF (to_jsonb(NEW) - ARRAY['status', 'review_note', 'reviewed_by', 'reviewed_at', 'published_at', 'updated_at'])
           IS DISTINCT FROM
           (to_jsonb(OLD) - ARRAY['status', 'review_note', 'reviewed_by', 'reviewed_at', 'published_at', 'updated_at']) THEN
            RAISE EXCEPTION 'moderation cannot change listing content' USING ERRCODE = 'P0001';
        END IF;
        RETURN NEW;
    END IF;
    IF NEW.status IS DISTINCT FROM OLD.status
       AND NOT (NEW.status = 'draft'
                OR (OLD.status = 'draft' AND NEW.status = 'submitted')
                OR NEW.status = 'retired') THEN
        RAISE EXCEPTION 'supplier cannot move % to %', OLD.status, NEW.status USING ERRCODE = 'P0001';
    END IF;
    IF NEW.review_note IS DISTINCT FROM OLD.review_note
       OR NEW.reviewed_by IS DISTINCT FROM OLD.reviewed_by
       OR NEW.reviewed_at IS DISTINCT FROM OLD.reviewed_at
       OR (NEW.published_at IS NOT NULL AND NEW.published_at IS DISTINCT FROM OLD.published_at) THEN
        RAISE EXCEPTION 'supplier cannot record reviews' USING ERRCODE = 'P0001';
    END IF;
    -- Anything changed on a published listing takes it out of the catalogue.
    IF OLD.status = 'approved' AND NEW.status = 'approved' THEN
        RAISE EXCEPTION 'published listings change only via draft' USING ERRCODE = 'P0001';
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER product_listings_guard BEFORE UPDATE ON product_listings
    FOR EACH ROW EXECUTE FUNCTION product_listing_guard();
