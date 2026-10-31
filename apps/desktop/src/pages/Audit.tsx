import { useTranslation } from "react-i18next";
import { useApi } from "../lib/useApi";
import { formatDateTime } from "../lib/format";
import type { AuditResponse, MembershipView } from "../lib/types";
import { ErrorText, PageHeader } from "../components/ui";

export function Audit({ active }: { active: MembershipView }) {
  const { t, i18n } = useTranslation();
  const { data, error, reload } = useApi<AuditResponse>(`/api/v1/orgs/${active.org_id}/audit?limit=200`);

  return (
    <section>
      <PageHeader title={t("nav.audit")}>
        <button type="button" onClick={reload}>
          {t("action.refresh")}
        </button>
      </PageHeader>
      <ErrorText code={error} />
      {data && (
        <>
          <p className={data.chain_valid ? "ok" : "error"} role="status">
            {data.chain_valid
              ? t("audit.chainValid")
              : t("audit.chainBroken", { seq: data.first_broken_seq })}
          </p>
          <table className="table">
            <thead>
              <tr>
                <th>#</th>
                <th>{t("audit.when")}</th>
                <th>{t("audit.action")}</th>
              </tr>
            </thead>
            <tbody>
              {data.events.map((e) => (
                <tr key={e.seq}>
                  <td className="num">{e.seq}</td>
                  <td>{formatDateTime(e.created_at, i18n.language, active.timezone)}</td>
                  <td>
                    <code>{e.action}</code>
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
