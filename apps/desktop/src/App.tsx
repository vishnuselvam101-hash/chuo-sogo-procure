import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { DEFAULT_LANGUAGE, isLanguage, setDocumentLanguage, type LanguageCode } from "./i18n";
import { useBridge } from "./lib/BridgeContext";
import { ok } from "./lib/bridge";
import { navFor, type PageId } from "./lib/nav";
import type { Me, MembershipView, Prefs } from "./lib/types";
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
      body = <Dashboard me={me} active={active} onChanged={() => void refreshMe()} />;
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
    case "billing":
      body = <Billing key={active.org_id} active={active} />;
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
        />
      );
      break;
    default:
      body = <Placeholder labelKey={current?.labelKey ?? HOME_LABEL} />;
  }

  return (
    <div className="shell">
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
      <main className="content">{body}</main>
    </div>
  );
}
