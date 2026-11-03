import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { DEFAULT_LANGUAGE, isLanguage, setDocumentLanguage, type LanguageCode } from "./i18n";
import { useBridge } from "./lib/BridgeContext";
import { ok } from "./lib/bridge";
import { navFor, type PageId } from "./lib/nav";
import type { Me, MembershipView, Prefs, TextSize } from "./lib/types";
import { applyDisplay, DEFAULT_TEXT_SIZE } from "./lib/display";
import { Sidebar } from "./components/Sidebar";
import { Login } from "./pages/Login";
import { Dashboard } from "./pages/Dashboard";
import { Members } from "./pages/Members";
import { Organisations } from "./pages/Organisations";
import { Audit } from "./pages/Audit";
import { Settings } from "./pages/Settings";
import { Placeholder } from "./pages/Placeholder";
import { Requests } from "./pages/Requests";
import { Suppliers } from "./pages/Suppliers";
import { Approvals } from "./pages/Approvals";
import { Products } from "./pages/Products";
import { OpsQueue } from "./pages/OpsQueue";
import { Billing } from "./pages/Billing";
import { Savings } from "./pages/Savings";
import { Records } from "./pages/Records";
import { MailInbox } from "./pages/MailInbox";
import { Listings } from "./pages/Listings";
import { Catalogue } from "./pages/Catalogue";
import { PurchaseHistory } from "./pages/PurchaseHistory";
import { LocalGate, ModeChoice } from "./pages/LocalMode";

const HOME: PageId = "dashboard";
const PROCUREMENT: PageId = "procurement";
const ORDERS_STAGE = "orders";
const FINANCE_STAGE = "finance";
const HOME_LABEL = "nav.dashboard";

/** Stand-in context while the user only has pending invitations. */
function noOrg(language: LanguageCode): MembershipView {
  return {
    org_id: "",
    org_name: "",
    org_kind: "customer",
    default_locale: language,
    timezone: "Asia/Tokyo",
    role: "",
    permissions: [],
    access: "member",
  };
}

/**
 * Display language: the user's explicit choice, else the active organisation's default,
 * else Japanese. Changing it only re-renders labels; no data is touched.
 */
export function resolveLanguage(
  pref: string | null | undefined,
  orgDefault: string | undefined,
): LanguageCode {
  if (isLanguage(pref)) return pref;
  if (isLanguage(orgDefault)) return orgDefault;
  return DEFAULT_LANGUAGE;
}

export function App() {
  const { i18n, t } = useTranslation();
  const bridge = useBridge();
  const [prefs, setPrefs] = useState<Prefs | null>(null);
  const [me, setMe] = useState<Me | null>(null);
  const [activeOrgId, setActiveOrgId] = useState<string | null>(null);
  const [page, setPage] = useState<PageId>(HOME);
  const [openRequest, setOpenRequest] = useState<string | null>(null);

  useEffect(() => {
    void bridge.prefsGet().then(setPrefs);
  }, [bridge]);

  const active = useMemo(
    () => me?.memberships.find((m) => m.org_id === activeOrgId) ?? me?.memberships[0] ?? null,
    [me, activeOrgId],
  );
  const language = resolveLanguage(prefs?.language, active?.default_locale);

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

  useEffect(() => {
    if (prefs) applyDisplay(prefs);
  }, [prefs]);

  const chooseDisplay = useCallback(
    async (textSize: TextSize, highContrast: boolean) => {
      await bridge.prefsSetDisplay(textSize, highContrast);
      setPrefs((p) => (p ? { ...p, text_size: textSize, high_contrast: highContrast } : p));
    },
    [bridge],
  );

  const refreshMe = useCallback(async () => {
    const res = await bridge.api<Me>("GET", "/api/v1/me");
    if (ok(res)) setMe(res.body);
    else if (res.status === 401) setMe(null);
  }, [bridge]);

  const signOut = useCallback(async () => {
    await bridge.logout();
    setMe(null);
    setActiveOrgId(null);
    setPage(HOME);
  }, [bridge]);

  if (!prefs) return null;
  // Fresh installation that can also run on its own: ask once where the data lives.
  if (prefs.mode === null)
    return <ModeChoice onChosen={(mode) => setPrefs((p) => (p ? { ...p, mode } : p))} />;
  if (!me && prefs.mode === "local")
    return <LocalGate>{(url) => <Login key={url} serverUrl={url} onLoggedIn={setMe} />}</LocalGate>;
  if (!me) return <Login serverUrl={prefs.server_url} onLoggedIn={setMe} />;

  if (!active) {
    // Signed in, but no active membership yet: only pending invitations.
    return (
      <main className="content">
        <Dashboard me={me} active={noOrg(language)} onChanged={() => void refreshMe()} />
      </main>
    );
  }

  const visible = navFor(active);
  const current = visible.find((n) => n.id === page) ?? visible[0];

  let body: React.ReactNode;
  switch (current?.implemented ? current.id : null) {
    case "dashboard":
      body = (
        <Dashboard
          me={me}
          active={active}
          onChanged={() => void refreshMe()}
          onNavigate={(p) => {
            setOpenRequest(null);
            setPage(p);
          }}
        />
      );
      break;
    case "procurement":
      body = (
        <Requests
          key={`${active.org_id}-${openRequest ?? ""}`}
          active={active}
          selfId={me.user.id}
          initialOpen={openRequest}
        />
      );
      break;
    case "suppliers":
      body = <Suppliers key={active.org_id} active={active} />;
      break;
    case "orders":
    case "invoices":
      body = (
        <Requests
          key={`${active.org_id}-${current?.id ?? ""}`}
          active={active}
          selfId={me.user.id}
          stage={current?.id === "orders" ? ORDERS_STAGE : FINANCE_STAGE}
          title={t(current?.labelKey ?? HOME_LABEL)}
        />
      );
      break;
    case "opsQueue":
      body = (
        <OpsQueue
          selfId={me.user.id}
          timezone={active.timezone}
          onOpen={(orgId, requestId) => {
            setActiveOrgId(orgId);
            setOpenRequest(requestId);
            setPage(PROCUREMENT);
          }}
        />
      );
      break;
    case "listings":
      body = <Listings key={active.org_id} active={active} />;
      break;
    case "catalogue":
      body = <Catalogue key={active.org_id} active={active} />;
      break;
    case "billing":
      body = <Billing key={active.org_id} active={active} />;
      break;
    case "savings":
      body = <Savings key={active.org_id} active={active} />;
      break;
    case "mailInbox":
      body = <MailInbox key={active.org_id} active={active} />;
      break;
    case "records":
      body = <Records key={active.org_id} active={active} />;
      break;
    case "history":
      body = <PurchaseHistory key={active.org_id} active={active} />;
      break;
    case "products":
      body = <Products key={active.org_id} active={active} />;
      break;
    case "approvals":
      body = <Approvals key={active.org_id} active={active} selfId={me.user.id} />;
      break;
    case "members":
      body = <Members active={active} selfId={me.user.id} />;
      break;
    case "organisations":
      body = <Organisations />;
      break;
    case "audit":
      body = <Audit active={active} />;
      break;
    case "settings":
      body = (
        <Settings
          language={language}
          explicit={isLanguage(prefs.language)}
          orgDefault={resolveLanguage(null, active.default_locale)}
          timezone={active.timezone}
          onLanguage={(c) => void chooseLanguage(c)}
          textSize={prefs.text_size ?? DEFAULT_TEXT_SIZE}
          highContrast={prefs.high_contrast ?? false}
          onDisplay={(size, contrast) => void chooseDisplay(size, contrast)}
          localMode={prefs.mode === "local"}
        />
      );
      break;
    default:
      body = <Placeholder labelKey={current?.labelKey ?? HOME_LABEL} />;
  }

  return (
    <div className="shell">
      <a className="skip-link" href="#main">
        {t("app.skip")}
      </a>
      <Sidebar
        memberships={me.memberships}
        active={active}
        page={current?.id ?? HOME}
        language={language}
        onNavigate={(p) => {
          setOpenRequest(null);
          setPage(p);
        }}
        onSwitchOrg={(id) => {
          setActiveOrgId(id);
          setPage(HOME);
        }}
        onLanguage={(c) => void chooseLanguage(c)}
        onSignOut={() => void signOut()}
      />
      <main className="content" id="main" tabIndex={-1}>
        {body}
      </main>
    </div>
  );
}
