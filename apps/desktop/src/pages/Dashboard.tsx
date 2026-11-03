import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useBridge } from "../lib/BridgeContext";
import { errorCode, ok } from "../lib/bridge";
import { navFor, type PageId } from "../lib/nav";
import type { Me, MembershipView } from "../lib/types";
import { ErrorText } from "../components/ui";
import { DashboardHero } from "../components/DashboardHero";

interface Props {
  me: Me;
  active: MembershipView;
  onChanged: () => void;
  /** Shortcuts to the pages this role can open; omitted while there is no organisation. */
  onNavigate?: (page: PageId) => void;
}

const NOT_SHORTCUTS: PageId[] = ["dashboard", "settings"];
const MAX_SHORTCUTS = 6;

export function Dashboard({ me, active, onChanged, onNavigate }: Props) {
  const { t } = useTranslation();
  const bridge = useBridge();
  const [error, setError] = useState<string | null>(null);
  const shortcuts = onNavigate
    ? navFor(active)
        .filter((n) => n.implemented && !NOT_SHORTCUTS.includes(n.id))
        .slice(0, MAX_SHORTCUTS)
    : [];

  async function respond(orgId: string, answer: "accept" | "decline") {
    const res = await bridge.api("POST", `/api/v1/invitations/${orgId}/${answer}`);
    if (ok(res)) onChanged();
    else setError(errorCode(res));
  }

  return (
    <section>
      <DashboardHero>
        {active.org_name && (
          <p className="hero-eyebrow">
            <span className="muted">{t("dashboard.workspace")}</span> <span>{active.org_name}</span>
          </p>
        )}
        <h1>{t("dashboard.welcome", { name: me.user.display_name })}</h1>
        {active.role && (
          <p className="hero-role">{t("dashboard.role", { role: t(`role.${active.role}`) })}</p>
        )}
      </DashboardHero>
      {me.invitations.length > 0 && (
        <div className="card">
          <h2>{t("dashboard.invitations")}</h2>
          <ul className="invitation-list">
            {me.invitations.map((inv) => (
              <li key={inv.org_id}>
                <span>
                  {inv.org_name} — {t(`role.${inv.role}`)}
                </span>
                <span className="row-actions">
                  <button
                    type="button"
                    className="primary"
                    onClick={() => void respond(inv.org_id, "accept")}
                  >
                    {t("invitation.accept")}
                  </button>
                  <button type="button" onClick={() => void respond(inv.org_id, "decline")}>
                    {t("invitation.decline")}
                  </button>
                </span>
              </li>
            ))}
          </ul>
          <ErrorText code={error} />
        </div>
      )}
      {shortcuts.length > 0 && onNavigate && (
        <section className="start-here" aria-labelledby="start-here-title">
          <h2 id="start-here-title">{t("dashboard.startHere")}</h2>
          <p className="muted">{t("dashboard.startHint")}</p>
          <ul className="shortcut-grid">
            {shortcuts.map((n, i) => (
              <li key={n.id}>
                <button type="button" className="shortcut" onClick={() => onNavigate(n.id)}>
                  <span className="shortcut-index" aria-hidden="true">
                    {String(i + 1).padStart(2, "0")}
                  </span>
                  <span className="shortcut-label">{t(n.labelKey)}</span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}
    </section>
  );
}
