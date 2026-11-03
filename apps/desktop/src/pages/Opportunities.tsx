import { useState } from "react";
import { useTranslation } from "react-i18next";
import { bpsToPercent, formatMilliPrice } from "../lib/format";
import type { OpportunityView } from "../lib/types";
import { useApi } from "../lib/useApi";
import { Loading, Money } from "../components/ui";

const PERIODS = [6, 12, 24, 36];

/**
 * Where the factory can save, from its own records: what it paid above the lowest price it
 * already got for each item, and items it always bought from one supplier (never compared).
 * Amounts come from the server; nothing is estimated here.
 */
export function Opportunities({ orgId }: { orgId: string }) {
  const { t, i18n } = useTranslation();
  const [months, setMonths] = useState(12);
  const { data, error } = useApi<OpportunityView>(
    `/api/v1/orgs/${orgId}/purchase-history/opportunities?months=${months}`,
  );
  const lang = i18n.language;

  return (
    <div className="card stack" data-testid="opportunities">
      <h2>{t("history:opportunities.heading")}</h2>
      <p className="muted small">{t("history:opportunities.hint")}</p>
      <label>
        {t("history:opportunities.period")}
        <select value={months} onChange={(e) => setMonths(Number(e.target.value))}>
          {PERIODS.map((m) => (
            <option key={m} value={m}>
              {t("history:opportunities.months", { count: m })}
            </option>
          ))}
        </select>
      </label>
      <Loading data={data} error={error} />
      {data && data.items.length === 0 && <p className="muted">{t("history:opportunities.none")}</p>}
      {data &&
        data.totals.map((x) => (
          <p key={x.currency}>
            {t("history:opportunities.spend")} <Money minor={x.spend_minor} currency={x.currency} /> ·{" "}
            {t("history:opportunities.potential")} <Money minor={x.potential_minor} currency={x.currency} /> ·{" "}
            {t("history:opportunities.fee", { share: bpsToPercent(data.share_bps) })}{" "}
            <Money minor={x.fee_minor} currency={x.currency} />
          </p>
        ))}
      {data && data.items.length > 0 && (
        <table className="table">
          <thead>
            <tr>
              <th>{t("history:opportunities.item")}</th>
              <th className="num">{t("history:opportunities.purchases")}</th>
              <th className="num">{t("history:opportunities.priceRange")}</th>
              <th className="num">{t("history:opportunities.spend")}</th>
              <th className="num">{t("history:opportunities.potential")}</th>
            </tr>
          </thead>
          <tbody>
            {data.items.map((o) => (
              <tr key={`${o.item_key}-${o.currency}`}>
                <td>
                  {o.description || o.item_key.slice(2)}
                  {o.single_supplier && (
                    <div className="muted small">{t("history:opportunities.singleSupplier")}</div>
                  )}
                </td>
                <td className="num">
                  {t("history:opportunities.counts", { purchases: o.purchases, suppliers: o.suppliers })}
                </td>
                <td className="num">
                  {formatMilliPrice(o.min_price_milli, o.currency, lang)} –{" "}
                  {formatMilliPrice(o.max_price_milli, o.currency, lang)}
                </td>
                <td className="num">
                  <Money minor={o.spend_minor} currency={o.currency} />
                </td>
                <td className="num">
                  <Money minor={o.potential_minor} currency={o.currency} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
