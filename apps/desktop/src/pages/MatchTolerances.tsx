import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import type { MatchTolerancesView } from "../lib/agents";
import { bpsToPercent, minorToInput, parseMinor, percentToBps } from "../lib/format";
import { P } from "../lib/perm";
import type { MembershipView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { ErrorText, Loading } from "../components/ui";

const CURRENCIES = ["JPY", "USD", "EUR", "INR", "CNY", "KRW"] as const;

interface Form {
  currency: string;
  price: string;
  priceAbs: string;
  qty: string;
  qtyAbs: string;
  rounding: string;
}

/** L2: how far an invoice may differ before a person has to look (organisation setting). */
export function MatchTolerances({ active }: { active: MembershipView }) {
  const { t } = useTranslation();
  const path = `/api/v1/orgs/${active.org_id}/matching/tolerances`;
  const { data, error, reload } = useApi<MatchTolerancesView>(path);
  const action = useAction();
  const canEdit = active.permissions.includes(P.manageInvoices);
  const [form, setForm] = useState<Form | null>(null);
  const [saved, setSaved] = useState(false);
  if (!data)
    return (
      <fieldset className="card">
        <legend>{t("agents:matching.tolerances.title")}</legend>
        <Loading data={data} error={error} />
      </fieldset>
    );
  const f: Form = form ?? {
    currency: data.currency,
    price: bpsToPercent(data.price_bps),
    priceAbs: minorToInput(data.price_line_abs_minor, data.currency),
    qty: bpsToPercent(data.quantity_bps),
    qtyAbs: String(data.quantity_abs),
    rounding: minorToInput(data.rounding_minor, data.currency),
  };
  const set = (patch: Partial<Form>) => {
    setSaved(false);
    setForm({ ...f, ...patch });
  };

  async function save(e: FormEvent) {
    e.preventDefault();
    const body = {
      currency: f.currency,
      price_bps: percentToBps(f.price),
      price_line_abs_minor: parseMinor(f.priceAbs, f.currency),
      quantity_bps: percentToBps(f.qty),
      quantity_abs: /^\d{1,9}$/.test(f.qtyAbs.trim()) ? Number(f.qtyAbs.trim()) : null,
      rounding_minor: parseMinor(f.rounding, f.currency),
    };
    if (Object.values(body).some((v) => v === null)) return action.setError("validation_failed/tolerances");
    if (await action.run("PUT", path, body)) {
      setForm(null);
      setSaved(true);
      reload();
    }
  }

  return (
    <form onSubmit={(e) => void save(e)}>
      <fieldset className="card stack" disabled={!canEdit} data-testid="tolerances">
        <legend>{t("agents:matching.tolerances.title")}</legend>
        <p className="muted small">{t("agents:matching.tolerances.exactHint")}</p>
        <div className="grid-2">
          <label>
            {t("agents:matching.tolerances.currency")}
            <select value={f.currency} onChange={(e) => set({ currency: e.target.value })}>
              {CURRENCIES.map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </select>
          </label>
          <label>
            {t("agents:ui.tolerances.percent", { label: t("agents:matching.tolerances.priceBps") })}
            <input inputMode="decimal" value={f.price} onChange={(e) => set({ price: e.target.value })} />
          </label>
          <label>
            {t("agents:matching.tolerances.priceLineAbs")}
            <input
              inputMode="decimal"
              value={f.priceAbs}
              onChange={(e) => set({ priceAbs: e.target.value })}
            />
          </label>
          <label>
            {t("agents:ui.tolerances.percent", { label: t("agents:matching.tolerances.quantityBps") })}
            <input inputMode="decimal" value={f.qty} onChange={(e) => set({ qty: e.target.value })} />
          </label>
          <label>
            {t("agents:matching.tolerances.quantityAbs")}
            <input inputMode="numeric" value={f.qtyAbs} onChange={(e) => set({ qtyAbs: e.target.value })} />
          </label>
          <label>
            {t("agents:matching.tolerances.rounding")}
            <input
              inputMode="decimal"
              value={f.rounding}
              onChange={(e) => set({ rounding: e.target.value })}
            />
          </label>
        </div>
        {canEdit && (
          <div className="row">
            <button type="submit" className="primary" disabled={action.busy || form === null}>
              {t("action.save")}
            </button>
            {saved && <span role="status">{t("agents:ui.settings.saved")}</span>}
          </div>
        )}
        <ErrorText code={action.error} />
      </fieldset>
    </form>
  );
}
