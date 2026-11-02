import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  bpsToPercent,
  formatDate,
  formatMilliPrice,
  formatMoney,
  formatNumber,
  minorToInput,
  parseMinor,
} from "../lib/format";
import type {
  ComparisonResult,
  ComparisonView,
  ItemView,
  MembershipView,
  OutboxMessage,
  PriceCheck,
  RfqDetail,
} from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { ErrorText, Loading, Money, StatusBadge } from "../components/ui";
import { CURRENCIES } from "./NewRequest";
import { AiFlags } from "./AiItems";
import { P } from "../lib/perm";

interface Props {
  active: MembershipView;
  rfqId: string;
  requestId: string;
  requestStatus: string;
  currency: string;
  items: ItemView[];
  onChanged: () => void;
}

const QUOTING = ["rfq_sent", "quotes_received", "under_comparison"];

export function RfqPanel({ active, rfqId, requestId, requestStatus, currency, items, onChanged }: Props) {
  const { t, i18n } = useTranslation();
  const org = `/api/v1/orgs/${active.org_id}`;
  const { data, error, reload } = useApi<RfqDetail>(`${org}/rfqs/${rfqId}`);
  const action = useAction();
  const [outbox, setOutbox] = useState<OutboxMessage[] | null>(null);
  const [quoting, setQuoting] = useState(false);
  const has = (p: string) => active.permissions.includes(p);
  const operator = active.access === "assigned";

  if (!data) return <Loading data={data} error={error} />;
  const rfq = data.rfq;
  const refresh = () => {
    reload();
    onChanged();
  };

  async function dispatch() {
    if (await action.run("POST", `${org}/rfqs/${rfqId}/dispatch`)) refresh();
  }
  async function showOutbox() {
    const res = await action.run<OutboxMessage[]>("GET", `${org}/rfqs/${rfqId}/outbox`);
    if (res) setOutbox(res.body);
  }
  async function compare() {
    if (await action.run("POST", `${org}/rfqs/${rfqId}/comparisons`)) refresh();
  }

  return (
    <div className="card stack">
      <div className="row">
        <strong>{rfq.number}</strong>
        <StatusBadge status={rfq.status} />
        {rfq.response_due && (
          <span className="muted">
            {t("procurement:rfq.dueOn", { date: formatDate(rfq.response_due, i18n.language) })}
          </span>
        )}
      </div>
      <ul className="tags">
        {data.recipients.map((r) => (
          <li key={r.supplier_id}>
            {r.supplier_name ?? ""} <StatusBadge status={r.status} />
          </li>
        ))}
      </ul>

      {operator && (
        <div className="row">
          {rfq.status === "draft" && has(P.manageRfqs) && (
            <button type="button" className="primary" onClick={() => void dispatch()}>
              {t("procurement:rfq.dispatch")}
            </button>
          )}
          {rfq.status !== "draft" && has(P.manageRfqs) && (
            <button type="button" onClick={() => void (outbox ? setOutbox(null) : showOutbox())}>
              {t("procurement:rfq.outbox")}
            </button>
          )}
          {rfq.status === "sent" && has(P.recordQuotations) && QUOTING.includes(requestStatus) && (
            <button type="button" onClick={() => setQuoting(!quoting)}>
              {t("procurement:quote.record")}
            </button>
          )}
          {has(P.manageRfqs) && data.quotations.length > 0 && QUOTING.slice(1).includes(requestStatus) && (
            <button type="button" className="primary" onClick={() => void compare()}>
              {t("procurement:comparison.run")}
            </button>
          )}
        </div>
      )}
      {rfq.status === "draft" && operator && (
        <p className="muted small">{t("procurement:rfq.testModeNote")}</p>
      )}
      <ErrorText code={action.error} />

      {outbox && (
        <div className="stack">
          <p className="muted small">{t("procurement:rfq.testModeNote")}</p>
          {outbox.map((m) => (
            <details key={m.id}>
              <summary>
                {m.supplier_name} {m.to_address ? `<${m.to_address}>` : ""}
              </summary>
              <pre className="pre">{`${m.subject}\n\n${m.body}`}</pre>
            </details>
          ))}
        </div>
      )}

      {quoting && (
        <QuotationForm
          base={`${org}/rfqs/${rfqId}`}
          recipients={data.recipients.map((r) => ({ id: r.supplier_id, name: r.supplier_name ?? "" }))}
          currency={currency}
          items={items}
          onSaved={() => {
            setQuoting(false);
            refresh();
          }}
        />
      )}

      {data.quotations.length > 0 && (
        <table className="table">
          <thead>
            <tr>
              <th>{t("procurement:quote.supplier")}</th>
              <th>{t("procurement:quote.reference")}</th>
              <th>{t("procurement:quote.validUntil")}</th>
              <th className="num">{t("procurement:quote.shipping")}</th>
              <th>{t("procurement:quote.lines")}</th>
            </tr>
          </thead>
          <tbody>
            {data.quotations.map((q) => (
              <tr key={q.id}>
                <td>{q.supplier_name ?? ""}</td>
                <td>{q.supplier_reference}</td>
                <td>{q.valid_until ? formatDate(q.valid_until, i18n.language) : "—"}</td>
                <td className="num">
                  <Money minor={q.shipping_minor} currency={q.currency} />
                </td>
                <td>
                  <ul className="plain small">
                    {q.lines.map((l) => {
                      const item = items.find((i) => i.id === l.request_item_id);
                      return (
                        <li key={l.request_item_id}>
                          {t("procurement:quote.lineSummary", {
                            line: item?.line_no ?? "?",
                            quantity: formatNumber(l.quantity, i18n.language),
                            days: l.lead_time_days ?? "—",
                          })}{" "}
                          <Money minor={l.unit_price_minor} currency={q.currency} />
                          {l.alternative && (
                            <span className="badge badge-warn">{t("procurement:quote.alternative")}</span>
                          )}
                        </li>
                      );
                    })}
                  </ul>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {data.latest_comparison_id && (
        <LatestComparison
          active={active}
          comparisonId={data.latest_comparison_id}
          requestId={requestId}
          items={items}
          canRecommend={operator && has(P.operateRequests) && requestStatus === "under_comparison"}
          onChanged={refresh}
        />
      )}
    </div>
  );
}

function LatestComparison(props: {
  active: MembershipView;
  comparisonId: string;
  requestId: string;
  items: ItemView[];
  canRecommend: boolean;
  onChanged: () => void;
}) {
  const { t } = useTranslation();
  const org = `/api/v1/orgs/${props.active.org_id}`;
  const { data, error } = useApi<ComparisonView>(`${org}/comparisons/${props.comparisonId}`);
  const action = useAction();
  const [reason, setReason] = useState("");
  if (!data) return <Loading data={data} error={error} />;

  async function recommend(quotationId: string) {
    const res = await action.run("POST", `${org}/requests/${props.requestId}/recommendations`, {
      comparison_id: props.comparisonId,
      quotation_id: quotationId,
      reason,
    });
    if (res) props.onChanged();
  }

  return (
    <div className="stack">
      <h3>{t("procurement:comparison.heading")}</h3>
      <ComparisonTable
        results={data.results}
        currency={data.currency}
        items={props.items}
        action={
          props.canRecommend
            ? (r) =>
                r.compliance === "compliant" ? (
                  <button type="button" disabled={action.busy} onClick={() => void recommend(r.quotation_id)}>
                    {t("procurement:comparison.recommend")}
                  </button>
                ) : null
            : undefined
        }
      />
      {props.canRecommend && (
        <label>
          {t("procurement:comparison.reason")}
          <input maxLength={2000} value={reason} onChange={(e) => setReason(e.target.value)} />
        </label>
      )}
      <ErrorText code={action.error} />
    </div>
  );
}

/** Deterministic comparison result as computed by the backend; the UI only displays it. */
export function ComparisonTable({
  results,
  currency,
  items,
  action,
}: {
  results: ComparisonResult[];
  currency: string;
  items?: ItemView[] | undefined;
  action?: ((r: ComparisonResult) => React.ReactNode) | undefined;
}) {
  const { t, i18n } = useTranslation();
  const withChecks = results.some((r) => (r.price_checks?.length ?? 0) > 0);
  const alertBps = results.find(
    (r) => r.price_alert_bps !== null && r.price_alert_bps !== undefined,
  )?.price_alert_bps;
  const cols = 5 + (withChecks ? 1 : 0) + (action ? 1 : 0);
  return (
    <>
      <table className="table">
        <thead>
          <tr>
            <th className="num">{t("procurement:comparison.rank")}</th>
            <th>{t("procurement:quote.supplier")}</th>
            <th>{t("procurement:comparison.compliance")}</th>
            <th className="num">{t("procurement:comparison.total")}</th>
            <th className="num">{t("procurement:comparison.leadTime")}</th>
            {withChecks && (
              <th className="num">
                {t("procurement:priceCheck.alerts")}
                {alertBps !== null && alertBps !== undefined && (
                  <div className="muted small">
                    {t("procurement:priceCheck.alertLine", { pct: signedPercent(alertBps) })}
                  </div>
                )}
              </th>
            )}
            {action && <th />}
          </tr>
        </thead>
        <tbody>
          {results.map((r) => (
            <PriceRows
              key={r.quotation_id}
              checks={r.price_checks ?? []}
              cols={cols}
              result={r}
              items={items}
            >
              <td className="num">{r.rank ?? "—"}</td>
              <td>{r.supplier_name}</td>
              <td>
                <StatusBadge status={r.compliance} />
                {r.reasons.length > 0 && (
                  <div className="muted small">
                    {r.reasons
                      .map((code) => t(`procurement:reason.${code}`, { defaultValue: code }))
                      .join(", ")}
                  </div>
                )}
              </td>
              <td className="num">
                <Money minor={r.total_minor} currency={currency} />
              </td>
              <td className="num">
                {r.max_lead_time_days === null || r.max_lead_time_days === undefined
                  ? "—"
                  : t("procurement:comparison.days", {
                      n: formatNumber(r.max_lead_time_days, i18n.language),
                    })}
              </td>
              {withChecks && (
                <td className={`num${(r.price_alerts ?? 0) > 0 ? " error" : ""}`}>
                  {(r.price_checks?.length ?? 0) > 0
                    ? t("procurement:priceCheck.alertCount", {
                        n: formatNumber(r.price_alerts ?? 0, i18n.language),
                      })
                    : "—"}
                </td>
              )}
              {action && <td className="row-actions">{action(r)}</td>}
            </PriceRows>
          ))}
        </tbody>
      </table>
      <DecisionMemoPanel results={results} />
    </>
  );
}

/**
 * The decision memo (LAYA method): each quotation scored against the customer's own priorities,
 * with risk penalties and the reasons. A recommendation for the person deciding; nothing here
 * chooses, orders or approves.
 */
export function DecisionMemoPanel({ results }: { results: ComparisonResult[] }) {
  const { t, i18n } = useTranslation();
  const withMemo = results.filter((r) => r.memo);
  if (withMemo.length === 0) return null;
  const ranked = withMemo
    .filter((r) => r.memo?.rank != null)
    .sort((a, b) => (a.memo?.rank ?? 0) - (b.memo?.rank ?? 0));
  const unranked = withMemo.filter((r) => r.memo?.rank == null);
  const num = (n: number) => formatNumber(n, i18n.language);
  const cautions = (r: ComparisonResult) =>
    (r.memo?.cautions ?? []).map((c) => t(`procurement:memo.caution.${c}`, { defaultValue: c })).join(" · ");
  return (
    <div className="card stack memo">
      <h3>{t("procurement:memo.heading")}</h3>
      {ranked.length > 0 && ranked[0] ? (
        <p>
          <strong>{t("procurement:memo.recommended", { supplier: ranked[0].supplier_name })}</strong>
        </p>
      ) : (
        <p className="muted">{t("procurement:memo.none")}</p>
      )}
      {ranked.length > 0 && (
        <table className="table">
          <thead>
            <tr>
              <th className="num">{t("procurement:memo.rank")}</th>
              <th>{t("procurement:quote.supplier")}</th>
              <th className="num">{t("procurement:memo.score")}</th>
              <th>{t("procurement:memo.factors")}</th>
              <th>{t("procurement:memo.cautions")}</th>
            </tr>
          </thead>
          <tbody>
            {ranked.map((r) => (
              <tr key={r.quotation_id}>
                <td className="num">{r.memo?.rank}</td>
                <td>
                  {r.supplier_name}
                  {r.memo?.preferred_supplier && (
                    <div className="muted small">{t("procurement:memo.preferred")}</div>
                  )}
                  {(r.memo?.past_purchases ?? 0) > 0 && (
                    <div className="muted small">
                      {t("procurement:memo.pastPurchases", { n: num(r.memo?.past_purchases ?? 0) })}
                    </div>
                  )}
                </td>
                <td className="num">{t("procurement:memo.outOf", { n: num(r.memo?.score ?? 0) })}</td>
                <td className="small">
                  {(r.memo?.factors ?? [])
                    .map((f) =>
                      t("procurement:memo.factor", {
                        name: t(`history:priority.${f.factor}`, { defaultValue: f.factor }),
                        points: num(f.points),
                        weight: num(f.weight),
                      }),
                    )
                    .join(" · ")}
                </td>
                <td className={`small${(r.memo?.cautions.length ?? 0) > 0 ? " error" : ""}`}>
                  {cautions(r) || "—"}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {unranked.length > 0 && (
        <ul className="plain small">
          {unranked.map((r) => (
            <li key={r.quotation_id}>
              {t("procurement:memo.notRanked", { supplier: r.supplier_name })}: {cautions(r)}
            </li>
          ))}
        </ul>
      )}
      <p className="muted small">{t("procurement:memo.note")}</p>
    </div>
  );
}

/** "+9.09", "-5", "0": basis points as a signed percentage. */
function signedPercent(bps: number): string {
  if (bps === 0) return "0";
  return `${bps > 0 ? "+" : "-"}${bpsToPercent(Math.abs(bps))}`;
}

/** A comparison row, followed by its past-price checks (reference only, never a decision). */
function PriceRows({
  result,
  checks,
  cols,
  items,
  children,
}: {
  result: ComparisonResult;
  checks: PriceCheck[];
  cols: number;
  items?: ItemView[] | undefined;
  children: React.ReactNode;
}) {
  const { t, i18n } = useTranslation();
  const lang = i18n.language;
  function itemLabel(id: string): string {
    const item = items?.find((i) => i.id === id);
    return item ? `${item.line_no}. ${item.description}` : t("procurement:priceCheck.unknownItem");
  }
  return (
    <>
      <tr>{children}</tr>
      {checks.length > 0 && (
        <tr className="price-checks">
          <td colSpan={cols}>
            <ul className="small">
              {checks.map((c) => (
                <li key={c.request_item_id} className={c.above_alert ? "error" : undefined}>
                  {t("procurement:priceCheck.line", {
                    item: itemLabel(c.request_item_id),
                    quoted: formatMoney(c.quoted_unit_price_minor, result.currency, lang),
                    last: formatMilliPrice(c.last_unit_price_milli, c.last_currency, lang),
                    supplier: c.last_supplier_name || t("procurement:priceCheck.unknownSupplier"),
                    date: formatDate(c.last_purchased_on, lang),
                  })}{" "}
                  <strong>
                    {c.change_bps === null || c.change_bps === undefined
                      ? t("procurement:priceCheck.noFx")
                      : t("procurement:priceCheck.change", { pct: signedPercent(c.change_bps) })}
                  </strong>
                  {c.above_alert && <> {t("procurement:priceCheck.aboveAlert")}</>}
                </li>
              ))}
            </ul>
          </td>
        </tr>
      )}
    </>
  );
}

interface LineDraft {
  price: string;
  quantity: string;
  lead: string;
  mpn: string;
  alternative: boolean;
}

function QuotationForm(props: {
  base: string;
  recipients: { id: string; name: string }[];
  currency: string;
  items: ItemView[];
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const action = useAction();
  const [head, setHead] = useState({
    supplier_id: props.recipients[0]?.id ?? "",
    currency: props.currency,
    shipping: minorToInput(0, props.currency),
    valid_until: "",
    reference: "",
  });
  const [lines, setLines] = useState<LineDraft[]>(
    props.items.map((i) => ({
      price: "",
      quantity: String(i.quantity),
      lead: "",
      mpn: "",
      alternative: false,
    })),
  );
  const setLine = (i: number, patch: Partial<LineDraft>) =>
    setLines(lines.map((l, j) => (j === i ? { ...l, ...patch } : l)));
  const [aiText, setAiText] = useState("");
  const [ai, setAi] = useState<{
    id: string;
    supplier: string;
    flags: string[];
    lineFlags: string[][];
  } | null>(null);

  /** Pre-fills the form from the supplier's text. The operator checks and saves; nothing is recorded by the AI. */
  async function readWithAi() {
    const res = await action.run<{
      id: string;
      status: string;
      error_code?: string | null;
      output?: unknown;
    }>("POST", `${props.base}/ai/extract-quotation`, { supplier_id: head.supplier_id, text: aiText });
    if (!res) return;
    if (res.body.status === "failed" || !res.body.output) {
      action.setError(`conflict/${res.body.error_code ?? "ai_unavailable"}`);
      return;
    }
    const q = res.body.output as {
      currency: string;
      shipping_minor: number;
      valid_until?: string | null;
      supplier_reference: string;
      flags: string[];
      lines: {
        line_no: number;
        unit_price_minor: number;
        quantity: number;
        lead_time_days?: number | null;
        offered_mpn: string;
        alternative: boolean;
        flags: string[];
      }[];
    };
    if (q.lines.length === 0) {
      setAi(null);
      action.setError("conflict/ai_nothing_found");
      return;
    }
    setHead({
      ...head,
      currency: q.currency,
      shipping: minorToInput(q.shipping_minor, q.currency),
      valid_until: q.valid_until ?? "",
      reference: q.supplier_reference,
    });
    setLines(
      props.items.map((item, i) => {
        const l = q.lines.find((x) => x.line_no === item.line_no);
        return l
          ? {
              price: minorToInput(l.unit_price_minor, q.currency),
              quantity: String(l.quantity),
              lead:
                l.lead_time_days === null || l.lead_time_days === undefined ? "" : String(l.lead_time_days),
              mpn: l.offered_mpn,
              alternative: l.alternative,
            }
          : (lines[i] ?? {
              price: "",
              quantity: String(item.quantity),
              lead: "",
              mpn: "",
              alternative: false,
            });
      }),
    );
    setAi({
      id: res.body.id,
      supplier: head.supplier_id,
      flags: q.flags,
      lineFlags: props.items.map((item) => q.lines.find((x) => x.line_no === item.line_no)?.flags ?? []),
    });
  }

  async function save() {
    const shipping = parseMinor(head.shipping, head.currency);
    const parsed = lines.map((l, i) => {
      const unit = parseMinor(l.price, head.currency);
      const qty = /^\d{1,10}$/.test(l.quantity.trim()) ? Number(l.quantity) : null;
      const lead = l.lead.trim() === "" ? null : /^\d{1,4}$/.test(l.lead.trim()) ? Number(l.lead) : NaN;
      if (unit === null || !qty || Number.isNaN(lead)) return null;
      return {
        request_item_id: props.items[i]?.id,
        unit_price_minor: unit,
        quantity: qty,
        lead_time_days: lead,
        offered_mpn: l.mpn,
        alternative: l.alternative,
      };
    });
    if (shipping === null || parsed.some((p) => p === null)) {
      action.setError("validation_failed/amount");
      return;
    }
    const res = await action.run("POST", `${props.base}/quotations`, {
      supplier_id: head.supplier_id,
      supplier_reference: head.reference,
      currency: head.currency,
      shipping_minor: shipping,
      valid_until: head.valid_until || null,
      lines: parsed,
      // Provenance only if the form still belongs to the supplier the AI read.
      ai_suggestion_id: ai && ai.supplier === head.supplier_id ? ai.id : null,
    });
    if (res) props.onSaved();
  }

  return (
    <div className="stack sub-card">
      <details>
        <summary>{t("procurement:ai.quoteTitle")}</summary>
        <div className="stack">
          <label>
            {t("procurement:ai.supplierText")}
            <textarea rows={4} maxLength={20000} value={aiText} onChange={(e) => setAiText(e.target.value)} />
          </label>
          <div className="row">
            <button
              type="button"
              disabled={!aiText.trim() || !head.supplier_id || action.busy}
              onClick={() => void readWithAi()}
            >
              {action.busy ? t("procurement:ai.working") : t("procurement:ai.readQuote")}
            </button>
          </div>
          {ai && <p className="notice">{t("procurement:ai.prefilled")}</p>}
          {ai && <AiFlags flags={ai.flags} />}
        </div>
      </details>
      <div className="row">
        <label>
          {t("procurement:quote.supplier")}
          <select
            value={head.supplier_id}
            onChange={(e) => {
              // Values read for one supplier must never stay in another supplier's form.
              if (ai) {
                setAi(null);
                setLines(
                  props.items.map((i) => ({
                    price: "",
                    quantity: String(i.quantity),
                    lead: "",
                    mpn: "",
                    alternative: false,
                  })),
                );
                setHead({
                  ...head,
                  supplier_id: e.target.value,
                  shipping: minorToInput(0, head.currency),
                  valid_until: "",
                  reference: "",
                });
              } else setHead({ ...head, supplier_id: e.target.value });
            }}
          >
            {props.recipients.map((r) => (
              <option key={r.id} value={r.id}>
                {r.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          {t("procurement:request.currency")}
          <select value={head.currency} onChange={(e) => setHead({ ...head, currency: e.target.value })}>
            {CURRENCIES.map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
          </select>
        </label>
        <label>
          {t("procurement:quote.shipping")}
          <input
            inputMode="decimal"
            value={head.shipping}
            onChange={(e) => setHead({ ...head, shipping: e.target.value })}
          />
        </label>
        <label>
          {t("procurement:quote.validUntil")}
          <input
            type="date"
            value={head.valid_until}
            onChange={(e) => setHead({ ...head, valid_until: e.target.value })}
          />
        </label>
        <label>
          {t("procurement:quote.reference")}
          <input
            maxLength={100}
            value={head.reference}
            onChange={(e) => setHead({ ...head, reference: e.target.value })}
          />
        </label>
      </div>
      <table className="table">
        <thead>
          <tr>
            <th className="num">{t("procurement:item.line")}</th>
            <th>{t("procurement:item.description")}</th>
            <th>{t("procurement:quote.unitPrice")}</th>
            <th>{t("procurement:item.quantity")}</th>
            <th>{t("procurement:quote.leadDays")}</th>
            <th>{t("procurement:quote.offeredMpn")}</th>
            <th>{t("procurement:quote.alternative")}</th>
          </tr>
        </thead>
        <tbody>
          {props.items.map((item, i) => (
            <tr key={item.id}>
              <td className="num">{item.line_no}</td>
              <td>
                {item.description}
                {ai && <AiFlags flags={ai.lineFlags[i] ?? []} />}
              </td>
              <td>
                <input
                  required
                  inputMode="decimal"
                  aria-label={t("procurement:quote.unitPrice")}
                  value={lines[i]?.price ?? ""}
                  onChange={(e) => setLine(i, { price: e.target.value })}
                />
              </td>
              <td>
                <input
                  inputMode="numeric"
                  aria-label={t("procurement:item.quantity")}
                  value={lines[i]?.quantity ?? ""}
                  onChange={(e) => setLine(i, { quantity: e.target.value })}
                />
              </td>
              <td>
                <input
                  inputMode="numeric"
                  aria-label={t("procurement:quote.leadDays")}
                  value={lines[i]?.lead ?? ""}
                  onChange={(e) => setLine(i, { lead: e.target.value })}
                />
              </td>
              <td>
                <input
                  aria-label={t("procurement:quote.offeredMpn")}
                  value={lines[i]?.mpn ?? ""}
                  onChange={(e) => setLine(i, { mpn: e.target.value })}
                />
              </td>
              <td>
                <input
                  type="checkbox"
                  aria-label={t("procurement:quote.alternative")}
                  checked={lines[i]?.alternative ?? false}
                  onChange={(e) => setLine(i, { alternative: e.target.checked })}
                />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="row">
        <button
          type="button"
          className="primary"
          disabled={action.busy || !head.supplier_id}
          onClick={() => void save()}
        >
          {t("action.save")}
        </button>
      </div>
      <ErrorText code={action.error} />
    </div>
  );
}
