import { useState } from "react";
import { useTranslation } from "react-i18next";
import { AGENTS, exceptionTitle, type StaffException } from "../lib/agents";
import { formatDateTime } from "../lib/format";
import { useApi } from "../lib/useApi";
import { EmptyState, Loading, PageHeader } from "../components/ui";
import { ExceptionDetail, ExceptionStatusBadge, SeverityBadge } from "../pages/AgentExceptions";
import { ADMIN_API, PLATFORM_TZ } from "./types";

const OPEN = "open";
const ALL = "all";

/**
 * Staff view: agent exceptions of every customer the administrator is assigned to.
 * Read only here; decisions are made in the desktop app inside that customer.
 */
export function AgentExceptionsAdmin() {
  const { t, i18n } = useTranslation();
  const [agent, setAgent] = useState("");
  const [status, setStatus] = useState(OPEN);
  const [open, setOpen] = useState<string | null>(null);
  const query = new URLSearchParams({ status, ...(agent ? { agent } : {}) }).toString();
  const { data, error, reload } = useApi<StaffException[]>(`${ADMIN_API}/ops/agents/exceptions?${query}`);
  const opened = data?.find((r) => r.exception.id === open)?.exception ?? null;

  return (
    <section>
      <PageHeader title={t("admin:nav.agents")}>
        <button type="button" onClick={reload}>
          {t("action.refresh")}
        </button>
      </PageHeader>
      <p className="lead">{t("agents:ui.staff.lead")}</p>
      <div className="row">
        <label>
          {t("agents:ui.filter.agent")}
          <select value={agent} onChange={(e) => setAgent(e.target.value)}>
            <option value="">{t("agents:ui.filter.all")}</option>
            {AGENTS.map((a) => (
              <option key={a} value={a}>
                {t(`agents:ui.agent.${a}`)}
              </option>
            ))}
          </select>
        </label>
        <label>
          {t("agents:ui.filter.status")}
          <select value={status} onChange={(e) => setStatus(e.target.value)}>
            <option value={OPEN}>{t("agents:runtime.exceptionStatus.open")}</option>
            <option value={ALL}>{t("agents:ui.filter.all")}</option>
          </select>
        </label>
      </div>
      <Loading data={data} error={error} />
      {data && data.length === 0 && <EmptyState hint={t("agents:ui.inbox.empty")} />}
      {data && data.length > 0 && (
        <table className="table" data-testid="staff-exceptions">
          <thead>
            <tr>
              <th>{t("agents:ui.col.customer")}</th>
              <th>{t("agents:ui.col.severity")}</th>
              <th>{t("agents:ui.col.title")}</th>
              <th>{t("agents:ui.col.created")}</th>
              <th>{t("agents:ui.col.status")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {data.map(({ org_name, exception: ex }) => (
              <tr key={ex.id}>
                <td>{org_name}</td>
                <td>
                  <SeverityBadge severity={ex.severity} />
                </td>
                <td>{exceptionTitle(ex, t, i18n.language)}</td>
                <td className="small">{formatDateTime(ex.created_at, i18n.language, PLATFORM_TZ)}</td>
                <td>
                  <ExceptionStatusBadge status={ex.status} />
                </td>
                <td className="row-actions">
                  <button type="button" onClick={() => setOpen(open === ex.id ? null : ex.id)}>
                    {open === ex.id ? t("agents:ui.inbox.close") : t("agents:ui.inbox.open")}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {opened && (
        <>
          <p className="muted small">{t("agents:ui.staff.decideInApp")}</p>
          <ExceptionDetail
            key={opened.id}
            ex={opened}
            orgId={null}
            timezone={PLATFORM_TZ}
            canDecide={false}
            onClose={() => setOpen(null)}
          />
        </>
      )}
    </section>
  );
}
