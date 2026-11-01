import { useState } from "react";
import { useTranslation } from "react-i18next";
import { formatDateTime } from "../lib/format";
import type { MembershipView, RequestSummary } from "../lib/types";
import { useApi } from "../lib/useApi";
import { Loading, PageHeader, StatusBadge } from "../components/ui";
import { NewRequest } from "./NewRequest";
import { RequestDetail } from "./RequestDetail";
import { P } from "../lib/perm";

const STATUSES = [
  "draft",
  "submitted",
  "needs_clarification",
  "identifying",
  "sourcing",
  "rfq_sent",
  "quotes_received",
  "under_comparison",
  "recommended",
  "awaiting_approval",
  "approved",
  "rejected",
  "cancelled",
];

interface Props {
  active: MembershipView;
  selfId: string;
  /** Fixed status filter (e.g. the Approvals page shows only `awaiting_approval`). */
  status?: string;
  title?: string;
  /** Open this request immediately (e.g. from the operations queue). */
  initialOpen?: string | null;
}

/** Request list for customers (own or all, as the backend decides) and assigned operators. */
export function Requests({ active, selfId, status, title, initialOpen }: Props) {
  const { t, i18n } = useTranslation();
  const [open, setOpen] = useState<string | null>(initialOpen ?? null);
  const [creating, setCreating] = useState(false);
  const [filter, setFilter] = useState(status ?? "");
  const query = filter ? `?status=${encodeURIComponent(filter)}` : "";
  const base = `/api/v1/orgs/${active.org_id}/requests`;
  const { data, error, reload } = useApi<RequestSummary[]>(`${base}${query}`);
  const canCreate = active.permissions.includes(P.submitRequest);

  if (creating)
    return (
      <NewRequest
        active={active}
        onCancel={() => setCreating(false)}
        onCreated={(id) => {
          setCreating(false);
          setOpen(id);
          reload();
        }}
      />
    );
  if (open)
    return (
      <RequestDetail
        active={active}
        selfId={selfId}
        requestId={open}
        onBack={() => {
          setOpen(null);
          reload();
        }}
      />
    );

  return (
    <section>
      <PageHeader title={title ?? t("nav.procurement")}>
        {!status && (
          <label className="compact">
            <span className="visually-hidden">{t("procurement:list.filter")}</span>
            <select value={filter} onChange={(e) => setFilter(e.target.value)}>
              <option value="">{t("procurement:list.all")}</option>
              {STATUSES.map((s) => (
                <option key={s} value={s}>
                  {t(`status.${s}`)}
                </option>
              ))}
            </select>
          </label>
        )}
        <button type="button" onClick={reload}>
          {t("action.refresh")}
        </button>
        {canCreate && !status && (
          <button type="button" className="primary" onClick={() => setCreating(true)}>
            {t("procurement:request.new")}
          </button>
        )}
      </PageHeader>
      <Loading data={data} error={error} />
      {data && data.length === 0 && <p className="muted">{t("state.empty")}</p>}
      {data && data.length > 0 && (
        <table className="table clickable">
          <thead>
            <tr>
              <th>{t("procurement:request.number")}</th>
              <th>{t("procurement:request.title")}</th>
              <th>{t("procurement:request.status")}</th>
              <th>{t("procurement:request.requester")}</th>
              <th className="num">{t("procurement:request.items")}</th>
              <th>{t("procurement:request.updated")}</th>
            </tr>
          </thead>
          <tbody>
            {data.map((r) => (
              <tr key={r.id} onClick={() => setOpen(r.id)}>
                <td>
                  <button type="button" className="link" onClick={() => setOpen(r.id)}>
                    {r.number}
                  </button>
                </td>
                <td>{r.title}</td>
                <td>
                  <StatusBadge status={r.status} />
                </td>
                <td>{r.requester_name ?? ""}</td>
                <td className="num">{r.item_count}</td>
                <td>{formatDateTime(r.updated_at, i18n.language, active.timezone)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
