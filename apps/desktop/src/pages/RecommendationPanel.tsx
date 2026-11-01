import { useState } from "react";
import { useTranslation } from "react-i18next";
import { formatDateTime } from "../lib/format";
import type { MembershipView, RecommendationDetail } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { ErrorText, Loading, Money, StatusBadge } from "../components/ui";
import { ComparisonTable } from "./RfqPanel";
import { P } from "../lib/perm";

interface Props {
  active: MembershipView;
  selfId: string;
  recommendationId: string;
  onChanged: () => void;
}

/**
 * Operator: send or withdraw the recommendation. Customer approvers: approve, reject or
 * ask a question. The backend enforces the policy (count, self-approval, validity); the UI
 * only offers the buttons and shows the outcome.
 */
export function RecommendationPanel({ active, selfId, recommendationId, onChanged }: Props) {
  const { t, i18n } = useTranslation();
  const base = `/api/v1/orgs/${active.org_id}/recommendations/${recommendationId}`;
  const { data, error, reload } = useApi<RecommendationDetail>(base);
  const action = useAction();
  const [comment, setComment] = useState("");

  if (!data) return <Loading data={data} error={error} />;
  const rec = data.recommendation;
  const operator = active.access === "assigned";
  const approver = !operator && active.permissions.includes(P.approveRecommendation);
  const decided = data.decisions.some((d) => d.decided_by === selfId);

  async function post(path: string, body: unknown = {}) {
    if (await action.run("POST", `${base}${path}`, body)) {
      setComment("");
      reload();
      onChanged();
    }
  }

  return (
    <div className="card stack">
      <div className="row">
        <h2>{t("procurement:recommendation.heading")}</h2>
        <StatusBadge status={rec.status} />
      </div>
      <div className="grid-2">
        <div>
          <div className="muted">{t("procurement:quote.supplier")}</div>
          <div>{rec.supplier_name ?? ""}</div>
        </div>
        <div>
          <div className="muted">{t("procurement:comparison.total")}</div>
          <Money minor={rec.total_minor} currency={rec.currency} />
        </div>
        <div>
          <div className="muted">{t("procurement:comparison.rank")}</div>
          <div>{rec.rank}</div>
        </div>
        {rec.required_approvals !== null && rec.required_approvals !== undefined && (
          <div>
            <div className="muted">{t("procurement:approval.required")}</div>
            <div>
              {t("procurement:approval.progress", {
                required: rec.required_approvals,
                remaining: data.remaining_approvals ?? 0,
              })}
            </div>
          </div>
        )}
        {rec.reason && <p className="span-2 pre">{rec.reason}</p>}
      </div>

      <ComparisonTable results={data.comparison} currency={rec.currency} />

      {data.decisions.length > 0 && (
        <ul className="plain">
          {data.decisions.map((d) => (
            <li key={`${d.decided_by}-${d.created_at}`}>
              <StatusBadge status={d.decision} /> {d.decided_by_name ?? ""}{" "}
              <span className="muted small">
                {formatDateTime(d.created_at, i18n.language, active.timezone)}
              </span>
              {d.comment && <div className="pre">{d.comment}</div>}
            </li>
          ))}
        </ul>
      )}

      {operator && rec.status === "recommended" && (
        <div className="row">
          <button type="button" className="primary" disabled={action.busy} onClick={() => void post("/send")}>
            {t("procurement:recommendation.send")}
          </button>
          <label className="grow">
            {t("procurement:recommendation.withdrawReason")}
            <input maxLength={2000} value={comment} onChange={(e) => setComment(e.target.value)} />
          </label>
          <button
            type="button"
            disabled={!comment.trim() || action.busy}
            onClick={() => void post("/withdraw", { reason: comment })}
          >
            {t("procurement:recommendation.withdraw")}
          </button>
        </div>
      )}

      {approver && rec.status === "awaiting_approval" && !decided && (
        <div className="stack">
          {rec.requested_by === selfId && <p className="notice">{t("procurement:approval.selfNote")}</p>}
          <label>
            {t("procurement:approval.comment")}
            <textarea
              rows={2}
              maxLength={2000}
              value={comment}
              onChange={(e) => setComment(e.target.value)}
            />
          </label>
          <div className="row">
            <button
              type="button"
              className="primary"
              disabled={action.busy}
              onClick={() => void post("/decisions", { decision: "approve", comment })}
            >
              {t("procurement:approval.approve")}
            </button>
            <button
              type="button"
              disabled={action.busy || !comment.trim()}
              onClick={() => void post("/decisions", { decision: "reject", comment })}
            >
              {t("procurement:approval.reject")}
            </button>
            <button
              type="button"
              disabled={action.busy || !comment.trim()}
              onClick={() => void post("/decisions", { decision: "clarify", comment })}
            >
              {t("procurement:approval.clarify")}
            </button>
          </div>
          <p className="muted small">{t("procurement:approval.commentRequired")}</p>
        </div>
      )}
      <ErrorText code={action.error} />
    </div>
  );
}
