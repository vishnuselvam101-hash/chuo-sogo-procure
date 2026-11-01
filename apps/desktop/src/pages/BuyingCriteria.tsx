import { useState } from "react";
import { useTranslation } from "react-i18next";
import { bpsToPercent, formatDateTime, percentToBps } from "../lib/format";
import { P } from "../lib/perm";
import type { BuyingCriteria, BuyingCriteriaInput, MembershipView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { ErrorText, Loading } from "../components/ui";

export const PRIORITIES = ["price", "lead_time", "quality", "existing_supplier", "domestic"] as const;

interface Form {
  priorities: string[];
  preferred: string;
  excluded: string;
  certifications: string;
  maxLead: string;
  alert: string;
  notes: string;
}

const lines = (s: string) =>
  s
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter(Boolean);

function toForm(c: BuyingCriteria): Form {
  return {
    priorities: c.priorities,
    preferred: c.preferred_suppliers.join("\n"),
    excluded: c.excluded_suppliers.join("\n"),
    certifications: c.required_certifications.join("\n"),
    maxLead: c.max_lead_time_days == null ? "" : String(c.max_lead_time_days),
    alert: c.price_alert_bps == null ? "" : bpsToPercent(c.price_alert_bps),
    notes: c.notes,
  };
}

/** The form's values as the API input, or null when a number is not valid. */
export function toCriteriaInput(f: Form): BuyingCriteriaInput | null {
  const maxLead =
    f.maxLead.trim() === "" ? null : /^\d{1,4}$/.test(f.maxLead.trim()) ? Number(f.maxLead) : NaN;
  const alert = f.alert.trim() === "" ? null : percentToBps(f.alert);
  if (Number.isNaN(maxLead) || (alert === null && f.alert.trim() !== "")) return null;
  return {
    priorities: f.priorities.filter(Boolean),
    preferred_suppliers: lines(f.preferred),
    excluded_suppliers: lines(f.excluded),
    required_certifications: lines(f.certifications),
    max_lead_time_days: maxLead,
    price_alert_bps: alert,
    notes: f.notes,
  };
}

/** The customer's own buying rules, written once and shown to whoever sources for them. */
export function BuyingCriteriaCard({ active }: { active: MembershipView }) {
  const { t, i18n } = useTranslation();
  const path = `/api/v1/orgs/${active.org_id}/buying-criteria`;
  const canManage = active.permissions.includes(P.managePurchaseHistory);
  const { data, error, reload } = useApi<BuyingCriteria>(path);
  const action = useAction();
  // Unsaved edits; until the first change the form shows what the server holds.
  const [edited, setEdited] = useState<Form | null>(null);
  const [saved, setSaved] = useState(false);
  const form = edited ?? (data ? toForm(data) : null);

  async function save() {
    if (!form) return;
    setSaved(false);
    const input = toCriteriaInput(form);
    if (!input) {
      action.setError("validation_failed");
      return;
    }
    if (await action.run("PUT", path, input)) {
      setSaved(true);
      setEdited(null);
      reload();
    }
  }

  const set = (patch: Partial<Form>) => form && setEdited({ ...form, ...patch });

  return (
    <div className="card stack">
      <h2>{t("history:criteria.heading")}</h2>
      <p className="muted">{t("history:criteria.hint")}</p>
      <Loading data={data} error={error} />
      {form && (
        <fieldset disabled={!canManage || action.busy} className="stack">
          <div className="row">
            {[0, 1, 2].map((i) => (
              <label key={i}>
                {t("history:criteria.priority", { n: i + 1 })}
                <select
                  value={form.priorities[i] ?? ""}
                  onChange={(e) => {
                    const next = [...form.priorities];
                    next[i] = e.target.value;
                    set({ priorities: next.slice(0, 3) });
                  }}
                >
                  <option value="">—</option>
                  {PRIORITIES.map((p) => (
                    <option
                      key={p}
                      value={p}
                      disabled={form.priorities.includes(p) && form.priorities[i] !== p}
                    >
                      {t(`history:priority.${p}`)}
                    </option>
                  ))}
                </select>
              </label>
            ))}
          </div>
          <div className="row">
            <label className="grow">
              {t("history:criteria.preferred")}
              <textarea
                rows={3}
                value={form.preferred}
                onChange={(e) => set({ preferred: e.target.value })}
              />
            </label>
            <label className="grow">
              {t("history:criteria.excluded")}
              <textarea rows={3} value={form.excluded} onChange={(e) => set({ excluded: e.target.value })} />
            </label>
            <label className="grow">
              {t("history:criteria.certifications")}
              <textarea
                rows={3}
                value={form.certifications}
                onChange={(e) => set({ certifications: e.target.value })}
              />
            </label>
          </div>
          <div className="row">
            <label>
              {t("history:criteria.maxLead")}
              <input
                inputMode="numeric"
                value={form.maxLead}
                onChange={(e) => set({ maxLead: e.target.value })}
              />
            </label>
            <label>
              {t("history:criteria.alert")}
              <input
                inputMode="decimal"
                value={form.alert}
                onChange={(e) => set({ alert: e.target.value })}
              />
            </label>
          </div>
          <label>
            {t("history:criteria.notes")}
            <textarea
              rows={4}
              maxLength={4000}
              value={form.notes}
              onChange={(e) => set({ notes: e.target.value })}
            />
          </label>
          {canManage && (
            <div className="row">
              <button type="button" className="primary" onClick={() => void save()}>
                {t("action.save")}
              </button>
              {saved && (
                <span role="status" className="success">
                  {t("history:criteria.saved")}
                </span>
              )}
            </div>
          )}
          {data?.updated_at && (
            <p className="muted">
              {t("history:criteria.updated", {
                who: data.updated_by_name ?? "—",
                when: formatDateTime(data.updated_at, i18n.language, active.timezone),
              })}
            </p>
          )}
        </fieldset>
      )}
      <ErrorText code={action.error} />
    </div>
  );
}
