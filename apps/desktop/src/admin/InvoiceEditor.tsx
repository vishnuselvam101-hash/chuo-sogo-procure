import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { minorToInput, parseMinor } from "../lib/format";
import type { InvoiceLineInput, IssuedInvoice } from "../lib/types";
import { ErrorText, Money } from "../components/ui";
import { RATES, rateLabel } from "../billing/InvoiceView";
import { ADMIN_API, type OrgOverview } from "./types";

const DEFAULT_CURRENCY = "JPY";
const STANDARD_RATE = 1000;
const CUSTOMER = "customer";

interface Row {
  description: string;
  quantity: string;
  unit: string;
  price: string;
  rate: number;
}

const emptyRow = (): Row => ({ description: "", quantity: "1", unit: "", price: "", rate: STANDARD_RATE });

function rowsFrom(doc: IssuedInvoice | null, currency: string): Row[] {
  if (!doc) return [emptyRow()];
  return doc.lines.map((l) => ({
    description: l.description,
    quantity: String(l.quantity),
    unit: l.unit,
    price: minorToInput(l.unit_price_minor, currency),
    rate: l.tax_rate_bps,
  }));
}

/** Rows → API lines; `null` (with the row index) when a quantity or price cannot be read. */
export function toLines(rows: Row[], currency: string): { lines: InvoiceLineInput[] } | { bad: number } {
  const lines: InvoiceLineInput[] = [];
  for (const [i, r] of rows.entries()) {
    const quantity = Number.parseInt(r.quantity, 10);
    const price = parseMinor(r.price, currency);
    if (
      !Number.isInteger(quantity) ||
      quantity < 1 ||
      String(quantity) !== r.quantity.trim() ||
      price === null
    )
      return { bad: i };
    lines.push({
      description: r.description,
      quantity,
      unit: r.unit,
      unit_price_minor: price,
      tax_rate_bps: r.rate,
    });
  }
  return { lines };
}

type Mode =
  { kind: "new" } | { kind: "edit"; doc: IssuedInvoice } | { kind: "credit"; original: IssuedInvoice };

/**
 * Draft editor for invoices and credit notes. Line amounts are shown for orientation; the
 * totals and the tax per rate are calculated by the server when the draft is saved.
 */
export function InvoiceEditor({
  mode,
  onSaved,
  onCancel,
}: {
  mode: Mode;
  onSaved: (orgId: string, id: string) => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const { run, busy, error, setError } = useAction();
  const orgs = useApi<OrgOverview[]>(`${ADMIN_API}/orgs`);
  const existing = mode.kind === "edit" ? mode.doc : null;
  const base = mode.kind === "credit" ? mode.original : existing;
  const currency = base?.currency ?? DEFAULT_CURRENCY;
  const [orgId, setOrgId] = useState(base?.org_id ?? "");
  const [recipientAddress, setRecipientAddress] = useState(existing?.recipient_address ?? "");
  const [dueDate, setDueDate] = useState(existing?.due_date ?? "");
  const [notes, setNotes] = useState(existing?.notes ?? "");
  const [reason, setReason] = useState("");
  const [rows, setRows] = useState<Row[]>(() => rowsFrom(existing, currency));

  const setRow = (i: number, patch: Partial<Row>) =>
    setRows((rs) => rs.map((r, j) => (j === i ? { ...r, ...patch } : r)));

  async function submit(e: FormEvent) {
    e.preventDefault();
    const parsed = toLines(rows, currency);
    if ("bad" in parsed) return setError("validation_failed/amount");
    let res: { body: { id: string } | null } | null;
    if (mode.kind === "credit") {
      res = await run<{ id: string }>(
        "POST",
        `${ADMIN_API}/orgs/${mode.original.org_id}/billing/invoices/${mode.original.id}/credit-notes`,
        { reason, notes, lines: parsed.lines },
      );
      if (res?.body) onSaved(mode.original.org_id, res.body.id);
      return;
    }
    const body = {
      currency,
      recipient_address: recipientAddress,
      due_date: dueDate || null,
      notes,
      lines: parsed.lines,
    };
    if (existing) {
      res = await run("PUT", `${ADMIN_API}/orgs/${existing.org_id}/billing/invoices/${existing.id}`, body);
      if (res) onSaved(existing.org_id, existing.id);
    } else {
      res = await run<{ id: string }>("POST", `${ADMIN_API}/orgs/${orgId}/billing/invoices`, body);
      if (res?.body) onSaved(orgId, res.body.id);
    }
  }

  const title =
    mode.kind === "credit"
      ? t("billing:creditFor", { number: mode.original.number })
      : existing
        ? t("billing:editDraft")
        : t("billing:newInvoice");

  return (
    <form className="card stack" onSubmit={submit} data-testid="invoice-editor">
      <h2>{title}</h2>
      {mode.kind === "new" && (
        <label>
          {t("billing:customer")}
          <select required value={orgId} onChange={(e) => setOrgId(e.target.value)}>
            <option value="">{t("billing:chooseCustomer")}</option>
            {(orgs.data ?? [])
              .filter((o) => o.kind === CUSTOMER)
              .map((o) => (
                <option key={o.id} value={o.id}>
                  {o.name}
                </option>
              ))}
          </select>
        </label>
      )}
      {mode.kind === "credit" ? (
        <label>
          {t("billing:reason")}
          <textarea required maxLength={2000} value={reason} onChange={(e) => setReason(e.target.value)} />
        </label>
      ) : (
        <div className="form-grid">
          <label>
            {t("billing:recipientAddress")}
            <input
              maxLength={500}
              value={recipientAddress}
              onChange={(e) => setRecipientAddress(e.target.value)}
            />
          </label>
          <label>
            {t("billing:dueDate")}
            <input type="date" value={dueDate} onChange={(e) => setDueDate(e.target.value)} />
          </label>
          <p className="muted small">{t("billing:dueDateHint")}</p>
        </div>
      )}
      <table className="table">
        <thead>
          <tr>
            <th>{t("billing:description")}</th>
            <th>{t("billing:quantity")}</th>
            <th>{t("billing:unit")}</th>
            <th>{t("billing:unitPriceExcl", { currency })}</th>
            <th>{t("billing:taxRate")}</th>
            <th className="num">{t("billing:amount")}</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {rows.map((r, i) => {
            const price = parseMinor(r.price, currency);
            const qty = Number.parseInt(r.quantity, 10);
            return (
              <tr key={i}>
                <td>
                  <input
                    required
                    maxLength={500}
                    aria-label={t("billing:description")}
                    value={r.description}
                    onChange={(e) => setRow(i, { description: e.target.value })}
                  />
                </td>
                <td>
                  <input
                    required
                    inputMode="numeric"
                    size={5}
                    aria-label={t("billing:quantity")}
                    value={r.quantity}
                    onChange={(e) => setRow(i, { quantity: e.target.value })}
                  />
                </td>
                <td>
                  <input
                    maxLength={20}
                    size={4}
                    aria-label={t("billing:unit")}
                    value={r.unit}
                    onChange={(e) => setRow(i, { unit: e.target.value })}
                  />
                </td>
                <td>
                  <input
                    required
                    inputMode="decimal"
                    size={10}
                    aria-label={t("billing:unitPrice")}
                    value={r.price}
                    onChange={(e) => setRow(i, { price: e.target.value })}
                  />
                </td>
                <td>
                  <select
                    aria-label={t("billing:taxRate")}
                    value={r.rate}
                    onChange={(e) => setRow(i, { rate: Number(e.target.value) })}
                  >
                    {RATES.map((rate) => (
                      <option key={rate} value={rate}>
                        {t(`billing:rates.r${rate}`, { label: rateLabel(rate) })}
                      </option>
                    ))}
                  </select>
                </td>
                <td className="num">
                  {price !== null && Number.isInteger(qty) && qty > 0 ? (
                    <Money minor={price * qty} currency={currency} />
                  ) : null}
                </td>
                <td>
                  {rows.length > 1 && (
                    <button
                      type="button"
                      className="link"
                      onClick={() => setRows((rs) => rs.filter((_, j) => j !== i))}
                    >
                      {t("billing:removeLine")}
                    </button>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      <div className="row">
        <button
          type="button"
          onClick={() => setRows((rs) => [...rs, emptyRow()])}
          disabled={rows.length >= 200}
        >
          {t("billing:addLine")}
        </button>
      </div>
      <label>
        {t("billing:notes")}
        <textarea maxLength={2000} value={notes} onChange={(e) => setNotes(e.target.value)} />
      </label>
      <p className="muted small">{t("billing:serverComputes")}</p>
      <ErrorText code={error} />
      <div className="row">
        <button type="submit" className="primary" disabled={busy}>
          {t("billing:saveDraft")}
        </button>
        <button type="button" onClick={onCancel}>
          {t("action.cancel")}
        </button>
      </div>
    </form>
  );
}
