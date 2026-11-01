-- Staff can read the name/locale/timezone of customer organisations they are actively
-- assigned to (needed to list their work), nothing more: every other table stays limited
-- to the organisation in context.
DROP POLICY organisations_visible ON organisations;
CREATE POLICY organisations_visible ON organisations
    USING (app_auth_mode()
           OR app_platform_scope()
           OR id = app_org_id()
           OR id IN (SELECT m.org_id FROM memberships m WHERE m.user_id = app_user_id())
           OR id IN (SELECT a.org_id FROM service_assignments a
                     WHERE a.user_id = app_user_id() AND a.revoked_at IS NULL))
    WITH CHECK (app_platform_scope());
