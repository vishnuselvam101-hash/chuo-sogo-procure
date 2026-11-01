import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useBridge } from "../lib/BridgeContext";
import { errorCode, ok } from "../lib/bridge";
import { formatDate } from "../lib/format";
import type { InvoiceSummary, IssuedInvoice } from "../lib/types";
import { ErrorText, Money } from "../components/ui";

export const RATES = [1000, 800, 0] as const;
export const REDUCED_RATE = 800;

/** "10%", "8%※" (reduced rate), "0%". */
export function rateLabel(bps: number): string {
  return `${bps / 100}%${bps === REDUCED_RATE ? "※" : ""}`;
}

export function DocStatus({ doc }: { doc: InvoiceSummary }) {
  const { t } = useTranslation();
  return (
    <>
      <span className={`badge badge-${doc.status}`}>{t(`billing:status.${doc.status}`)}</span>
      {doc.overdue && <span className="badge badge-overdue">{t("billing:status.overdue")}</span>}
    </>
  );
}

export function DocNumber({ doc }: { doc: InvoiceSummary }) {
  const { t } = useTranslation();
  return <>{doc.number ?? <span className="muted">{t("billing:draftNumber")}</span>}</>;
}

/** Saves the issued PDF into Downloads (native layer); shows where it went. */
export function PdfButton({ doc }: { doc: InvoiceSummary }) {
  const { t } = useTranslation();
  const bridge = useBridge();
  const [saved, setSaved] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  if (!doc.number) return null;
  const number = doc.number;
  return (
    <span className="stack">
      <button
        type="button"
        onClick={async () => {
          setError(null);
          setSaved(null);
          try {
            const res = await bridge.invoicePdf(doc.org_id, doc.id, number);
            if (ok(res)) setSaved(res.body.saved_to);
            else setError(errorCode(res));
          } catch (err) {
            setError(typeof err === "string" ? err : "unknown");
          }
        }}
      >
        {t("billing:downloadPdf")}
      </button>
      {saved && (
        <span className="ok small" role="status">
          {t("billing:savedTo", { path: saved })}
        </span>
      )}
      <ErrorText code={error} />
    </span>
  );
}

/** Read-only view of a document: parties, lines, totals per tax rate (all computed by the server). */
export function InvoiceView({ doc }: { doc: IssuedInvoice }) {
  const { t, i18n } = useTranslation();
  const date = (d: string | null | undefined) => (d ? formatDate(d, i18n.language) : "—");
  return (
    <div className="stack" data-testid="invoice-view">
      <dl className="facts">
        <dt>{t("billing:number")}</dt>
        <dd>
          <DocNumber doc={doc} /> <DocStatus doc={doc} />
        </dd>
        <dt>{t("billing:kind")}</dt>
        <dd>{t(`billing:kinds.${doc.kind}`)}</dd>
        <dt>{t("billing:recipient")}</dt>
        <dd>
          {doc.recipient_name}
          {doc.recipient_address && <div className="muted small">{doc.recipient_address}</div>}
        </dd>
        <dt>{t("billing:issueDate")}</dt>
        <dd>{date(doc.issue_date)}</dd>
        {doc.kind === "invoice" && (
          <>
            <dt>{t("billing:dueDate")}</dt>
            <dd>{date(doc.due_date)}</dd>
          </>
        )}
        {doc.paid_on && (
          <>
            <dt>{t("billing:paidOn")}</dt>
            <dd>
              {date(doc.paid_on)}
              {doc.paid_note && <span className="muted"> · {doc.paid_note}</span>}
            </dd>
          </>
        )}
        {doc.reason && (
          <>
            <dt>{t("billing:reason")}</dt>
            <dd className="pre">{doc.reason}</dd>
          </>
        )}
        {doc.notes && (
          <>
            <dt>{t("billing:notes")}</dt>
            <dd className="pre">{doc.notes}</dd>
          </>
        )}
      </dl>
      <table className="table">
        <thead>
          <tr>
            <th>#</th>
            <th>{t("billing:description")}</th>
            <th className="num">{t("billing:quantity")}</th>
            <th className="num">{t("billing:unitPrice")}</th>
            <th className="num">{t("billing:taxRate")}</th>
            <th className="num">{t("billing:amount")}</th>
          </tr>
        </thead>
        <tbody>
          {doc.lines.map((l) => (
            <tr key={l.position}>
              <td className="num">{l.position}</td>
              <td>{l.description}</td>
              <td className="num">
                {l.quantity} {l.unit}
              </td>
              <td className="num">
                <Money minor={l.unit_price_minor} currency={doc.currency} />
              </td>
              <td className="num">{rateLabel(l.tax_rate_bps)}</td>
              <td className="num">
                <Money minor={l.unit_price_minor * l.quantity} currency={doc.currency} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <table className="table narrow-table" data-testid="per-rate">
        <tbody>
          {doc.per_rate.map((r) => (
            <tr key={r.rate_bps}>
              <th>{t("billing:subjectTo", { rate: rateLabel(r.rate_bps) })}</th>
              <td className="num">
                <Money minor={r.base_minor} currency={doc.currency} />
              </td>
              <td className="num">
                {t("billing:taxShort")} <Money minor={r.tax_minor} currency={doc.currency} />
              </td>
            </tr>
          ))}
          <tr>
            <th>{t("billing:subtotal")}</th>
            <td className="num" colSpan={2}>
              <Money minor={doc.subtotal_minor} currency={doc.currency} />
            </td>
          </tr>
          <tr>
            <th>{t("billing:tax")}</th>
            <td className="num" colSpan={2}>
              <Money minor={doc.tax_minor} currency={doc.currency} />
            </td>
          </tr>
          <tr>
            <th>{t("billing:total")}</th>
            <td className="num strong" colSpan={2} data-testid="invoice-total">
              <Money minor={doc.total_minor} currency={doc.currency} />
            </td>
          </tr>
        </tbody>
      </table>
      {doc.lines.some((l) => l.tax_rate_bps === REDUCED_RATE) && (
        <p className="muted small">{t("billing:reducedNote")}</p>
      )}
      {doc.pdf_sha256 && (
        <p className="muted small">
          {t("billing:pdfHash")} <code className="wrap">{doc.pdf_sha256}</code>
        </p>
      )}
    </div>
  );
}
