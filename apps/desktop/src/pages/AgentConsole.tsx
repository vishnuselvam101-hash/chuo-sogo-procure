import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import {
  AGENTS,
  RUN_STATUSES,
  SEVERITIES,
  SWEEP_AGENTS,
  type AgentRun,
  type AgentSettings,
  type AgentSummary,
  type ChaseSettings,
} from "../lib/agents";
import { formatDateTime, formatNumber } from "../lib/format";
import { P } from "../lib/perm";
import type { MembershipView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { EmptyState, ErrorText, Loading, PageHeader } from "../components/ui";

const countOf = (list: { key: string; count: number }[], key: string) =>
  list.find((c) => c.key === key)?.count ?? 0;

/** L8: what the agents did, what waits for people, and how they are set up. */
export function AgentConsole({ active, onOpenInbox }: { active: MembershipView; onOpenInbox?: () => void }) {
  const { t, i18n } = useTranslation();
  const base = `/api/v1/orgs/${active.org_id}/agents`;
  const summary = useApi<AgentSummary>(`${base}/summary`);
  const canManage = active.permissions.includes(P.manageAgents);
  const [agent, setAgent] = useState("");
  const [status, setStatus] = useState("");
  const query = new URLSearchParams({
    ...(agent ? { agent } : {}),
    ...(status ? { status } : {}),
    limit: "100",
  });
  const runs = useApi<AgentRun[]>(`${base}/runs?${query.toString()}`);
  const trigger = useAction();
  const [queued, setQueued] = useState<string | null>(null);
  const s = summary.data;

  async function runNow(name: string) {
    setQueued(null);
    if (await trigger.run("POST", `${base}/${name}/run`, {})) {
      setQueued(name);
      runs.reload();
      summary.reload();
    }
  }

  return (
    <section>
      <PageHeader title={t("nav.agents")}>
        <button
          type="button"
          onClick={() => {
            summary.reload();
            runs.reload();
          }}
        >
          {t("action.refresh")}
        </button>
      </PageHeader>
      <p className="lead">{t("agents:ui.console.lead")}</p>
      <Loading data={s} error={summary.error} />
      {s && (
        <>
          {!s.enabled && <p className="notice">{t("agents:ui.console.disabled")}</p>}
          <dl className="stats" data-testid="summary">
            {SEVERITIES.map((sev) => (
              <div key={sev}>
                <dt>
                  {t("agents:ui.console.openBySeverity", { severity: t(`agents:runtime.severity.${sev}`) })}
                </dt>
                <dd>{formatNumber(countOf(s.open_exceptions_by_severity, sev), i18n.language)}</dd>
              </div>
            ))}
            {RUN_STATUSES.map((st) => (
              <div key={st}>
                <dt>
                  {t("agents:ui.console.runsByStatus", { status: t(`agents:runtime.runStatus.${st}`) })}
                </dt>
                <dd>{formatNumber(countOf(s.runs_by_status, st), i18n.language)}</dd>
              </div>
            ))}
            <div>
              <dt>{t("agents:ui.console.modelCalls")}</dt>
              <dd>
                {t("agents:ui.console.ofBudget", {
                  used: formatNumber(s.model_calls_this_month, i18n.language),
                  budget: formatNumber(s.monthly_model_call_budget, i18n.language),
                })}
              </dd>
            </div>
          </dl>
          {onOpenInbox && (
            <div className="row">
              <button type="button" onClick={onOpenInbox}>
                {t("agents:ui.console.toInbox")}
              </button>
            </div>
          )}
        </>
      )}

      <h2>{t("agents:ui.console.runs")}</h2>
      {canManage && (
        <div className="row" data-testid="run-now">
          {SWEEP_AGENTS.map((a) => (
            <button key={a} type="button" disabled={trigger.busy} onClick={() => void runNow(a)}>
              {t("agents:ui.console.runNow", { agent: t(`agents:ui.agent.${a}`) })}
            </button>
          ))}
        </div>
      )}
      {queued && (
        <p role="status" className="small">
          {t("agents:ui.console.queued", { agent: t(`agents:ui.agent.${queued}`) })}
        </p>
      )}
      <ErrorText code={trigger.error} />
      <div className="row">
        <label>
          {t("agents:ui.filter.agent")}
          <select value={agent} onChange={(e) => setAgent(e.target.value)}>
            <option value="">{t("agents:ui.filter.all")}</option>
            {AGENTS.map((a) => (
              <option key={a} value={a}>
                {t(`agents:ui.agent.${a}`)}
              </option>
            ))}
          </select>
        </label>
        <label>
          {t("agents:ui.filter.status")}
          <select value={status} onChange={(e) => setStatus(e.target.value)}>
            <option value="">{t("agents:ui.filter.all")}</option>
            {RUN_STATUSES.map((st) => (
              <option key={st} value={st}>
                {t(`agents:runtime.runStatus.${st}`)}
              </option>
            ))}
          </select>
        </label>
      </div>
      <Loading data={runs.data} error={runs.error} />
      {runs.data && runs.data.length === 0 && <EmptyState hint={t("agents:ui.console.runsEmpty")} />}
      {runs.data && runs.data.length > 0 && (
        <div className="table-scroll">
          <table className="table" data-testid="runs">
            <thead>
              <tr>
                <th>{t("agents:ui.col.created")}</th>
                <th>{t("agents:ui.col.agent")}</th>
                <th>{t("agents:ui.col.trigger")}</th>
                <th>{t("agents:ui.col.status")}</th>
                <th className="num">{t("agents:ui.col.modelCalls")}</th>
                <th>{t("agents:ui.col.finished")}</th>
              </tr>
            </thead>
            <tbody>
              {runs.data.map((r) => (
                <tr key={r.id}>
                  <td className="small">{formatDateTime(r.created_at, i18n.language, active.timezone)}</td>
                  <td>{t(`agents:ui.agent.${r.agent}`, { defaultValue: r.agent })}</td>
                  <td>{t(`agents:ui.trigger.${r.trigger}`, { defaultValue: r.trigger })}</td>
                  <td>
                    <span className={`badge badge-run-${r.status}`}>
                      {t(`agents:runtime.runStatus.${r.status}`, { defaultValue: r.status })}
                    </span>
                    {r.error_code && (
                      <div className="muted small">
                        {t(`agents:ui.runError.${r.error_code}`, { defaultValue: r.error_code })}
                      </div>
                    )}
                  </td>
                  <td className="num">{r.model_calls}</td>
                  <td className="small">
                    {r.finished_at ? formatDateTime(r.finished_at, i18n.language, active.timezone) : "—"}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <SettingsPanel active={active} canManage={canManage} onSaved={summary.reload} />
      <ChaseCadencePanel active={active} canManage={canManage} />
    </section>
  );
}

function SettingsPanel({
  active,
  canManage,
  onSaved,
}: {
  active: MembershipView;
  canManage: boolean;
  onSaved: () => void;
}) {
  const { t, i18n } = useTranslation();
  const path = `/api/v1/orgs/${active.org_id}/agents/settings`;
  const { data, error, reload } = useApi<AgentSettings>(path);
  const action = useAction();
  const [form, setForm] = useState<{ enabled: boolean; auto: boolean; budget: string } | null>(null);
  const [saved, setSaved] = useState(false);
  if (!data)
    return (
      <fieldset className="card">
        <legend>{t("agents:ui.settings.title")}</legend>
        <Loading data={data} error={error} />
      </fieldset>
    );
  const f = form ?? {
    enabled: data.enabled,
    auto: data.auto_send_supplier_mail,
    budget: String(data.monthly_model_call_budget),
  };

  async function save(e: FormEvent) {
    e.preventDefault();
    const budget = /^\d{1,7}$/.test(f.budget.trim()) ? Number(f.budget.trim()) : null;
    if (budget === null) return action.setError("validation_failed/monthly_model_call_budget");
    const res = await action.run("PUT", path, {
      enabled: f.enabled,
      auto_send_supplier_mail: f.auto,
      monthly_model_call_budget: budget,
    });
    if (res) {
      setForm(null);
      setSaved(true);
      reload();
      onSaved();
    }
  }

  return (
    <form onSubmit={(e) => void save(e)}>
      <fieldset className="card stack" data-testid="agent-settings" disabled={!canManage}>
        <legend>{t("agents:ui.settings.title")}</legend>
        {!canManage && <p className="muted small">{t("agents:ui.settings.readOnly")}</p>}
        <label className="radio">
          <input
            type="checkbox"
            checked={f.enabled}
            onChange={(e) => {
              setSaved(false);
              setForm({ ...f, enabled: e.target.checked });
            }}
          />
          {t("agents:ui.settings.enabled")}
        </label>
        <p className="muted small">{t("agents:ui.settings.enabledHint")}</p>
        <label className="radio">
          <input
            type="checkbox"
            checked={f.auto}
            onChange={(e) => {
              setSaved(false);
              setForm({ ...f, auto: e.target.checked });
            }}
          />
          {t("agents:ui.settings.autoSend")}
        </label>
        <p className="notice small" role={f.auto ? "alert" : undefined}>
          {t("agents:ui.settings.autoSendWarning")}
        </p>
        {data.auto_send_supplier_mail && (
          <p className="small" data-testid="auto-send-state">
            {data.auto_send_confirmed
              ? t("agents:ui.settings.autoSendConfirmed")
              : t("agents:ui.settings.autoSendPending")}
          </p>
        )}
        <label>
          {t("agents:ui.settings.budget")}
          <input
            inputMode="numeric"
            value={f.budget}
            onChange={(e) => {
              setSaved(false);
              setForm({ ...f, budget: e.target.value });
            }}
          />
        </label>
        <p className="muted small">{t("agents:ui.settings.budgetHint")}</p>
        {data.updated_at && (
          <p className="muted small">
            {t("agents:ui.settings.updated", {
              at: formatDateTime(data.updated_at, i18n.language, active.timezone),
            })}
          </p>
        )}
        {canManage && (
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

const CADENCE = [
  "ack_days",
  "delivery_date_days",
  "overdue_days",
  "invoice_days",
  "repeat_days",
  "max_attempts",
] as const;
type Cadence = Record<(typeof CADENCE)[number], string>;

function ChaseCadencePanel({ active, canManage }: { active: MembershipView; canManage: boolean }) {
  const { t } = useTranslation();
  const path = `/api/v1/orgs/${active.org_id}/agents/chasing/settings`;
  const { data, error, reload } = useApi<ChaseSettings>(path);
  const action = useAction();
  const [form, setForm] = useState<Cadence | null>(null);
  if (!data) return error ? <ErrorText code={error} /> : null;
  const f: Cadence = form ?? (Object.fromEntries(CADENCE.map((k) => [k, String(data[k])])) as Cadence);

  async function save(e: FormEvent) {
    e.preventDefault();
    const body = Object.fromEntries(
      CADENCE.map((k) => [k, /^\d{1,3}$/.test(f[k].trim()) ? Number(f[k]) : NaN]),
    );
    if (Object.values(body).some((v) => Number.isNaN(v))) return action.setError("validation_failed/cadence");
    if (await action.run("PUT", path, body)) {
      setForm(null);
      reload();
    }
  }

  return (
    <form onSubmit={(e) => void save(e)}>
      <fieldset className="card stack" disabled={!canManage} data-testid="chase-settings">
        <legend>{t("agents:chasing.settings.title")}</legend>
        <div className="grid-2">
          {CADENCE.map((k) => (
            <label key={k}>
              {t(`agents:chasing.settings.${k}`)}
              <input
                inputMode="numeric"
                value={f[k]}
                onChange={(e) => setForm({ ...f, [k]: e.target.value })}
              />
            </label>
          ))}
        </div>
        {canManage && (
          <div className="row">
            <button type="submit" className="primary" disabled={action.busy || form === null}>
              {t("action.save")}
            </button>
          </div>
        )}
        <ErrorText code={action.error} />
      </fieldset>
    </form>
  );
}
