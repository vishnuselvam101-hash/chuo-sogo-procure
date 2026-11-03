import { useState } from "react";
import { useTranslation } from "react-i18next";
import { bpsToPercent, formatDate, formatMilliPrice } from "../lib/format";
import type { SavingsView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { ErrorText, Loading, Money } from "../components/ui";
import { ADMIN_API } from "./types";

/**
 * The savings-share terms with one factory, the prices it paid before (its own records, or
 * agreed and confirmed by the factory), the ledger, and the monthly fee invoice draft.
 */
export function SavingsAdmin({ orgId }: { orgId: string }) {
  const { t, i18n } = useTranslation();
  const base = `${ADMIN_API}/orgs/${orgId}/savings`;
  const { data, error, reload } = useApi<SavingsView>(base);
  const action = useAction();
  const lang = i18n.language;
  const [terms, setTerms] = useState({ starts_on: "", share: "50", months: "36", notes: "" });
  const [baseline, setBaseline] = useState({ mpn: "", description: "", price: "", unit: "", note: "" });
  const [month, setMonth] = useState("");
  const [fixResult, setFixResult] = useState<{ fixed: number; waiting_for_factory: number } | null>(null);
  const [created, setCreated] = useState<string | null>(null);

  if (!data) return <Loading data={data} error={error} />;
  const fixed = data.entries.length > 0;

  async function saveTerms() {
    const share = Math.round(Number(terms.share) * 100);
    const months = Number(terms.months);
    if (!terms.starts_on) return action.setError("validation_failed/starts_on");
    if (!Number.isInteger(share)) return action.setError("validation_failed/share_bps");
    if (
      await action.run("PUT", `${base}/agreement`, {
        starts_on: terms.starts_on,
        share_bps: share,
        term_months: months,
        notes: terms.notes,
      })
    )
      reload();
  }

  async function addBaseline() {
    // Yen per unit as typed ("1180" or "1180.5") → thousandths of a minor unit.
    const m = /^(\d+)(?:\.(\d{1,3}))?$/.exec(baseline.price.trim());
    if (!m) return action.setError("validation_failed/unit_price_milli");
    const milli = Number(m[1]) * 1000 + Number((m[2] ?? "").padEnd(3, "0"));
    if (
      await action.run("POST", `${base}/baselines`, {
        mpn: baseline.mpn,
        description: baseline.description,
        unit_price_milli: milli,
        unit: baseline.unit,
        note: baseline.note,
      })
    ) {
      setBaseline({ mpn: "", description: "", price: "", unit: "", note: "" });
      reload();
    }
  }

  async function removeBaseline(id: string) {
    if (await action.run("DELETE", `${base}/baselines/${id}`)) reload();
  }

  async function invoice() {
    const res = await action.run<{ id: string }>("POST", `${base}/invoice`, { month });
    if (res) {
      setCreated(res.body.id);
      reload();
    }
  }

  async function fixBaselines() {
    const res = await action.run<{ fixed: number; waiting_for_factory: number }>(
      "POST",
      `${base}/baselines/from-records`,
    );
    if (res) {
      setFixResult(res.body);
      reload();
    }
  }

  return (
    <div className="stack">
      <div className="card stack">
        <h2>{t("billing:savings.terms_heading")}</h2>
        {data.agreement ? (
          <p>
            {t("billing:savings.terms", {
              share: bpsToPercent(data.agreement.share_bps),
              months: data.agreement.term_months,
              from: formatDate(data.agreement.starts_on, lang),
              to: formatDate(data.agreement.ends_on, lang),
            })}
          </p>
        ) : (
          <p className="muted">{t("billing:savings.noAgreement")}</p>
        )}
        {fixed ? (
          <p className="muted small">{t("billing:savings.termsFixed")}</p>
        ) : (
          <div className="row">
            <label>
              {t("billing:savings.startsOn")}
              <input
                type="date"
                value={terms.starts_on}
                onChange={(e) => setTerms({ ...terms, starts_on: e.target.value })}
              />
            </label>
            <label>
              {t("billing:savings.sharePercent")}
              <input
                inputMode="decimal"
                value={terms.share}
                onChange={(e) => setTerms({ ...terms, share: e.target.value })}
              />
            </label>
            <label>
              {t("billing:savings.months")}
              <input
                inputMode="numeric"
                value={terms.months}
                onChange={(e) => setTerms({ ...terms, months: e.target.value })}
              />
            </label>
            <label className="grow">
              {t("billing:savings.notes")}
              <input
                maxLength={2000}
                value={terms.notes}
                onChange={(e) => setTerms({ ...terms, notes: e.target.value })}
              />
            </label>
            <button type="button" className="primary" disabled={action.busy} onClick={() => void saveTerms()}>
              {t("action.save")}
            </button>
          </div>
        )}
        {data.totals.map((x) => (
          <p key={x.currency}>
            {t("billing:savings.saved")} <Money minor={x.saving_minor} currency={x.currency} /> ·{" "}
            {t("billing:savings.fee")} <Money minor={x.fee_minor} currency={x.currency} /> ·{" "}
            {t("billing:savings.unbilled")} <Money minor={x.unbilled_fee_minor} currency={x.currency} />
          </p>
        ))}
        <div className="row">
          <label>
            {t("billing:savings.month")}
            <input type="month" value={month} onChange={(e) => setMonth(e.target.value)} />
          </label>
          <button type="button" disabled={action.busy || !month} onClick={() => void invoice()}>
            {t("billing:savings.createInvoice")}
          </button>
        </div>
        {created && <p className="notice">{t("billing:savings.invoiceCreated")}</p>}
      </div>

      <div className="card stack">
        <h2>{t("billing:savings.baselines")}</h2>
        <div className="row">
          <button type="button" disabled={action.busy || !data.agreement} onClick={() => void fixBaselines()}>
            {t("billing:savings.fixFromRecords")}
          </button>
          {fixResult && (
            <span className="muted small">
              {t("billing:savings.fixedFromRecords", {
                fixed: fixResult.fixed,
                waiting: fixResult.waiting_for_factory,
              })}
            </span>
          )}
        </div>
        <p className="muted small">{t("billing:savings.agreedHint")}</p>
        <table className="table">
          <thead>
            <tr>
              <th>{t("billing:savings.item")}</th>
              <th className="num">{t("billing:savings.pricePaidBefore")}</th>
              <th>{t("billing:savings.source")}</th>
              <th>{t("billing:statusLabel")}</th>
            </tr>
          </thead>
          <tbody>
            {data.baselines.map((b) => (
              <tr key={b.id}>
                <td>{b.item_key.slice(2)}</td>
                <td className="num">
                  {formatMilliPrice(b.unit_price_milli, b.currency, lang)}
                  {b.unit ? ` / ${b.unit}` : ""}
                </td>
                <td>
                  {b.source === "history"
                    ? t("billing:savings.fromHistory", {
                        date: b.purchased_on ? formatDate(b.purchased_on, lang) : "",
                      })
                    : b.note}
                </td>
                <td>
                  {b.confirmed_at ? (
                    t("billing:savings.confirmed")
                  ) : (
                    <>
                      {t("billing:savings.awaitingFactory")}{" "}
                      {b.used === 0 && (
                        <button type="button" className="link" onClick={() => void removeBaseline(b.id)}>
                          {t("action.remove")}
                        </button>
                      )}
                    </>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        <div className="row">
          <label>
            {t("billing:savings.mpn")}
            <input
              value={baseline.mpn}
              maxLength={100}
              onChange={(e) => setBaseline({ ...baseline, mpn: e.target.value })}
            />
          </label>
          <label className="grow">
            {t("billing:savings.description")}
            <input
              value={baseline.description}
              maxLength={1000}
              onChange={(e) => setBaseline({ ...baseline, description: e.target.value })}
            />
          </label>
          <label>
            {t("billing:savings.pricePaidBefore")}
            <input
              inputMode="decimal"
              value={baseline.price}
              onChange={(e) => setBaseline({ ...baseline, price: e.target.value })}
            />
          </label>
          <label>
            {t("billing:savings.unit")}
            <input
              value={baseline.unit}
              maxLength={16}
              onChange={(e) => setBaseline({ ...baseline, unit: e.target.value })}
            />
          </label>
        </div>
        <div className="row">
          <label className="grow">
            {t("billing:savings.evidence")}
            <input
              value={baseline.note}
              maxLength={1000}
              onChange={(e) => setBaseline({ ...baseline, note: e.target.value })}
            />
          </label>
          <button
            type="button"
            disabled={action.busy || !baseline.price || !baseline.note.trim()}
            onClick={() => void addBaseline()}
          >
            {t("billing:savings.addBaseline")}
          </button>
        </div>
      </div>
      <ErrorText code={action.error} />
    </div>
  );
}
