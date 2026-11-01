import { useState } from "react";
import { useTranslation } from "react-i18next";
import { minorToInput, parseMinor } from "../lib/format";
import type { MembershipView, PolicyView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { ErrorText, Loading, Money } from "../components/ui";
import { CURRENCIES } from "./NewRequest";
import { Requests } from "./Requests";
import { AWAITING_APPROVAL, P } from "../lib/perm";

/** Pending approvals for approvers; the approval policy editor for organisation admins. */
export function Approvals({ active, selfId }: { active: MembershipView; selfId: string }) {
  const { t } = useTranslation();
  const has = (p: string) => active.permissions.includes(p);
  return (
    <>
      {has(P.approveRecommendation) && (
        <Requests
          active={active}
          selfId={selfId}
          status={AWAITING_APPROVAL}
          title={t("procurement:approval.pending")}
        />
      )}
      {has(P.manageApprovalPolicy) && <PolicyEditor active={active} />}
    </>
  );
}

interface TierDraft {
  min: string;
  approvals: number;
}

function PolicyEditor({ active }: { active: MembershipView }) {
  const { t } = useTranslation();
  const base = `/api/v1/orgs/${active.org_id}/approval-policies`;
  const { data, error, reload } = useApi<PolicyView[]>(base);
  const action = useAction();
  const [currency, setCurrency] = useState("JPY");
  const [draft, setDraft] = useState<{ currency: string; tiers: TierDraft[]; self: boolean } | null>(null);

  if (!data) return <Loading data={data} error={error} />;
  const current = data.find((p) => p.currency === currency);
  const editing =
    draft?.currency === currency
      ? draft
      : {
          currency,
          tiers: (current?.tiers ?? []).map((tier) => ({
            min: minorToInput(tier.min_minor, currency),
            approvals: tier.approvals,
          })),
          self: current?.allow_self_approval ?? false,
        };
  const update = (patch: Partial<typeof editing>) => setDraft({ ...editing, ...patch });

  async function save() {
    const tiers = editing.tiers.map((tier) => ({
      min_minor: parseMinor(tier.min, currency),
      approvals: tier.approvals,
    }));
    if (tiers.some((tier) => tier.min_minor === null)) {
      action.setError("validation_failed/amount");
      return;
    }
    const res = await action.run("PUT", `${base}/${currency}`, { tiers, allow_self_approval: editing.self });
    if (res) {
      setDraft(null);
      reload();
    }
  }

  return (
    <section className="card stack">
      <h2>{t("procurement:policy.heading")}</h2>
      <p className="muted small">{t("procurement:policy.explain")}</p>
      <label className="compact">
        {t("procurement:request.currency")}
        <select value={currency} onChange={(e) => setCurrency(e.target.value)}>
          {CURRENCIES.map((c) => (
            <option key={c} value={c}>
              {c}
            </option>
          ))}
        </select>
      </label>
      <p className="muted small">
        {current
          ? t("procurement:policy.version", { version: current.version })
          : t("procurement:policy.default")}
      </p>
      <table className="table">
        <thead>
          <tr>
            <th>{t("procurement:policy.from")}</th>
            <th>{t("procurement:policy.approvals")}</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {editing.tiers.map((tier, i) => (
            <tr key={i}>
              <td>
                <input
                  inputMode="decimal"
                  aria-label={t("procurement:policy.from")}
                  value={tier.min}
                  onChange={(e) =>
                    update({
                      tiers: editing.tiers.map((x, j) => (j === i ? { ...x, min: e.target.value } : x)),
                    })
                  }
                />{" "}
                <Money minor={parseMinor(tier.min, currency)} currency={currency} />
              </td>
              <td>
                <select
                  aria-label={t("procurement:policy.approvals")}
                  value={tier.approvals}
                  onChange={(e) =>
                    update({
                      tiers: editing.tiers.map((x, j) =>
                        j === i ? { ...x, approvals: Number(e.target.value) } : x,
                      ),
                    })
                  }
                >
                  {[1, 2, 3, 4, 5].map((n) => (
                    <option key={n} value={n}>
                      {n}
                    </option>
                  ))}
                </select>
              </td>
              <td>
                <button
                  type="button"
                  onClick={() => update({ tiers: editing.tiers.filter((_, j) => j !== i) })}
                >
                  {t("procurement:item.remove")}
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <label className="radio">
        <input type="checkbox" checked={editing.self} onChange={(e) => update({ self: e.target.checked })} />
        {t("procurement:policy.allowSelf")}
      </label>
      <div className="row">
        <button
          type="button"
          onClick={() => update({ tiers: [...editing.tiers, { min: "0", approvals: 2 }] })}
        >
          {t("procurement:policy.addTier")}
        </button>
        <button type="button" className="primary" disabled={action.busy} onClick={() => void save()}>
          {t("action.save")}
        </button>
      </div>
      <ErrorText code={action.error} />
    </section>
  );
}
