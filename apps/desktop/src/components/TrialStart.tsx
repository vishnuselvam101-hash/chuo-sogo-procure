import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useBridge } from "../lib/BridgeContext";
import { errorCode, ok } from "../lib/bridge";
import type { Me } from "../lib/types";
import { ErrorText } from "./ui";

export interface TrialAccount {
  email: string;
  display_name: string;
  role: string;
  org_name: string;
  org_kind: string;
}

// The order a visitor walks through the trial: the customer's own roles, then Chuo-Sogo's
// buyer, then the supplier.
const KIND_ORDER = ["customer", "platform", "supplier", "consumer"];
const ROLE_ORDER = [
  "customer_admin",
  "procurement_manager",
  "requester",
  "finance",
  "procurement_operator",
  "procurement_admin",
  "supplier_admin",
  "supplier_staff",
  "consumer",
];
const rank = (a: TrialAccount) => KIND_ORDER.indexOf(a.org_kind) * 100 + ROLE_ORDER.indexOf(a.role);

/**
 * Trial accounts offered by the server (only a trial server answers; anywhere else the list
 * is empty and the normal sign-in form shows). `undefined` while asking.
 */
export function useTrialAccounts(app: "business" | "shop"): TrialAccount[] | undefined {
  const bridge = useBridge();
  const [accounts, setAccounts] = useState<TrialAccount[] | undefined>(undefined);
  useEffect(() => {
    let live = true;
    bridge
      .api<TrialAccount[]>("GET", "/api/v1/demo/accounts")
      .then((res) => {
        const list = ok(res) && Array.isArray(res.body) ? res.body : [];
        const mine = list.filter((a) => (app === "shop") === (a.org_kind === "consumer"));
        if (live) setAccounts([...mine].sort((a, b) => rank(a) - rank(b)));
      })
      .catch(() => live && setAccounts([]));
    return () => {
      live = false;
    };
  }, [bridge, app]);
  return accounts;
}

/** Trial server: one button per role, no password. */
export function TrialStart({
  title,
  accounts,
  onSignedIn,
  onUseForm,
}: {
  title: string;
  accounts: TrialAccount[];
  onSignedIn: (me: Me) => void;
  onUseForm: () => void;
}) {
  const { t } = useTranslation();
  const bridge = useBridge();
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function enter(email: string) {
    setBusy(email);
    setError(null);
    try {
      const res = await bridge.demoLogin(email);
      if (ok(res)) onSignedIn(res.body);
      else setError(errorCode(res));
    } catch (err) {
      setError(typeof err === "string" ? err : "unknown");
    } finally {
      setBusy(null);
    }
  }

  return (
    <main className="login">
      <div className="card login-card trial-card" data-testid="trial-start">
        <h1>{title}</h1>
        <p className="muted small">{t("trial.lead")}</p>
        <ul className="trial-roles">
          {accounts.map((a) => (
            <li key={a.email}>
              <button
                type="button"
                className="trial-role"
                disabled={busy !== null}
                onClick={() => void enter(a.email)}
              >
                <span className="trial-role-name">{t(`role.${a.role}`, { defaultValue: a.role })}</span>
                <span className="trial-role-org">{a.org_name}</span>
              </button>
            </li>
          ))}
        </ul>
        <ErrorText code={error} />
        <p className="muted small">{t("trial.note")}</p>
        <button type="button" className="link" onClick={onUseForm}>
          {t("trial.useEmail")}
        </button>
      </div>
    </main>
  );
}
