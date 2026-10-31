import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useBridge } from "../lib/BridgeContext";
import { errorCode, ok } from "../lib/bridge";
import type { Me, MembershipView } from "../lib/types";
import { ErrorText, PageHeader } from "../components/ui";

interface Props {
  me: Me;
  active: MembershipView;
  onChanged: () => void;
}

export function Dashboard({ me, active, onChanged }: Props) {
  const { t } = useTranslation();
  const bridge = useBridge();
  const [error, setError] = useState<string | null>(null);

  async function respond(orgId: string, answer: "accept" | "decline") {
    const res = await bridge.api("POST", `/api/v1/invitations/${orgId}/${answer}`);
    if (ok(res)) onChanged();
    else setError(errorCode(res));
  }

  return (
    <section>
      <PageHeader title={t("dashboard.welcome", { name: me.user.display_name })} />
      <div className="card">
        <div className="muted">{active.org_name}</div>
        <div>{t("dashboard.role", { role: t(`role.${active.role}`) })}</div>
      </div>
      {me.invitations.length > 0 && (
        <div className="card">
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
    </section>
  );
}
