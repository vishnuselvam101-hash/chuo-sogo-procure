import { useTranslation } from "react-i18next";
import { bpsToPercent, formatDate, formatMilliPrice, intlLocale } from "../lib/format";
import { PRIORITIES } from "./BuyingCriteria";
import type { BuyingCriteria, ItemReference, MembershipView } from "../lib/types";
import { useApi } from "../lib/useApi";
import { Loading } from "../components/ui";

/**
 * Decision material next to a request: what this customer paid before for each line (from
 * their imported purchase history) and their buying criteria. Reference only; nothing here
 * chooses a supplier.
 */
export function PriceReference({ active, requestId }: { active: MembershipView; requestId: string }) {
  const { t, i18n } = useTranslation();
  const lang = i18n.language;
  const list = new Intl.ListFormat(intlLocale(lang), { style: "narrow", type: "unit" });
  const base = `/api/v1/orgs/${active.org_id}`;
  const refs = useApi<ItemReference[]>(`${base}/requests/${requestId}/price-reference`);
  const criteria = useApi<BuyingCriteria>(`${base}/buying-criteria`);
  const c = criteria.data;
  const hasCriteria =
    c &&
    (c.priorities.length > 0 ||
      c.preferred_suppliers.length > 0 ||
      c.excluded_suppliers.length > 0 ||
      c.required_certifications.length > 0 ||
      c.max_lead_time_days != null ||
      c.price_alert_bps != null ||
      c.notes);

  return (
    <div className="card stack">
      <h2>{t("history:reference.heading")}</h2>
      <Loading data={refs.data} error={refs.error} />
      {refs.data && refs.data.every((r) => !r.summary) && (
        <p className="muted">{t("history:reference.none")}</p>
      )}
      {refs.data?.some((r) => r.summary) && (
        <div className="table-scroll">
          <table className="table">
            <thead>
              <tr>
                <th className="num">{t("procurement:item.line")}</th>
                <th>{t("history:reference.last")}</th>
                <th className="num">{t("history:reference.range")}</th>
                <th>{t("history:reference.cheapest")}</th>
                <th className="num">{t("history:reference.count")}</th>
              </tr>
            </thead>
            <tbody>
              {refs.data.map((r) => {
                const s = r.summary;
                if (!s) {
                  return (
                    <tr key={r.item_id}>
                      <td className="num">{r.line_no}</td>
                      <td colSpan={4} className="muted">
                        {t("history:reference.noMatch")}
                      </td>
                    </tr>
                  );
                }
                const cur = s.last.currency;
                return (
                  <tr key={r.item_id}>
                    <td className="num">{r.line_no}</td>
                    <td>
                      <span className="num">{formatMilliPrice(s.last.unit_price_milli, cur, lang)}</span>{" "}
                      <span className="muted">
                        {s.last.supplier_name || "—"} · {formatDate(s.last.purchased_on, lang)}
                      </span>
                    </td>
                    <td className="num">
                      {formatMilliPrice(s.min_price_milli, cur, lang)} –{" "}
                      {formatMilliPrice(s.max_price_milli, cur, lang)}
                    </td>
                    <td>
                      {s.cheapest.supplier_name || "—"}{" "}
                      <span className="muted">{formatDate(s.cheapest.purchased_on, lang)}</span>
                    </td>
                    <td className="num">
                      {s.purchases}
                      <span className="muted">
                        {" "}
                        ({t(`history:reference.on_${r.matched_on ?? "description"}`)})
                      </span>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
      {hasCriteria && c && (
        <dl className="facts">
          {c.priorities.length > 0 && (
            <>
              <dt>{t("history:criteria.priorities")}</dt>
              <dd>
                {c.priorities
                  .filter((p): p is (typeof PRIORITIES)[number] =>
                    (PRIORITIES as readonly string[]).includes(p),
                  )
                  .map((p) => t(`history:priority.${p}`))
                  .join(" › ")}
              </dd>
            </>
          )}
          {c.preferred_suppliers.length > 0 && (
            <>
              <dt>{t("history:criteria.preferred")}</dt>
              <dd>{list.format(c.preferred_suppliers)}</dd>
            </>
          )}
          {c.excluded_suppliers.length > 0 && (
            <>
              <dt>{t("history:criteria.excluded")}</dt>
              <dd>{list.format(c.excluded_suppliers)}</dd>
            </>
          )}
          {c.required_certifications.length > 0 && (
            <>
              <dt>{t("history:criteria.certifications")}</dt>
              <dd>{list.format(c.required_certifications)}</dd>
            </>
          )}
          {c.max_lead_time_days != null && (
            <>
              <dt>{t("history:criteria.maxLead")}</dt>
              <dd>{c.max_lead_time_days}</dd>
            </>
          )}
          {c.price_alert_bps != null && (
            <>
              <dt>{t("history:criteria.alert")}</dt>
              <dd>{bpsToPercent(c.price_alert_bps)} %</dd>
            </>
          )}
          {c.notes && (
            <>
              <dt>{t("history:criteria.notes")}</dt>
              <dd className="pre">{c.notes}</dd>
            </>
          )}
        </dl>
      )}
    </div>
  );
}
