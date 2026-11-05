import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { SuggestionView } from "../lib/agents";
import { useAction } from "../lib/useAction";
import { ErrorText } from "../components/ui";

/** What `extract_invoice` returns (validated by the server; still only a proposal). */
export interface InvoiceSuggestion {
  supplier_invoice_number: string;
  invoice_date?: string | null;
  due_date?: string | null;
  currency: string;
  shipping_minor: number;
  tax_rate_bps: number;
  tax_minor: number;
  flags?: string[];
  lines: {
    line_no?: number | null;
    order_item_id?: string | null;
    description: string;
    quantity: number;
    unit_price_minor: number;
    tax_rate_bps?: number | null;
    needs_review?: boolean;
    flags?: string[];
  }[];
}

const MAX_TEXT = 20_000;
const UNKNOWN = "unknown";

/**
 * L2: paste the supplier's invoice text; the server's model proposes the entries, which only
 * fill the form below. Nothing is recorded until a person presses "Record invoice".
 */
export function InvoiceExtract({
  orgId,
  orderId,
  onSuggestion,
}: {
  orgId: string;
  orderId: string;
  onSuggestion: (s: InvoiceSuggestion) => void;
}) {
  const { t } = useTranslation();
  const [text, setText] = useState("");
  const [open, setOpen] = useState(false);
  const [result, setResult] = useState<SuggestionView | null>(null);
  const action = useAction();

  async function read() {
    const res = await action.run<SuggestionView>(
      "POST",
      `/api/v1/orgs/${orgId}/orders/${orderId}/ai/extract-invoice`,
      {
        text,
      },
    );
    if (!res) return;
    setResult(res.body);
    if (res.body.status !== "failed") onSuggestion(res.body.output as unknown as InvoiceSuggestion);
  }

  const out = result && result.status !== "failed" ? (result.output as unknown as InvoiceSuggestion) : null;
  const review = out ? out.lines.filter((l) => l.needs_review || (l.flags?.length ?? 0) > 0).length : 0;

  if (!open)
    return (
      <div className="row">
        <button type="button" onClick={() => setOpen(true)}>
          {t("agents:matching.extractInvoice.title")}
        </button>
      </div>
    );
  return (
    <div className="sub-card ai-assist stack" data-testid="extract-invoice">
      <h4>{t("agents:matching.extractInvoice.title")}</h4>
      <p className="muted small">{t("agents:matching.extractInvoice.hint")}</p>
      <label>
        {t("agents:ui.extract.text")}
        <textarea rows={6} maxLength={MAX_TEXT} value={text} onChange={(e) => setText(e.target.value)} />
      </label>
      <div className="row">
        <button type="button" disabled={action.busy || !text.trim()} onClick={() => void read()}>
          {t("agents:ui.extract.read")}
        </button>
        <button type="button" onClick={() => setOpen(false)}>
          {t("action.cancel")}
        </button>
      </div>
      {result?.status === "failed" && <ErrorText code={result.error_code ?? UNKNOWN} />}
      {out && (
        <p role="status" className="small">
          {t("agents:ui.extract.filled", { lines: out.lines.length })}
          {review > 0 && ` ${t("agents:ui.extract.review", { count: review })}`}
        </p>
      )}
      <ErrorText code={action.error} />
    </div>
  );
}
