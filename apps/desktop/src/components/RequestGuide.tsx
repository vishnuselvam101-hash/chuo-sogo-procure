import { useTranslation } from "react-i18next";

/**
 * "Where are we / what happens next": a plain-language reading of the request status
 * (crates/domain/src/workflow.rs). Display only; the server decides every transition.
 */
const STEPS = [
  { id: "ask", statuses: ["draft", "submitted", "needs_clarification"] },
  { id: "find", statuses: ["identifying", "sourcing"] },
  { id: "quote", statuses: ["rfq_sent", "quotes_received", "under_comparison", "recommended"] },
  { id: "approve", statuses: ["awaiting_approval", "approved", "rejected"] },
  { id: "deliver", statuses: ["ordered", "delivered"] },
  { id: "pay", statuses: ["invoiced", "disputed", "reconciled", "closed"] },
] as const;

const TROUBLE = ["needs_clarification", "rejected", "disputed"];
const DONE = "done";
const CURRENT = "current";
const TODO = "todo";

export function RequestGuide({ status }: { status: string }) {
  const { t } = useTranslation();
  const current = STEPS.findIndex((s) => (s.statuses as readonly string[]).includes(status));
  const known = current >= 0 || status === "cancelled";
  if (!known) return null;
  const finished = status === "closed";
  const tone = status === "cancelled" ? "stopped" : TROUBLE.includes(status) ? "attention" : "normal";
  return (
    <section
      className={`guide guide-${tone}`}
      aria-labelledby="request-guide-title"
      data-testid="request-guide"
    >
      <div className="guide-head">
        <h2 id="request-guide-title">{t("procurement:guide.title")}</h2>
        {current >= 0 && (
          <span className="guide-count">
            {t("procurement:guide.stepOf", { n: current + 1, total: STEPS.length })}
          </span>
        )}
      </div>
      <ol className="guide-steps">
        {STEPS.map((s, i) => {
          const state = finished || i < current ? DONE : i === current ? CURRENT : TODO;
          return (
            <li
              key={s.id}
              className={`guide-step ${state}`}
              aria-current={state === CURRENT ? "step" : undefined}
            >
              <span className="guide-dot" aria-hidden="true">
                {i + 1}
              </span>
              <span className="guide-label">{t(`procurement:guide.step.${s.id}`)}</span>
            </li>
          );
        })}
      </ol>
      <dl className="guide-text">
        <div>
          <dt>{t("procurement:guide.now")}</dt>
          <dd>{t(`procurement:guide.status.${status}.now`)}</dd>
        </div>
        <div>
          <dt>{t("procurement:guide.next")}</dt>
          <dd>{t(`procurement:guide.status.${status}.next`)}</dd>
        </div>
      </dl>
    </section>
  );
}
