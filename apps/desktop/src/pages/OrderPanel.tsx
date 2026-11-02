import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "../lib/api-schema";
import { formatDate, formatDateTime, minorToInput, parseMinor } from "../lib/format";
import { P } from "../lib/perm";
import type { MembershipView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { ErrorText, Money, StatusBadge } from "../components/ui";

type OrderDetail = components["schemas"]["OrderDetail"];
const ROUNDINGS = ["floor", "half_up", "ceil"];
const DEFAULT_RESOLUTION = "credit_note";
const RESOLVED = "resolved";
const OPEN = "open";
const RESOLUTIONS = ["credit_note", "supplier_corrected", "accepted", "written_off", "other"];
const ORDER_STAGES = ["ordered", "delivered", "invoiced", "disputed", "reconciled", "closed"];

/** Purchase order, receiving, invoice and three-way match for one request. */
export function OrderPanel({
  active,
  requestId,
  status,
  onChanged,
}: {
  active: MembershipView;
  requestId: string;
  status: string;
  onChanged: () => void;
}) {
  const { t } = useTranslation();
  const has = (p: string) => active.permissions.includes(p);
  const org = `/api/v1/orgs/${active.org_id}`;
  const action = useAction();
  const [po, setPo] = useState("");

  if (status === "approved") {
    if (!has(P.recordOrders)) return null;
    return (
      <div className="card stack">
        <h2>{t("procurement:order.heading")}</h2>
        <p className="muted small">{t("procurement:order.explain")}</p>
        <div className="row">
          <label className="grow">
            {t("procurement:order.poNumber")}
            <input maxLength={100} value={po} onChange={(e) => setPo(e.target.value)} />
          </label>
          <button
            type="button"
            className="primary"
            disabled={!po.trim() || action.busy}
            onClick={() =>
              void action
                .run("POST", `${org}/requests/${requestId}/order`, { po_number: po })
                .then((r) => r && onChanged())
            }
          >
            {t("procurement:order.record")}
          </button>
        </div>
        <ErrorText code={action.error} />
      </div>
    );
  }
  if (!ORDER_STAGES.includes(status)) return null;
  return <OrderDetails active={active} requestId={requestId} status={status} onChanged={onChanged} />;
}

function OrderDetails({
  active,
  requestId,
  status,
  onChanged,
}: {
  active: MembershipView;
  requestId: string;
  status: string;
  onChanged: () => void;
}) {
  const { t, i18n } = useTranslation();
  const has = (p: string) => active.permissions.includes(p);
  const org = `/api/v1/orgs/${active.org_id}`;
  const { data, error, reload } = useApi<OrderDetail>(`${org}/requests/${requestId}/order`);
  const action = useAction();
  const [receive, setReceive] = useState<Record<string, string>>({});
  const [inv, setInv] = useState<{
    number: string;
    date: string;
    due: string;
    shipping: string;
    rate: string;
    rounding: string;
    tax: string;
    lines: Record<string, { qty: string; price: string; rate: string }>;
  } | null>(null);
  const [reverseReason, setReverseReason] = useState("");
  const [eta, setEta] = useState({ date: "", note: "" });
  const [resolution, setResolution] = useState<Record<string, { kind: string; note: string }>>({});

  if (!data) return error ? <ErrorText code={error} /> : null;
  const o = data.order;
  const cur = o.currency;
  const refresh = () => {
    reload();
    onChanged();
  };

  const items = data.items;

  async function recordReceipt() {
    const lines = items
      .map((i) => ({ order_item_id: i.id, raw: (receive[i.id] ?? "").trim() }))
      .filter((l) => l.raw !== "")
      .map((l) => ({
        order_item_id: l.order_item_id,
        quantity: /^\d{1,10}$/.test(l.raw) ? Number(l.raw) : NaN,
      }));
    if (lines.length === 0 || lines.some((l) => !l.quantity))
      return action.setError("validation_failed/lines");
    if (await action.run("POST", `${org}/orders/${o.id}/deliveries`, { lines })) {
      setReceive({});
      refresh();
    }
  }

  const invoiceForm =
    inv ??
    ({
      number: "",
      date: new Date().toISOString().slice(0, 10),
      due: "",
      shipping: minorToInput(o.shipping_minor, cur),
      rate: "10",
      rounding: "floor",
      tax: "",
      // Typed from the supplier's invoice; the order's figures are shown only as hints so a
      // click-through cannot "match" without anyone reading the invoice.
      lines: Object.fromEntries(data.items.map((i) => [i.id, { qty: "", price: "", rate: "" }])),
    } as NonNullable<typeof inv>);

  async function recordInvoice() {
    const f = invoiceForm;
    const shipping = parseMinor(f.shipping, cur);
    const tax = parseMinor(f.tax, cur);
    // Rate in percent with up to two decimals → basis points, without floating point.
    const rate = /^\d{1,3}(\.\d{1,2})?$/.test(f.rate.trim()) ? parseMinor(f.rate.trim(), "USD") : null;
    const toBps = (v: string) =>
      /^\d{1,3}(\.\d{1,2})?$/.test(v.trim()) ? parseMinor(v.trim(), "USD") : null;
    const lines = items.map((i) => {
      const l = f.lines[i.id] ?? { qty: "", price: "", rate: "" };
      const qty = /^\d{1,10}$/.test(l.qty.trim()) ? Number(l.qty) : NaN;
      // Blank: the invoice's rate; otherwise this line's own rate (e.g. 8% reduced rate).
      const lineRate = l.rate.trim() === "" ? undefined : toBps(l.rate);
      return {
        order_item_id: i.id,
        quantity: qty,
        unit_price_minor: parseMinor(l.price, cur),
        tax_rate_bps: lineRate,
      };
    });
    if (
      shipping === null ||
      tax === null ||
      rate === null ||
      lines.some((l) => !l.quantity || l.unit_price_minor === null || l.tax_rate_bps === null)
    )
      return action.setError("validation_failed/amount");
    const res = await action.run("POST", `${org}/orders/${o.id}/invoice`, {
      supplier_invoice_number: f.number,
      invoice_date: f.date,
      due_date: f.due || null,
      currency: cur,
      shipping_minor: shipping,
      tax_rate_bps: rate,
      tax_rounding: f.rounding,
      tax_minor: tax,
      lines,
    });
    if (res) {
      setInv(null);
      refresh();
    }
  }

  async function confirmEta() {
    if (!eta.date) return action.setError("validation_failed/confirmed_on");
    if (
      await action.run("POST", `${org}/orders/${o.id}/delivery-date`, {
        confirmed_on: eta.date,
        note: eta.note,
      })
    ) {
      setEta({ date: "", note: "" });
      refresh();
    }
  }

  async function reverse(deliveryId: string) {
    const reason = reverseReason.trim();
    if (!reason) return action.setError("validation_failed/reason");
    if (await action.run("POST", `${org}/deliveries/${deliveryId}/reverse`, { reason })) {
      setReverseReason("");
      refresh();
    }
  }

  async function resolve(id: string) {
    const r = resolution[id] ?? { kind: DEFAULT_RESOLUTION, note: "" };
    if (await action.run("POST", `${org}/discrepancies/${id}/resolve`, { resolution: r.kind, note: r.note }))
      refresh();
  }

  const setLine = (id: string, patch: Partial<{ qty: string; price: string; rate: string }>) =>
    setInv({
      ...invoiceForm,
      lines: {
        ...invoiceForm.lines,
        [id]: { ...(invoiceForm.lines[id] ?? { qty: "", price: "", rate: "" }), ...patch },
      },
    });

  return (
    <div className="card stack">
      <div className="row">
        <h2>{t("procurement:order.heading")}</h2>
        <span>{t("procurement:order.po", { number: o.po_number })}</span>
        <span className="muted">{o.supplier_name ?? ""}</span>
        <Money minor={o.total_minor} currency={cur} />
      </div>
      <div className="row small">
        <span>
          {t("procurement:delivery.expected")}:{" "}
          {o.expected_on ? formatDate(o.expected_on, i18n.language) : t("procurement:delivery.unknown")}
        </span>
        {o.delivery_confirmed_on ? (
          <span className="muted">
            {t("procurement:delivery.confirmedBySupplier")}
            {o.delivery_note ? ` — ${o.delivery_note}` : ""}
          </span>
        ) : (
          o.promised_on && <span className="muted">{t("procurement:delivery.fromLeadTime")}</span>
        )}
        {o.late && <span className="badge badge-overdue">{t("procurement:delivery.late")}</span>}
      </div>
      {status === "ordered" && active.access === "assigned" && has(P.operateRequests) && (
        <div className="row">
          <label>
            {t("procurement:delivery.confirmedOn")}
            <input type="date" value={eta.date} onChange={(e) => setEta({ ...eta, date: e.target.value })} />
          </label>
          <label className="grow">
            {t("procurement:delivery.note")}
            <input
              maxLength={2000}
              value={eta.note}
              onChange={(e) => setEta({ ...eta, note: e.target.value })}
            />
          </label>
          <button type="button" disabled={action.busy || !eta.date} onClick={() => void confirmEta()}>
            {t("procurement:delivery.record")}
          </button>
        </div>
      )}
      <table className="table">
        <thead>
          <tr>
            <th className="num">{t("procurement:item.line")}</th>
            <th>{t("procurement:item.description")}</th>
            <th className="num">{t("procurement:order.ordered")}</th>
            <th className="num">{t("procurement:order.received")}</th>
            <th className="num">{t("procurement:quote.unitPrice")}</th>
            {status === "ordered" && has(P.receiveGoods) && <th>{t("procurement:order.receiveNow")}</th>}
          </tr>
        </thead>
        <tbody>
          {data.items.map((i) => (
            <tr key={i.id}>
              <td className="num">{i.line_no}</td>
              <td>
                {i.description}
                {i.alternative && (
                  <span className="badge badge-warn">{t("procurement:quote.alternative")}</span>
                )}
              </td>
              <td className="num">{`${i.quantity} ${i.unit}`}</td>
              <td className="num">{i.received}</td>
              <td className="num">
                <Money minor={i.unit_price_minor} currency={cur} />
              </td>
              {status === "ordered" && has(P.receiveGoods) && (
                <td>
                  <input
                    inputMode="numeric"
                    aria-label={t("procurement:order.receiveNow")}
                    value={receive[i.id] ?? ""}
                    placeholder={String(Math.max(i.quantity - i.received, 0))}
                    onChange={(e) => setReceive({ ...receive, [i.id]: e.target.value })}
                  />
                </td>
              )}
            </tr>
          ))}
        </tbody>
      </table>
      {status === "ordered" && has(P.receiveGoods) && (
        <div className="row">
          <button
            type="button"
            className="primary"
            disabled={action.busy}
            onClick={() => void recordReceipt()}
          >
            {t("procurement:order.recordReceipt")}
          </button>
        </div>
      )}
      {data.deliveries.length > 0 &&
        (status === "ordered" || status === "delivered") &&
        has(P.receiveGoods) && (
          <label className="compact">
            {t("procurement:order.reverseReason")}
            <input
              maxLength={2000}
              value={reverseReason}
              onChange={(e) => setReverseReason(e.target.value)}
            />
          </label>
        )}
      {data.deliveries.length > 0 && (
        <ul className="plain small">
          {data.deliveries.map((d) => (
            <li key={d.id} className={d.reversed_at ? "muted" : undefined}>
              {t("procurement:order.deliveryLine", {
                when: formatDateTime(d.received_at, i18n.language, active.timezone),
                who: d.received_by_name ?? "",
              })}
              {d.note && ` · ${d.note}`}
              {d.reversed_at && (
                <span className="badge badge-rejected">
                  {t("procurement:order.reversed", { reason: d.reversal_reason ?? "" })}
                </span>
              )}
              {!d.reversed_at && (status === "ordered" || status === "delivered") && has(P.receiveGoods) && (
                <button type="button" className="link" onClick={() => void reverse(d.id)}>
                  {t("procurement:order.reverse")}
                </button>
              )}
            </li>
          ))}
        </ul>
      )}

      {status === "delivered" && has(P.manageInvoices) && (
        <div className="stack sub-card">
          <h3>{t("procurement:invoice.record")}</h3>
          <div className="row">
            <label>
              {t("procurement:invoice.number")}
              <input
                value={invoiceForm.number}
                onChange={(e) => setInv({ ...invoiceForm, number: e.target.value })}
              />
            </label>
            <label>
              {t("procurement:invoice.date")}
              <input
                type="date"
                value={invoiceForm.date}
                onChange={(e) => setInv({ ...invoiceForm, date: e.target.value })}
              />
            </label>
            <label>
              {t("procurement:invoice.due")}
              <input
                type="date"
                value={invoiceForm.due}
                onChange={(e) => setInv({ ...invoiceForm, due: e.target.value })}
              />
            </label>
          </div>
          <table className="table">
            <thead>
              <tr>
                <th className="num">{t("procurement:item.line")}</th>
                <th>{t("procurement:item.quantity")}</th>
                <th>{t("procurement:quote.unitPrice")}</th>
                <th>{t("procurement:invoice.lineTaxRate")}</th>
              </tr>
            </thead>
            <tbody>
              {data.items.map((i) => (
                <tr key={i.id}>
                  <td className="num">{i.line_no}</td>
                  <td>
                    <input
                      inputMode="numeric"
                      aria-label={t("procurement:item.quantity")}
                      placeholder={String(i.quantity)}
                      value={invoiceForm.lines[i.id]?.qty ?? ""}
                      onChange={(e) => setLine(i.id, { qty: e.target.value })}
                    />
                  </td>
                  <td>
                    <input
                      inputMode="decimal"
                      aria-label={t("procurement:quote.unitPrice")}
                      placeholder={minorToInput(i.unit_price_minor, cur)}
                      value={invoiceForm.lines[i.id]?.price ?? ""}
                      onChange={(e) => setLine(i.id, { price: e.target.value })}
                    />
                  </td>
                  <td>
                    <input
                      inputMode="decimal"
                      aria-label={t("procurement:invoice.lineTaxRate")}
                      placeholder={invoiceForm.rate}
                      value={invoiceForm.lines[i.id]?.rate ?? ""}
                      onChange={(e) => setLine(i.id, { rate: e.target.value })}
                    />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <div className="row">
            <label>
              {t("procurement:quote.shipping")}
              <input
                inputMode="decimal"
                value={invoiceForm.shipping}
                onChange={(e) => setInv({ ...invoiceForm, shipping: e.target.value })}
              />
            </label>
            <label className="narrow">
              {t("procurement:invoice.taxRate")}
              <input
                inputMode="decimal"
                value={invoiceForm.rate}
                onChange={(e) => setInv({ ...invoiceForm, rate: e.target.value })}
              />
            </label>
            <label>
              {t("procurement:invoice.rounding")}
              <select
                value={invoiceForm.rounding}
                onChange={(e) => setInv({ ...invoiceForm, rounding: e.target.value })}
              >
                {ROUNDINGS.map((r) => (
                  <option key={r} value={r}>
                    {t(`procurement:rounding.${r}`)}
                  </option>
                ))}
              </select>
            </label>
            <label>
              {t("procurement:invoice.tax")}
              <input
                inputMode="decimal"
                value={invoiceForm.tax}
                onChange={(e) => setInv({ ...invoiceForm, tax: e.target.value })}
              />
            </label>
          </div>
          <p className="muted small">{t("procurement:invoice.explain")}</p>
          <div className="row">
            <button
              type="button"
              className="primary"
              disabled={!invoiceForm.number.trim() || !invoiceForm.tax.trim() || action.busy}
              onClick={() => void recordInvoice()}
            >
              {t("procurement:invoice.recordAndMatch")}
            </button>
          </div>
        </div>
      )}

      {data.invoice && (
        <div className="stack">
          <div className="row">
            <h3>{t("procurement:invoice.heading", { number: data.invoice.supplier_invoice_number })}</h3>
            <StatusBadge status={data.invoice.match_status} />
          </div>
          <div className="grid-2 small">
            <div>
              {t("procurement:invoice.date")}: {formatDate(data.invoice.invoice_date, i18n.language)}
            </div>
            <div>
              {t("procurement:invoice.total")}:{" "}
              <Money minor={data.invoice.total_minor} currency={data.invoice.currency} />
            </div>
          </div>
          {data.discrepancies.length > 0 && (
            <table className="table">
              <thead>
                <tr>
                  <th>{t("procurement:invoice.difference")}</th>
                  <th className="num">{t("procurement:invoice.expected")}</th>
                  <th className="num">{t("procurement:invoice.actual")}</th>
                  <th>{t("procurement:invoice.resolution")}</th>
                </tr>
              </thead>
              <tbody>
                {data.discrepancies.map((d) => {
                  const money = ["price_mismatch", "shipping_mismatch", "tax_mismatch"].includes(d.kind);
                  const show = (v: number | null | undefined) =>
                    v === null || v === undefined ? (
                      "—"
                    ) : money ? (
                      <Money minor={v} currency={data.invoice?.currency ?? cur} />
                    ) : (
                      String(v)
                    );
                  const r = resolution[d.id] ?? { kind: DEFAULT_RESOLUTION, note: "" };
                  return (
                    <tr key={d.id}>
                      <td>
                        {t(`procurement:discrepancy.${d.kind}`, { defaultValue: d.kind })}
                        {d.line_no !== null && d.line_no !== undefined && (
                          <span className="muted small">{` · ${t("procurement:item.line")} ${d.line_no}`}</span>
                        )}
                      </td>
                      <td className="num">{show(d.expected)}</td>
                      <td className="num">{show(d.actual)}</td>
                      <td>
                        {d.status === RESOLVED ? (
                          <span className="small">
                            <StatusBadge status={RESOLVED} />{" "}
                            {t(`procurement:resolution.${d.resolution ?? "other"}`)} · {d.resolution_note}
                          </span>
                        ) : has(P.manageInvoices) ? (
                          <div className="row">
                            <select
                              aria-label={t("procurement:invoice.resolution")}
                              value={r.kind}
                              onChange={(e) =>
                                setResolution({ ...resolution, [d.id]: { ...r, kind: e.target.value } })
                              }
                            >
                              {RESOLUTIONS.map((k) => (
                                <option key={k} value={k}>
                                  {t(`procurement:resolution.${k}`)}
                                </option>
                              ))}
                            </select>
                            <input
                              aria-label={t("procurement:invoice.resolutionNote")}
                              placeholder={t("procurement:invoice.resolutionNote")}
                              value={r.note}
                              onChange={(e) =>
                                setResolution({ ...resolution, [d.id]: { ...r, note: e.target.value } })
                              }
                            />
                            <button
                              type="button"
                              disabled={!r.note.trim() || action.busy}
                              onClick={() => void resolve(d.id)}
                            >
                              {t("procurement:invoice.resolve")}
                            </button>
                          </div>
                        ) : (
                          <StatusBadge status={OPEN} />
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
        </div>
      )}
      {status === "reconciled" && has(P.manageInvoices) && (
        <div className="row">
          <button
            type="button"
            className="primary"
            disabled={action.busy}
            onClick={() =>
              void action.run("POST", `${org}/requests/${requestId}/close`).then((r) => r && refresh())
            }
          >
            {t("procurement:order.close")}
          </button>
        </div>
      )}
      <ErrorText code={action.error} />
    </div>
  );
}
