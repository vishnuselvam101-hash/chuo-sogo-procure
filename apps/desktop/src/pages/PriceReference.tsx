import { useState } from "react";
import { useTranslation } from "react-i18next";
import { bpsToPercent, formatDate, formatMilliPrice, intlLocale } from "../lib/format";
import { P } from "../lib/perm";
import { PRIORITIES } from "./BuyingCriteria";
import type { BuyingCriteria, ItemReference, MembershipView, SimilarItem } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { ErrorText, Loading } from "../components/ui";

/**
 * Past items that look like a line with no match (proposals only). With the right to manage
 * the records, one can be confirmed as the same item; the reference then includes it.
 */
function SimilarItems({
  active,
  requestId,
  line,
  onLinked,
}: {
  active: MembershipView;
  requestId: string;
  line: ItemReference;
  onLinked: () => void;
}) {
  const { t, i18n } = useTranslation();
  const lang = i18n.language;
  const base = `/api/v1/orgs/${active.org_id}`;
  const canManage = active.permissions.includes(P.managePurchaseHistory);
  const similar = useApi<SimilarItem[]>(`${base}/requests/${requestId}/items/${line.item_id}/similar-items`);
  const action = useAction();

  async function link(candidate: SimilarItem) {
    if (!line.item_key) return;
    const done = await action.run("POST", `${base}/item-aliases`, {
      alias_key: line.item_key,
      canonical_key: candidate.item_key,
    });
    if (done) onLinked();
  }

  return (
    <div className="stack">
      <Loading data={similar.data} error={similar.error} />
      {similar.data?.length === 0 && <p className="muted">{t("history:similar.none")}</p>}
      {similar.data && similar.data.length > 0 && (
        <ul className="plain">
          {similar.data.map((c) => (
            <li key={c.item_key} className="row">
              <span className="grow">
                {c.description || "—"}
                {c.mpn && <span className="muted"> · {c.mpn}</span>}
                <span className="muted">
                  {" "}
                  · {t("history:similar.purchases", { count: c.purchases })} ·{" "}
                  {formatMilliPrice(c.last_unit_price_milli, c.currency, lang)} (
                  {formatDate(c.last_purchased_on, lang)})
                </span>
              </span>
              {canManage && (
                <button type="button" disabled={action.busy} onClick={() => void link(c)}>
                  {t("history:similar.link")}
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      <ErrorText code={action.error} />
    </div>
  );
}

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
  const [open, setOpen] = useState<string | null>(null);
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
      {refs.data && refs.data.length > 0 && (
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
                      <td colSpan={4}>
                        <span className="muted">{t("history:reference.noMatch")}</span>{" "}
                        {r.item_key && open !== r.item_id && (
                          <button type="button" className="link" onClick={() => setOpen(r.item_id)}>
                            {t("history:similar.find")}
                          </button>
                        )}
                        {open === r.item_id && (
                          <SimilarItems
                            active={active}
                            requestId={requestId}
                            line={r}
                            onLinked={() => {
                              setOpen(null);
                              refs.reload();
                            }}
                          />
                        )}
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
