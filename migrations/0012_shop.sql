-- Phase E: consumer shop. Individuals sign up (each gets a personal organisation of kind
-- 'consumer', so the existing tenant isolation applies), order published listings at
-- tax-inclusive prices from Chuo-Sogo, and pay by bank transfer against the invoice issued
-- when Chuo-Sogo confirms the order.

ALTER TABLE organisations DROP CONSTRAINT organisations_kind_check;
ALTER TABLE organisations ADD CONSTRAINT organisations_kind_check
    CHECK (kind IN ('platform', 'customer', 'supplier', 'consumer'));

ALTER TABLE memberships DROP CONSTRAINT memberships_role_check;
ALTER TABLE memberships ADD CONSTRAINT memberships_role_check
    CHECK (role IN ('platform_super_admin', 'procurement_admin', 'procurement_operator',
                    'customer_admin', 'procurement_manager', 'requester', 'finance',
                    'supplier_admin', 'supplier_staff', 'consumer'));

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
        WHEN NEW.role = 'consumer' THEN 'consumer'
        ELSE 'customer' END;
    IF k IS DISTINCT FROM role_kind THEN
        RAISE EXCEPTION 'role % not allowed in % organisation', NEW.role, k USING ERRCODE = 'check_violation';
    END IF;
    IF (k = 'platform') <> (ak = 'internal') THEN
        RAISE EXCEPTION '% account not allowed in % organisation', ak, k USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END $$;

-- Stock is reserved when Chuo-Sogo confirms a consumer order and returned when a confirmed
-- order is cancelled (see shop.rs); the listing guard allows exactly that change.
CREATE OR REPLACE FUNCTION product_listing_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE moderator boolean := app_platform_scope();
BEGIN
    -- Shop staff reserve and return stock for consumer orders: a stock-only change in a
    -- platform-scoped transaction, never below zero.
    IF moderator AND NEW.stock IS DISTINCT FROM OLD.stock AND NEW.stock >= 0
       AND (to_jsonb(NEW) - ARRAY['stock', 'updated_at']) = (to_jsonb(OLD) - ARRAY['stock', 'updated_at']) THEN
        RETURN NEW;
    END IF;
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

-- Invoices whose unit prices include tax (consumer sales).
ALTER TABLE issued_invoices ADD COLUMN prices_include_tax boolean NOT NULL DEFAULT false;

-- Shop rules and the seller notice required by the Act on Specified Commercial
-- Transactions (特定商取引法に基づく表記). The shop accepts orders only when open.
CREATE TABLE shop_settings (
    org_id                    uuid PRIMARY KEY REFERENCES organisations (id),
    open                      boolean NOT NULL DEFAULT false,
    shipping_fee_minor        bigint NOT NULL DEFAULT 0 CHECK (shipping_fee_minor >= 0),
    free_shipping_over_minor  bigint CHECK (free_shipping_over_minor IS NULL OR free_shipping_over_minor >= 0),
    payment_days              integer NOT NULL DEFAULT 7 CHECK (payment_days BETWEEN 1 AND 60),
    seller_name               text NOT NULL DEFAULT '' CHECK (length(seller_name) <= 200),
    responsible_person        text NOT NULL DEFAULT '' CHECK (length(responsible_person) <= 200),
    seller_address            text NOT NULL DEFAULT '' CHECK (length(seller_address) <= 500),
    seller_phone              text NOT NULL DEFAULT '' CHECK (length(seller_phone) <= 50),
    seller_email              text NOT NULL DEFAULT '' CHECK (length(seller_email) <= 200),
    price_info                text NOT NULL DEFAULT '' CHECK (length(price_info) <= 2000),
    payment_info              text NOT NULL DEFAULT '' CHECK (length(payment_info) <= 2000),
    delivery_info             text NOT NULL DEFAULT '' CHECK (length(delivery_info) <= 2000),
    returns_policy            text NOT NULL DEFAULT '' CHECK (length(returns_policy) <= 4000),
    privacy_policy            text NOT NULL DEFAULT '' CHECK (length(privacy_policy) <= 8000),
    updated_by                uuid NOT NULL REFERENCES users (id),
    updated_at                timestamptz NOT NULL DEFAULT now(),
    -- Open only with the legally required notice filled in.
    CHECK (NOT open OR (length(btrim(seller_name)) > 0 AND length(btrim(responsible_person)) > 0
                        AND length(btrim(seller_address)) > 0 AND length(btrim(seller_phone)) > 0
                        AND length(btrim(seller_email)) > 0 AND length(btrim(payment_info)) > 0
                        AND length(btrim(delivery_info)) > 0 AND length(btrim(returns_policy)) > 0
                        AND length(btrim(privacy_policy)) > 0))
);

CREATE TABLE shop_orders (
    id                  uuid PRIMARY KEY,
    -- The consumer's personal organisation.
    org_id              uuid NOT NULL REFERENCES organisations (id),
    number              text NOT NULL UNIQUE,
    status              text NOT NULL DEFAULT 'placed'
                        CHECK (status IN ('placed', 'confirmed', 'paid', 'shipped', 'completed', 'cancelled')),
    ship_name           text NOT NULL CHECK (length(btrim(ship_name)) BETWEEN 1 AND 100),
    postal_code         text NOT NULL CHECK (postal_code ~ '^[0-9]{3}-?[0-9]{4}$'),
    prefecture          text NOT NULL CHECK (length(btrim(prefecture)) BETWEEN 2 AND 10),
    address_line        text NOT NULL CHECK (length(btrim(address_line)) BETWEEN 1 AND 300),
    phone               text NOT NULL CHECK (phone ~ '^[0-9+() -]{6,20}$'),
    note                text NOT NULL DEFAULT '' CHECK (length(note) <= 1000),
    currency            text NOT NULL DEFAULT 'JPY' CHECK (currency = 'JPY'),
    items_total_minor   bigint NOT NULL CHECK (items_total_minor >= 0),
    shipping_minor      bigint NOT NULL CHECK (shipping_minor >= 0),
    total_minor         bigint NOT NULL CHECK (total_minor >= 0),
    tax_minor           bigint NOT NULL CHECK (tax_minor >= 0),
    invoice_id          uuid REFERENCES issued_invoices (id),
    -- Stock reserved at confirmation, returned on cancellation: [{"listing_id", "quantity"}].
    reserved            jsonb NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(reserved) = 'array'),
    credit_note_id      uuid REFERENCES issued_invoices (id),
    carrier             text NOT NULL DEFAULT '' CHECK (length(carrier) <= 100),
    tracking_number     text NOT NULL DEFAULT '' CHECK (length(tracking_number) <= 100),
    cancel_reason       text NOT NULL DEFAULT '' CHECK (length(cancel_reason) <= 1000),
    placed_by           uuid NOT NULL REFERENCES users (id),
    placed_at           timestamptz NOT NULL DEFAULT now(),
    confirmed_at        timestamptz,
    paid_at             timestamptz,
    shipped_at          timestamptz,
    completed_at        timestamptz,
    cancelled_at        timestamptz,
    updated_at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    CHECK (status IN ('placed', 'cancelled') OR invoice_id IS NOT NULL),
    CHECK (total_minor = items_total_minor + shipping_minor)
);

CREATE TABLE shop_order_lines (
    id                uuid PRIMARY KEY,
    org_id            uuid NOT NULL,
    order_id          uuid NOT NULL,
    position          integer NOT NULL CHECK (position BETWEEN 1 AND 50),
    listing_id        uuid NOT NULL REFERENCES product_listings (id),
    -- Snapshots at ordering time (the listing may change later).
    supplier_org_id   uuid NOT NULL REFERENCES organisations (id),
    title             text NOT NULL,
    model_number      text NOT NULL,
    unit_price_minor  bigint NOT NULL CHECK (unit_price_minor >= 0),
    tax_rate_bps      integer NOT NULL CHECK (tax_rate_bps IN (0, 800, 1000)),
    quantity          integer NOT NULL CHECK (quantity BETWEEN 1 AND 1000),
    FOREIGN KEY (order_id, org_id) REFERENCES shop_orders (id, org_id),
    UNIQUE (order_id, position)
);

CREATE INDEX shop_orders_org ON shop_orders (org_id, placed_at DESC);
CREATE INDEX shop_orders_status ON shop_orders (status, placed_at);
CREATE INDEX shop_order_lines_order ON shop_order_lines (order_id);

-- Order lifecycle in the database: consumers can only withdraw a placed order; every other
-- change needs a platform-scoped (shop staff) transaction and follows the lifecycle.
CREATE FUNCTION shop_order_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    staff boolean := app_platform_scope();
    allowed boolean;
BEGIN
    IF NEW.status = OLD.status THEN
        -- Same state: only shop staff may change details (tracking, invoice references).
        IF NOT staff THEN
            RAISE EXCEPTION 'order % cannot be changed', OLD.number USING ERRCODE = 'P0001';
        END IF;
        RETURN NEW;
    END IF;
    allowed := CASE
        WHEN NOT staff THEN OLD.status = 'placed' AND NEW.status = 'cancelled'
        ELSE (OLD.status, NEW.status) IN (('placed', 'confirmed'), ('confirmed', 'paid'),
                                          ('paid', 'shipped'), ('shipped', 'completed'),
                                          ('placed', 'cancelled'), ('confirmed', 'cancelled'),
                                          ('paid', 'cancelled'))
    END;
    IF NOT allowed THEN
        RAISE EXCEPTION 'order % cannot move from % to %', OLD.number, OLD.status, NEW.status
            USING ERRCODE = 'P0001';
    END IF;
    -- Consumers change nothing but the state (and its time and reason).
    IF NOT staff AND (to_jsonb(NEW) - ARRAY['status', 'cancelled_at', 'cancel_reason', 'updated_at'])
                     IS DISTINCT FROM
                     (to_jsonb(OLD) - ARRAY['status', 'cancelled_at', 'cancel_reason', 'updated_at']) THEN
        RAISE EXCEPTION 'order % cannot be changed', OLD.number USING ERRCODE = 'P0001';
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER shop_orders_guard BEFORE UPDATE ON shop_orders
    FOR EACH ROW EXECUTE FUNCTION shop_order_guard();

-- Lines are written once, with their order.
CREATE FUNCTION shop_order_line_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'order lines are immutable' USING ERRCODE = 'P0001';
END $$;
CREATE TRIGGER shop_order_lines_guard BEFORE UPDATE OR DELETE ON shop_order_lines
    FOR EACH ROW EXECUTE FUNCTION shop_order_line_guard();

ALTER TABLE shop_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE shop_settings FORCE ROW LEVEL SECURITY;
-- Everyone may read the shop rules and seller notice; only shop staff write them.
CREATE POLICY shop_settings_read ON shop_settings FOR SELECT USING (true);
CREATE POLICY shop_settings_write ON shop_settings FOR INSERT
    WITH CHECK (app_platform_scope() AND org_id = app_org_id());
CREATE POLICY shop_settings_update ON shop_settings FOR UPDATE
    USING (app_platform_scope() AND org_id = app_org_id())
    WITH CHECK (app_platform_scope() AND org_id = app_org_id());

ALTER TABLE shop_orders ENABLE ROW LEVEL SECURITY;
ALTER TABLE shop_orders FORCE ROW LEVEL SECURITY;
CREATE POLICY shop_orders_read ON shop_orders FOR SELECT
    USING (org_id = app_org_id() OR app_platform_scope());
CREATE POLICY shop_orders_insert ON shop_orders FOR INSERT
    WITH CHECK (org_id = app_org_id() AND status = 'placed');
CREATE POLICY shop_orders_update ON shop_orders FOR UPDATE
    USING (org_id = app_org_id() OR app_platform_scope())
    WITH CHECK (org_id = app_org_id() OR app_platform_scope());

ALTER TABLE shop_order_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE shop_order_lines FORCE ROW LEVEL SECURITY;
CREATE POLICY shop_order_lines_read ON shop_order_lines FOR SELECT
    USING (org_id = app_org_id() OR app_platform_scope());
CREATE POLICY shop_order_lines_insert ON shop_order_lines FOR INSERT
    WITH CHECK (org_id = app_org_id());

GRANT SELECT, INSERT, UPDATE ON shop_settings, shop_orders TO procure_runtime;
GRANT SELECT, INSERT ON shop_order_lines TO procure_runtime;
