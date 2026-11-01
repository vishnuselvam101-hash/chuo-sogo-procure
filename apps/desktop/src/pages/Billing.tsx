import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useApi } from "../lib/useApi";
import { formatDate } from "../lib/format";
import type { InvoiceSummary, IssuedInvoice, MembershipView } from "../lib/types";
import { Loading, Money, PageHeader } from "../components/ui";
import { DocNumber, DocStatus, InvoiceView, PdfButton } from "../billing/InvoiceView";

function Detail({ path, onBack }: { path: string; onBack: () => void }) {
  const { t } = useTranslation();
  const { data, error } = useApi<IssuedInvoice>(path);
  return (
    <section>
      <PageHeader title={data?.number ?? t("billing:fromChuoSogo")}>
        {data && <PdfButton doc={data} />}
        <button type="button" onClick={onBack}>
          {t("action.back")}
        </button>
      </PageHeader>
      <Loading data={data} error={error} />
      {data && <InvoiceView doc={data} />}
    </section>
  );
}

/** Invoices and credit notes Chuo-Sogo issued to this organisation (finance). */
export function Billing({ active }: { active: MembershipView }) {
  const { t, i18n } = useTranslation();
  const base = `/api/v1/orgs/${active.org_id}/billing/invoices`;
  const { data, error } = useApi<InvoiceSummary[]>(base);
  const [open, setOpen] = useState<string | null>(null);
  if (open) return <Detail path={`${base}/${open}`} onBack={() => setOpen(null)} />;
  return (
    <section>
      <PageHeader title={t("nav.billing")} />
      <p className="muted small">{t("billing:customerHint")}</p>
      <Loading data={data} error={error} />
      {data && data.length === 0 && <p className="muted">{t("billing:none")}</p>}
      {data && data.length > 0 && (
        <table className="table" data-testid="customer-billing">
          <thead>
            <tr>
              <th>{t("billing:number")}</th>
              <th>{t("billing:kind")}</th>
              <th>{t("billing:issueDate")}</th>
              <th>{t("billing:dueDate")}</th>
              <th className="num">{t("billing:total")}</th>
              <th>{t("billing:statusLabel")}</th>
            </tr>
          </thead>
          <tbody>
            {data.map((d) => (
              <tr key={d.id}>
                <td>
                  <button type="button" className="link" onClick={() => setOpen(d.id)}>
                    <DocNumber doc={d} />
                  </button>
                </td>
                <td>{t(`billing:kinds.${d.kind}`)}</td>
                <td>{d.issue_date ? formatDate(d.issue_date, i18n.language) : "—"}</td>
                <td>{d.due_date ? formatDate(d.due_date, i18n.language) : "—"}</td>
                <td className="num">
                  <Money minor={d.total_minor} currency={d.currency} />
                </td>
                <td>
                  <DocStatus doc={d} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
