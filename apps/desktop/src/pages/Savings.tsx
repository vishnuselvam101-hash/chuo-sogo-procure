import { useTranslation } from "react-i18next";
import { bpsToPercent, formatDate, formatMilliPrice } from "../lib/format";
import { P } from "../lib/perm";
import type { MembershipView, SavingsView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { ErrorText, Loading, Money, PageHeader } from "../components/ui";

/**
 * How Chuo-Sogo is paid: an agreed share of the amount each price went down from the price
 * the factory paid before. The factory sees every line, the baseline it was measured
 * against, and confirms baselines that did not come from its own records.
 */
export function Savings({ active }: { active: MembershipView }) {
  const { t, i18n } = useTranslation();
  const base = `/api/v1/orgs/${active.org_id}/savings`;
  const { data, error, reload } = useApi<SavingsView>(base);
  const action = useAction();
  const approver = active.access !== "assigned" && active.permissions.includes(P.approveRecommendation);
  const lang = i18n.language;

  async function confirm(id: string) {
    if (await action.run("POST", `${base}/baselines/${id}/confirm`)) reload();
  }

  return (
    <section className="stack">
      <PageHeader title={t("billing:savings.title")} />
      <Loading data={data} error={error} />
      {data && (
        <>
          <div className="card stack">
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
            <p className="muted small">{t("billing:savings.rule")}</p>
            {data.totals.map((x) => (
              <div className="grid-2" key={x.currency}>
                <div>
                  <div className="muted">{t("billing:savings.saved")}</div>
                  <Money minor={x.saving_minor} currency={x.currency} />
                </div>
                <div>
                  <div className="muted">{t("billing:savings.fee")}</div>
                  <Money minor={x.fee_minor} currency={x.currency} />
                  {x.unbilled_fee_minor > 0 && (
                    <div className="muted small">
                      {t("billing:savings.unbilled")}{" "}
                      <Money minor={x.unbilled_fee_minor} currency={x.currency} />
                    </div>
                  )}
                </div>
              </div>
            ))}
          </div>

          <h2>{t("billing:savings.baselines")}</h2>
          <p className="muted small">{t("billing:savings.baselinesHint")}</p>
          {data.baselines.length === 0 ? (
            <p className="muted">{t("billing:savings.noBaselines")}</p>
          ) : (
            <table className="table" data-testid="savings-baselines">
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
                      ) : approver ? (
                        <button type="button" disabled={action.busy} onClick={() => void confirm(b.id)}>
                          {t("billing:savings.confirm")}
                        </button>
                      ) : (
                        t("billing:savings.awaitingFactory")
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <ErrorText code={action.error} />

          <h2>{t("billing:savings.ledger")}</h2>
          {data.entries.length === 0 ? (
            <p className="muted">{t("billing:savings.noEntries")}</p>
          ) : (
            <table className="table" data-testid="savings-ledger">
              <thead>
                <tr>
                  <th>{t("billing:savings.ordered")}</th>
                  <th>{t("billing:savings.order")}</th>
                  <th>{t("billing:savings.item")}</th>
                  <th className="num">{t("billing:savings.pricePaidBefore")}</th>
                  <th className="num">{t("billing:savings.priceNow")}</th>
                  <th className="num">{t("billing:savings.saved")}</th>
                  <th className="num">{t("billing:savings.fee")}</th>
                  <th>{t("billing:statusLabel")}</th>
                </tr>
              </thead>
              <tbody>
                {data.entries.map((e) => (
                  <tr key={e.id}>
                    <td>{formatDate(e.ordered_on, lang)}</td>
                    <td>
                      {e.po_number} <span className="muted small">{e.request_number}</span>
                    </td>
                    <td>
                      {e.description} × {e.quantity}
                    </td>
                    <td className="num">
                      {e.baseline_unit_price_milli === null || e.baseline_unit_price_milli === undefined
                        ? "—"
                        : formatMilliPrice(e.baseline_unit_price_milli, e.currency, lang)}
                    </td>
                    <td className="num">
                      <Money minor={e.unit_price_minor} currency={e.currency} />
                    </td>
                    <td className="num">
                      <Money minor={e.saving_minor} currency={e.currency} />
                    </td>
                    <td className="num">
                      <Money minor={e.fee_minor} currency={e.currency} />
                    </td>
                    <td>
                      <span className={`badge badge-${e.status === "counted" ? "approved" : "pending"}`}>
                        {t(`billing:savings.status.${e.status}`)}
                      </span>
                      {e.invoice_number && <div className="muted small">{e.invoice_number}</div>}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      )}
    </section>
  );
}
