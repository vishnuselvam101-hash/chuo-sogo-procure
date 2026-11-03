import { useState } from "react";
import { useTranslation } from "react-i18next";
import { formatDateTime } from "../lib/format";
import type { MembershipView, RequestSummary } from "../lib/types";
import { useApi } from "../lib/useApi";
import { EmptyState, Loading, PageHeader, StatusBadge } from "../components/ui";
import { CsvExport } from "../components/CsvExport";
import { RequestImport } from "./RequestImport";
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
  /** `orders` or `finance`: a group of statuses (orders and invoices pages). */
  stage?: string;
  /** Open this request immediately (e.g. from the operations queue). */
  initialOpen?: string | null;
}

/** Request list for customers (own or all, as the backend decides) and assigned operators. */
const ORDERS_CSV = "orders";

export function Requests({ active, selfId, status, title, initialOpen, stage }: Props) {
  const { t, i18n } = useTranslation();
  const [open, setOpen] = useState<string | null>(initialOpen ?? null);
  const [creating, setCreating] = useState(false);
  const [importing, setImporting] = useState(false);
  const [filter, setFilter] = useState(status ?? "");
  const query = filter
    ? `?status=${encodeURIComponent(filter)}`
    : stage
      ? `?stage=${encodeURIComponent(stage)}`
      : "";
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
        {!status && !stage && (
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
        {canCreate && !status && !stage && (
          <button type="button" onClick={() => setImporting(!importing)}>
            {t("procurement:import.open")}
          </button>
        )}
        {canCreate && !status && !stage && (
          <button type="button" className="primary" onClick={() => setCreating(true)}>
            {t("procurement:request.new")}
          </button>
        )}
      </PageHeader>
      {importing && <RequestImport active={active} onImported={reload} />}
      {stage === "orders" && active.permissions.includes(P.viewOrders) && (
        <CsvExport active={active} kind={ORDERS_CSV} />
      )}
      <Loading data={data} error={error} />
      {data && data.length === 0 && (
        <EmptyState
          hint={t(canCreate && !status && !stage ? "empty.requestsCreate" : "empty.requestsStage")}
        />
      )}
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
