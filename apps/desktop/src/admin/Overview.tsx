import { useTranslation } from "react-i18next";
import { useApi } from "../lib/useApi";
import { Loading, PageHeader } from "../components/ui";
import { ADMIN_API, type SystemHealth } from "./types";

function bytes(n: number, lang: string): string {
  const mb = n / (1024 * 1024);
  return `${new Intl.NumberFormat(lang, { maximumFractionDigits: 1 }).format(mb)} MB`;
}

/** System health: server, database, AI provider, sessions, and per-organisation warnings. */
export function Overview({ onOpenOrg }: { onOpenOrg: (orgId: string) => void }) {
  const { t, i18n } = useTranslation();
  const { data, error, reload } = useApi<SystemHealth>(`${ADMIN_API}/health`);
  const n = (v: number) => new Intl.NumberFormat(i18n.language).format(v);

  return (
    <section>
      <PageHeader title={t("admin:nav.overview")}>
        <button type="button" onClick={reload}>
          {t("action.refresh")}
        </button>
      </PageHeader>
      <Loading data={data} error={error} />
      {data && (
        <>
          <dl className="stats" data-testid="health">
            <div>
              <dt>{t("admin:health.server")}</dt>
              <dd>{data.server_version}</dd>
            </div>
            <div>
              <dt>{t("admin:health.schema")}</dt>
              <dd>{data.schema_version ?? "—"}</dd>
            </div>
            <div>
              <dt>{t("admin:health.database")}</dt>
              <dd>{bytes(data.database_bytes, i18n.language)}</dd>
            </div>
            <div>
              <dt>{t("admin:health.ai")}</dt>
              <dd>
                {data.ai_provider}
                {data.ai_model ? ` · ${data.ai_model}` : ""}
              </dd>
            </div>
            <div>
              <dt>{t("admin:health.sessions")}</dt>
              <dd>{n(data.active_sessions)}</dd>
            </div>
            <div>
              <dt>{t("admin:health.accounts")}</dt>
              <dd>
                {n(data.accounts_active)}
                {data.accounts_locked > 0 && (
                  <span className="error">
                    {" "}
                    · {t("admin:health.locked", { count: data.accounts_locked })}
                  </span>
                )}
              </dd>
            </div>
          </dl>
          <table className="table">
            <thead>
              <tr>
                <th>{t("admin:orgs.name")}</th>
                <th className="num">{t("admin:orgs.openRequests")}</th>
                <th className="num">{t("admin:health.overdueTasks")}</th>
                <th className="num">{t("admin:health.disputedInvoices")}</th>
                <th className="num">{t("admin:health.failedAi")}</th>
                <th>{t("admin:health.auditChain")}</th>
              </tr>
            </thead>
            <tbody>
              {data.orgs.map((o) => (
                <tr key={o.org_id}>
                  <td>
                    <button type="button" className="link" onClick={() => onOpenOrg(o.org_id)}>
                      {o.name}
                    </button>
                  </td>
                  <td className="num">{n(o.open_requests)}</td>
                  <td className={`num${o.overdue_tasks > 0 ? " error" : ""}`}>{n(o.overdue_tasks)}</td>
                  <td className={`num${o.disputed_invoices > 0 ? " error" : ""}`}>
                    {n(o.disputed_invoices)}
                  </td>
                  <td className={`num${o.failed_ai_24h > 0 ? " error" : ""}`}>{n(o.failed_ai_24h)}</td>
                  <td className={o.audit_chain_valid ? "ok" : "error"}>
                    {o.audit_chain_valid ? t("admin:health.chainOk") : t("admin:health.chainBroken")}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
    </section>
  );
}
