import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "../lib/api-schema";
import { formatDateTime } from "../lib/format";
import { P } from "../lib/perm";
import type { MembershipView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { ErrorText } from "../components/ui";

type OrderEmailView = components["schemas"]["OrderEmailView"];
const DRAFT = "draft";

/** The order email to the chosen supplier: our staff read it, tick the check and send it. */
export function OrderEmail({ active, orderId }: { active: MembershipView; orderId: string }) {
  const { t, i18n } = useTranslation();
  const org = `/api/v1/orgs/${active.org_id}`;
  const { data, error, reload } = useApi<OrderEmailView>(`${org}/orders/${orderId}/order-email`);
  const action = useAction();
  const [checked, setChecked] = useState(false);
  const [open, setOpen] = useState(false);
  if (!data) return error ? <ErrorText code={error} /> : null;
  const staff = active.access === "assigned" && active.permissions.includes(P.operateRequests);
  const draft = data.status === DRAFT;
  return (
    <div className="stack">
      <div className="row small">
        <strong>{t("procurement:orderEmail.heading")}</strong>
        <span className="badge">{t(`procurement:orderEmail.status.${data.status}`)}</span>
        {data.checked_by_name && (
          <span className="muted">
            {t("procurement:orderEmail.checkedBy", { name: data.checked_by_name })}
          </span>
        )}
        {data.created_at && (
          <span className="muted">{formatDateTime(data.created_at, i18n.language, active.timezone)}</span>
        )}
        <button type="button" onClick={() => setOpen(!open)}>
          {open ? t("procurement:orderEmail.hide") : t("procurement:orderEmail.show")}
        </button>
      </div>
      {open && <pre className="pre">{data.body}</pre>}
      {draft && staff && (
        <div className="row">
          <label className="row">
            <input type="checkbox" checked={checked} onChange={(e) => setChecked(e.target.checked)} />
            {t("procurement:orderEmail.confirm")}
          </label>
          <button
            type="button"
            className="primary"
            disabled={!checked || action.busy}
            onClick={() =>
              void action
                .run("POST", `${org}/orders/${orderId}/order-email/send`, { checked: true })
                .then((r) => r && reload())
            }
          >
            {t("procurement:orderEmail.send")}
          </button>
        </div>
      )}
      {draft && !staff && <p className="muted small">{t("procurement:orderEmail.waiting")}</p>}
      {data.status === "recorded" && <p className="muted small">{t("procurement:orderEmail.testMode")}</p>}
      <ErrorText code={action.error} />
    </div>
  );
}
