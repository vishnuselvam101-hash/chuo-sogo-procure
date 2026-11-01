import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { DEFAULT_LANGUAGE, isLanguage, setDocumentLanguage, type LanguageCode } from "../i18n";
import { useBridge } from "../lib/BridgeContext";
import { errorCode, ok } from "../lib/bridge";
import type { Prefs } from "../lib/types";
import { LanguageMenu } from "../components/LanguageMenu";
import { Settings } from "../pages/Settings";
import { AdminLogin } from "./AdminLogin";
import { Overview } from "./Overview";
import { Orgs } from "./Orgs";
import { OrgDetail } from "./OrgDetail";
import { Accounts } from "./Accounts";
import { Billing } from "./Billing";
import { ADMIN_API, PERM, PLATFORM_TZ, type AdminMe } from "./types";

const SESSION_CHECK_MS = 60_000;

export type AdminPage = "overview" | "orgs" | "accounts" | "billing" | "settings";

/** Sections shown for the administrator's permissions; the server checks each call again. */
export function adminNav(permissions: string[]): AdminPage[] {
  const pages: AdminPage[] = [];
  if (permissions.includes(PERM.viewHealth)) pages.push("overview");
  if (permissions.includes(PERM.manageOrgs)) pages.push("orgs", "accounts");
  if (permissions.includes(PERM.manageBilling)) pages.push("billing");
  pages.push("settings");
  return pages;
}

/** The Chuo-Sogo admin console: staff only, separate app, separate API namespace. */
export function AdminApp() {
  const { i18n, t } = useTranslation();
  const bridge = useBridge();
  const [prefs, setPrefs] = useState<Prefs | null>(null);
  const [me, setMe] = useState<AdminMe | null>(null);
  const [page, setPage] = useState<AdminPage | null>(null);
  const [openOrg, setOpenOrg] = useState<string | null>(null);

  useEffect(() => {
    void bridge.prefsGet().then(setPrefs);
  }, [bridge]);

  const language: LanguageCode = isLanguage(prefs?.language) ? prefs.language : DEFAULT_LANGUAGE;
  useEffect(() => {
    if (i18n.language !== language) void i18n.changeLanguage(language);
    setDocumentLanguage(language);
  }, [i18n, language]);

  const chooseLanguage = useCallback(
    async (code: LanguageCode | null) => {
      await bridge.prefsSetLanguage(code);
      setPrefs((p) => (p ? { ...p, language: code } : p));
    },
    [bridge],
  );

  /** After the code: load the administrator, or end the new session and report why. */
  const loadMe = useCallback(async (): Promise<string | null> => {
    const res = await bridge.api<AdminMe>("GET", `${ADMIN_API}/me`);
    if (ok(res)) {
      setMe(res.body);
      return null;
    }
    await bridge.logout();
    return errorCode(res);
  }, [bridge]);

  // Admin sessions end after 30 minutes idle (server side): notice it and return to sign-in.
  const signedIn = me !== null;
  useEffect(() => {
    if (!signedIn) return;
    const id = window.setInterval(() => {
      void bridge.api("GET", `${ADMIN_API}/me`).then((res) => {
        if (res.status === 401) setMe(null);
      });
    }, SESSION_CHECK_MS);
    return () => window.clearInterval(id);
  }, [bridge, signedIn]);

  const signOut = useCallback(async () => {
    await bridge.logout();
    setMe(null);
    setPage(null);
    setOpenOrg(null);
  }, [bridge]);

  if (!prefs) return null;
  if (!me) return <AdminLogin serverUrl={prefs.server_url} onSignedIn={loadMe} />;

  const nav = adminNav(me.permissions);
  const current = page && nav.includes(page) ? page : (nav[0] ?? "settings");
  const open = (orgId: string) => {
    setOpenOrg(orgId);
    setPage("orgs");
  };

  let body: React.ReactNode;
  if (current === "orgs" && openOrg)
    body = (
      <OrgDetail key={openOrg} orgId={openOrg} permissions={me.permissions} onBack={() => setOpenOrg(null)} />
    );
  else if (current === "orgs") body = <Orgs canCreate onOpen={open} />;
  else if (current === "billing") body = <Billing />;
  else if (current === "overview") body = <Overview onOpenOrg={open} />;
  else if (current === "accounts")
    body = <Accounts selfId={me.user.id} canManage={me.permissions.includes(PERM.manageInternalUsers)} />;
  else
    body = (
      <Settings
        language={language}
        explicit={isLanguage(prefs.language)}
        orgDefault={DEFAULT_LANGUAGE}
        timezone={PLATFORM_TZ}
        onLanguage={(c) => void chooseLanguage(c)}
      />
    );

  return (
    <div className="shell admin">
      <nav className="sidebar" aria-label={t("admin:app.name")}>
        <div className="brand">
          <div className="brand-name">{t("admin:app.name")}</div>
          <div className="brand-workspace">{t(`role.${me.role}`, { defaultValue: me.role })}</div>
        </div>
        <div className="org-name">{me.user.display_name}</div>
        {nav.length === 1 && <p className="muted small">{t("admin:noSections")}</p>}
        <ul className="nav-list">
          {nav.map((id) => (
            <li key={id}>
              <button
                type="button"
                className={`nav-item${current === id ? " active" : ""}`}
                aria-current={current === id ? "page" : undefined}
                onClick={() => {
                  setOpenOrg(null);
                  setPage(id);
                }}
              >
                {t(`admin:nav.${id}`)}
              </button>
            </li>
          ))}
        </ul>
        <div className="sidebar-footer">
          <LanguageMenu current={language} onChange={(c) => void chooseLanguage(c)} />
          <button type="button" className="nav-item" onClick={() => void signOut()}>
            {t("auth.signOut")}
          </button>
        </div>
      </nav>
      <main className="content">{body}</main>
    </div>
  );
}
