import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { MembershipView } from "../lib/types";
import type { components } from "../lib/api-schema";
import { useAction } from "../lib/useAction";
import { ErrorText } from "../components/ui";

type Suggestion = components["schemas"]["SuggestionView"];
interface ItemSuggestion {
  description: string;
  manufacturer: string;
  mpn: string;
  quantity: number;
  unit: string;
  confidence: number;
  flags: string[];
  needs_review: boolean;
}

const FAILED = "failed";

/** Flags the deterministic checks put on AI output, in the user's language. */
export function AiFlags({ flags }: { flags: string[] }) {
  const { t } = useTranslation();
  if (flags.length === 0) return null;
  return (
    <div className="small warn-text">
      {flags.map((f) => t(`procurement:aiFlag.${f}`, { defaultValue: f })).join(" · ")}
    </div>
  );
}

/**
 * Requester only, on an editable draft: paste an email or list, get item suggestions, tick
 * the ones to add. Nothing is added until the requester chooses.
 */
export function AiItems({
  active,
  requestId,
  onAdded,
}: {
  active: MembershipView;
  requestId: string;
  onAdded: () => void;
}) {
  const { t } = useTranslation();
  const org = `/api/v1/orgs/${active.org_id}`;
  const action = useAction();
  const [text, setText] = useState("");
  const [suggestion, setSuggestion] = useState<Suggestion | null>(null);
  const [chosen, setChosen] = useState<number[]>([]);
  const items = (suggestion?.output as unknown as ItemSuggestion[] | null | undefined) ?? [];

  async function extract() {
    const res = await action.run<Suggestion>("POST", `${org}/requests/${requestId}/ai/extract-items`, {
      text,
    });
    if (!res) return;
    setSuggestion(res.body);
    const out = (res.body.output as unknown as ItemSuggestion[] | null) ?? [];
    // Pre-tick only what passed every check with enough confidence.
    setChosen(out.flatMap((i, n) => (i.needs_review ? [] : [n])));
    if (res.body.status === FAILED) action.setError(`conflict/${res.body.error_code ?? "ai_unavailable"}`);
  }

  async function accept() {
    if (!suggestion) return;
    const res = await action.run(`POST`, `${org}/ai/suggestions/${suggestion.id}/accept-items`, {
      indices: chosen,
    });
    if (res) {
      setSuggestion(null);
      setText("");
      onAdded();
    }
  }

  async function dismiss() {
    if (!suggestion) return;
    if (await action.run("POST", `${org}/ai/suggestions/${suggestion.id}/reject`, { reason: "" }))
      setSuggestion(null);
  }

  return (
    <details className="card">
      <summary>{t("procurement:ai.itemsTitle")}</summary>
      <div className="stack">
        <p className="muted small">{t("procurement:ai.itemsHint")}</p>
        <label>
          {t("procurement:ai.sourceText")}
          <textarea rows={5} maxLength={20000} value={text} onChange={(e) => setText(e.target.value)} />
        </label>
        <div className="row">
          <button type="button" disabled={!text.trim() || action.busy} onClick={() => void extract()}>
            {action.busy ? t("procurement:ai.working") : t("procurement:ai.extract")}
          </button>
        </div>
        {suggestion && suggestion.status !== FAILED && (
          <div className="stack sub-card">
            <p className="notice">
              {t("procurement:ai.reviewNotice", { model: `${suggestion.provider}/${suggestion.model}` })}
            </p>
            {items.length === 0 && <p className="muted">{t("procurement:ai.nothingFound")}</p>}
            {items.map((item, n) => (
              <label key={n} className="radio">
                <input
                  type="checkbox"
                  checked={chosen.includes(n)}
                  onChange={(e) =>
                    setChosen(e.target.checked ? [...chosen, n] : chosen.filter((c) => c !== n))
                  }
                />
                <span>
                  {`${item.quantity} ${item.unit} · ${item.description}`}
                  {item.mpn && ` · ${item.manufacturer} ${item.mpn}`.trimEnd()}{" "}
                  <span className={item.needs_review ? "badge badge-warn" : "badge"}>
                    {t("procurement:ai.confidence", { value: item.confidence })}
                  </span>
                  <AiFlags flags={item.flags} />
                </span>
              </label>
            ))}
            <div className="row">
              <button
                type="button"
                className="primary"
                disabled={chosen.length === 0 || action.busy}
                onClick={() => void accept()}
              >
                {t("procurement:ai.addSelected", { count: chosen.length })}
              </button>
              <button type="button" disabled={action.busy} onClick={() => void dismiss()}>
                {t("procurement:ai.dismiss")}
              </button>
            </div>
          </div>
        )}
        <ErrorText code={action.error} />
      </div>
    </details>
  );
}
