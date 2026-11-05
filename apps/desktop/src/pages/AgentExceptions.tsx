import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ACTION_LABEL,
  AGENTS,
  EDITABLE,
  RESOLUTIONS,
  SEVERITIES,
  actionType,
  canEdit,
  exceptionBody,
  exceptionTitle,
  type Action,
  type AgentException,
  type QuotationFacts,
  type SuggestionView,
} from "../lib/agents";
import { formatDate, formatDateTime } from "../lib/format";
import { P } from "../lib/perm";
import type { MembershipView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { EmptyState, ErrorText, Loading, Money, PageHeader } from "../components/ui";
import { FactsTable } from "./CostWork";

const OPEN = "open";
const ALL = "all";
const ACCEPTED = "accepted";
const UNKNOWN = "unknown";
const MONEY_FIELDS = ["expected", "actual", "difference_minor"] as const;

export function SeverityBadge({ severity }: { severity: string }) {
  const { t } = useTranslation();
  return (
    <span className={`badge badge-sev-${severity}`}>
      {t(`agents:runtime.severity.${severity}`, { defaultValue: severity })}
    </span>
  );
}

export function ExceptionStatusBadge({ status }: { status: string }) {
  const { t } = useTranslation();
  return (
    <span className={`badge badge-ex-${status}`}>
      {t(`agents:runtime.exceptionStatus.${status}`, { defaultValue: status })}
    </span>
  );
}

/** L4: the exception inbox. Every consequential step waits here for a person. */
export function AgentExceptions({ active }: { active: MembershipView }) {
  const { t, i18n } = useTranslation();
  const [agent, setAgent] = useState("");
  const [severity, setSeverity] = useState("");
  const [status, setStatus] = useState(OPEN);
  const [openId, setOpenId] = useState<string | null>(null);
  const base = `/api/v1/orgs/${active.org_id}/agents/exceptions`;
  const query = new URLSearchParams({ status, ...(agent ? { agent } : {}) }).toString();
  const { data, error, reload } = useApi<AgentException[]>(`${base}?${query}`);
  const canDecide = active.permissions.includes(P.decideAgentExceptions);
  const rows = (data ?? []).filter((e) => !severity || e.severity === severity);
  const opened = rows.find((e) => e.id === openId) ?? null;

  return (
    <section>
      <PageHeader title={t("nav.exceptions")}>
        <button type="button" onClick={reload}>
          {t("action.refresh")}
        </button>
      </PageHeader>
      <p className="lead">{t("agents:ui.inbox.lead")}</p>
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
          {t("agents:ui.filter.severity")}
          <select value={severity} onChange={(e) => setSeverity(e.target.value)}>
            <option value="">{t("agents:ui.filter.all")}</option>
            {SEVERITIES.map((s) => (
              <option key={s} value={s}>
                {t(`agents:runtime.severity.${s}`)}
              </option>
            ))}
          </select>
        </label>
        <label>
          {t("agents:ui.filter.status")}
          <select value={status} onChange={(e) => setStatus(e.target.value)}>
            <option value={OPEN}>{t("agents:runtime.exceptionStatus.open")}</option>
            <option value={ALL}>{t("agents:ui.filter.all")}</option>
          </select>
        </label>
      </div>
      {!canDecide && <p className="muted small">{t("agents:ui.inbox.viewOnly")}</p>}
      <Loading data={data} error={error} />
      {data && rows.length === 0 && <EmptyState hint={t("agents:ui.inbox.empty")} />}
      {rows.length > 0 && (
        <table className="table" data-testid="exceptions">
          <thead>
            <tr>
              <th>{t("agents:ui.col.severity")}</th>
              <th>{t("agents:ui.col.title")}</th>
              <th>{t("agents:ui.col.agent")}</th>
              <th>{t("agents:ui.col.created")}</th>
              <th>{t("agents:ui.col.status")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {rows.map((ex) => (
              <tr key={ex.id}>
                <td>
                  <SeverityBadge severity={ex.severity} />
                </td>
                <td>{exceptionTitle(ex, t, i18n.language)}</td>
                <td>{t(`agents:ui.agent.${ex.agent}`, { defaultValue: ex.agent })}</td>
                <td className="small">{formatDateTime(ex.created_at, i18n.language, active.timezone)}</td>
                <td>
                  <ExceptionStatusBadge status={ex.status} />
                </td>
                <td className="row-actions">
                  <button type="button" onClick={() => setOpenId(openId === ex.id ? null : ex.id)}>
                    {openId === ex.id ? t("agents:ui.inbox.close") : t("agents:ui.inbox.open")}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {opened && (
        <ExceptionDetail
          key={opened.id}
          ex={opened}
          orgId={active.org_id}
          timezone={active.timezone}
          canDecide={canDecide}
          onDecided={() => {
            setOpenId(null);
            reload();
          }}
          onClose={() => setOpenId(null)}
        />
      )}
    </section>
  );
}

/**
 * One exception: what the agent found, what it proposes, and Approve / Edit / Reject.
 * `orgId` null = staff console (read only; no organisation session to fetch details with).
 */
export function ExceptionDetail({
  ex,
  orgId,
  timezone,
  canDecide,
  onDecided,
  onClose,
}: {
  ex: AgentException;
  orgId: string | null;
  timezone: string;
  canDecide: boolean;
  onDecided?: () => void;
  onClose: () => void;
}) {
  const { t, i18n } = useTranslation();
  const proposed = (ex.proposed_action ?? null) as Action | null;
  const type = actionType(proposed);
  const [draft, setDraft] = useState<Action | null>(null);
  const [note, setNote] = useState("");
  const decide = useAction();
  const editable = canDecide && ex.status === OPEN && canEdit(proposed);
  const decidable = canDecide && ex.status === OPEN && orgId !== null;
  const body = exceptionBody(ex, t, (k) => i18n.exists(k), i18n.language);

  async function send(decision: "approve" | "edit" | "reject") {
    if (!orgId) return;
    if (decision === "reject" && !note.trim()) return decide.setError("validation_failed/note");
    const res = await decide.run("POST", `/api/v1/orgs/${orgId}/agents/exceptions/${ex.id}/decide`, {
      decision,
      action: decision === "edit" ? draft : null,
      note: note.trim() || null,
      // The version shown here: if an agent refreshed the proposal meanwhile, the server
      // refuses with exception_changed instead of acting on something not seen.
      expected_updated_at: ex.updated_at,
    });
    if (res) onDecided?.();
  }

  return (
    <aside
      className="card stack drawer"
      aria-label={exceptionTitle(ex, t, i18n.language)}
      data-testid="detail"
    >
      <div className="row">
        <SeverityBadge severity={ex.severity} />
        <h2 className="grow">{exceptionTitle(ex, t, i18n.language)}</h2>
        <button type="button" onClick={onClose}>
          {t("agents:ui.inbox.close")}
        </button>
      </div>
      <p className="muted small">
        {t(`agents:ui.agent.${ex.agent}`, { defaultValue: ex.agent })} ·{" "}
        {formatDateTime(ex.created_at, i18n.language, timezone)} · <ExceptionStatusBadge status={ex.status} />
      </p>
      {body && <p>{body}</p>}
      <ExceptionFacts ex={ex} orgId={orgId} />
      {ex.suggestion_id && orgId && <SuggestionText orgId={orgId} id={ex.suggestion_id} />}
      <div className="sub-card stack">
        <h3>{t("agents:ui.detail.proposed")}</h3>
        {!type && <p className="muted">{t("agents:ui.detail.noAction")}</p>}
        {type && (
          <ProposedAction
            action={draft ?? proposed ?? {}}
            editing={draft !== null}
            params={ex.params as Record<string, unknown>}
            onChange={setDraft}
          />
        )}
        {ex.status !== OPEN && ex.decision_note && (
          <p className="small">
            {t("agents:ui.detail.decisionNote")}: {ex.decision_note}
          </p>
        )}
      </div>
      {decidable && (
        <div className="stack">
          <label>
            {t("agents:ui.detail.note")}
            <textarea maxLength={2000} rows={2} value={note} onChange={(e) => setNote(e.target.value)} />
          </label>
          <div className="row">
            {draft === null ? (
              <button
                type="button"
                className="primary"
                disabled={decide.busy}
                onClick={() => void send("approve")}
              >
                {type ? t("agents:ui.detail.approve") : t("agents:ui.detail.approveNoAction")}
              </button>
            ) : (
              <button
                type="button"
                className="primary"
                disabled={decide.busy}
                onClick={() => void send("edit")}
              >
                {t("agents:ui.detail.approveEdited")}
              </button>
            )}
            {editable && draft === null && (
              <button type="button" onClick={() => setDraft(structuredClone(proposed ?? {}))}>
                {t("agents:ui.detail.edit")}
              </button>
            )}
            {draft !== null && (
              <button type="button" onClick={() => setDraft(null)}>
                {t("agents:ui.detail.discardEdit")}
              </button>
            )}
            <button
              type="button"
              className="danger"
              disabled={decide.busy}
              onClick={() => void send("reject")}
            >
              {t("agents:ui.detail.reject")}
            </button>
          </div>
          <p className="muted small">{t("agents:ui.detail.rejectHint")}</p>
          <ErrorText code={decide.error} />
        </div>
      )}
      {canDecide === false && ex.status === OPEN && (
        <p className="muted small">{t("agents:ui.inbox.viewOnly")}</p>
      )}
    </aside>
  );
}

const asText = (v: unknown) => (typeof v === "string" ? v : "");
const asList = (v: unknown) => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);

/** The proposal in words, with inputs for the fields a person may change. */
function ProposedAction({
  action,
  editing,
  params,
  onChange,
}: {
  action: Action;
  editing: boolean;
  params: Record<string, unknown>;
  onChange: (a: Action) => void;
}) {
  const { t, i18n } = useTranslation();
  const type = actionType(action) ?? "";
  const fields = editing ? (EDITABLE[type] ?? []) : [];
  const set = (patch: Record<string, unknown>) => onChange({ ...action, ...patch });
  const draft = (action.draft ?? {}) as Record<string, unknown>;
  const date = asText(action.date);

  return (
    <div className="stack" data-testid="proposed">
      <p>
        <strong>{t(`agents:${ACTION_LABEL[type] ?? type}`, { defaultValue: type })}</strong>
      </p>
      {type === "resolve_discrepancy" &&
        (fields.includes("resolution") ? (
          <label>
            {t("agents:ui.field.resolution")}
            <select
              value={asText(action.resolution) || ACCEPTED}
              onChange={(e) => set({ resolution: e.target.value })}
            >
              {RESOLUTIONS.map((r) => (
                <option key={r} value={r}>
                  {t(`procurement:resolution.${r}`)}
                </option>
              ))}
            </select>
          </label>
        ) : (
          <p>
            {t("agents:ui.field.resolution")}:{" "}
            {t(`procurement:resolution.${asText(action.resolution) || ACCEPTED}`)}
          </p>
        ))}
      {type === "request_credit_note" && <p className="small">{t("agents:ui.hint.creditNote")}</p>}
      {type === "send_chase" && (
        <>
          <p className="small">
            {t(`agents:chasing.chaseType.${asText(action.chase_type)}`, {
              defaultValue: asText(action.chase_type),
            })}{" "}
            · {t("agents:ui.field.attempt", { n: Number(action.attempt ?? 1) })}
          </p>
          <p className="notice small">{t("agents:ui.hint.mailTestMode")}</p>
          {fields.includes("subject") ? (
            <>
              <label>
                {t("agents:ui.field.subject")}
                <input
                  maxLength={200}
                  value={asText(draft.subject)}
                  onChange={(e) => set({ draft: { ...draft, subject: e.target.value } })}
                />
              </label>
              <label>
                {t("agents:ui.field.body")}
                <textarea
                  rows={10}
                  maxLength={10000}
                  value={asText(draft.body)}
                  onChange={(e) => set({ draft: { ...draft, body: e.target.value } })}
                />
              </label>
            </>
          ) : (
            <div className="sub-card" data-testid="email-preview">
              <p className="small">
                <strong>{t("agents:ui.field.subject")}:</strong> {asText(draft.subject)}
              </p>
              <pre className="pre">{asText(draft.body)}</pre>
            </div>
          )}
        </>
      )}
      {(type === "record_promised_date" || type === "confirm_delivery_date") && (
        <>
          {fields.includes("date") ? (
            <label>
              {t("agents:ui.field.date")}
              <input type="date" value={date} onChange={(e) => set({ date: e.target.value || null })} />
            </label>
          ) : (
            <p>
              {t("agents:ui.field.date")}:{" "}
              {date ? formatDate(date, i18n.language) : t("agents:ui.field.noDate")}
            </p>
          )}
          {type === "confirm_delivery_date" &&
            (fields.includes("note") ? (
              <label>
                {t("agents:ui.field.note")}
                <input
                  maxLength={2000}
                  value={asText(action.note)}
                  onChange={(e) => set({ note: e.target.value })}
                />
              </label>
            ) : (
              asText(action.note) && (
                <p className="small">
                  {t("agents:ui.field.note")}: {asText(action.note)}
                </p>
              )
            ))}
        </>
      )}
      {type === "save_brief" && <p className="small">{t("agents:ui.hint.saveBrief")}</p>}
      {type === "open_negotiation_task" && (
        <>
          <p className="small">{t("agents:ui.hint.negotiationTask")}</p>
          {fields.includes("title") && (
            <label>
              {t("agents:ui.field.taskTitle")}
              <input
                maxLength={200}
                value={asText(action.title)}
                onChange={(e) => set({ title: e.target.value })}
              />
            </label>
          )}
        </>
      )}
      {type === "request_document_update" && (
        <>
          <p className="small">{t("agents:ui.hint.documentUpdate")}</p>
          <p className="notice small">{t("agents:ui.hint.mailTestMode")}</p>
          {fields.includes("note") ? (
            <label>
              {t("agents:ui.field.note")}
              <input
                maxLength={1000}
                value={asText(action.note)}
                onChange={(e) => set({ note: e.target.value })}
              />
            </label>
          ) : (
            asText(action.note) && <p className="small">{asText(action.note)}</p>
          )}
        </>
      )}
      {type === "flag_supplier_review" && (
        <>
          <p className="small">{t("agents:ui.hint.supplierReview", { month: asText(action.month) })}</p>
          <ul>
            {asList(action.reasons).map((r) => (
              <li key={r}>{t(`agents:supplier.reason.${r}`, { defaultValue: r })}</li>
            ))}
          </ul>
        </>
      )}
      {type === "apply_import" && (
        <p className="small">{t("agents:ui.hint.applyImport", { records: Number(params.records ?? 0) })}</p>
      )}
    </div>
  );
}

/** What the agent found, per agent, from the exception's own values. */
function ExceptionFacts({ ex, orgId }: { ex: AgentException; orgId: string | null }) {
  const { t, i18n } = useTranslation();
  const p = ex.params as Record<string, unknown>;
  const currency = asText(p.currency);
  if (ex.agent === "matching")
    return (
      <dl className="stats">
        {typeof p.discrepancy_kind === "string" && (
          <div>
            <dt>{t("agents:ui.field.discrepancy")}</dt>
            <dd>{t(`agents:matching.kind.${p.discrepancy_kind}`, { defaultValue: p.discrepancy_kind })}</dd>
          </div>
        )}
        {typeof p.item === "string" && (
          <div>
            <dt>{t("agents:matching.field.item")}</dt>
            <dd>{p.item}</dd>
          </div>
        )}
        {MONEY_FIELDS.map((k) =>
          typeof p[k] === "number" && currency ? (
            <div key={k}>
              <dt>
                {t(
                  k === "expected"
                    ? "agents:matching.field.expected"
                    : k === "actual"
                      ? "agents:matching.field.actual"
                      : "agents:matching.field.difference",
                )}
              </dt>
              <dd>
                <Money minor={p[k] as number} currency={currency} />
              </dd>
            </div>
          ) : null,
        )}
        {typeof p.invoiced_quantity === "number" && (
          <div>
            <dt>{t("agents:matching.field.invoicedQuantity")}</dt>
            <dd>{p.invoiced_quantity}</dd>
          </div>
        )}
      </dl>
    );
  if (ex.agent === "chasing" && p.acknowledged !== undefined)
    return (
      <div className="stack small">
        <p>
          {t("agents:ui.field.acknowledged")}:{" "}
          {t(`agents:chasing.ack.${asText(p.acknowledged)}`, { defaultValue: "" })}
          {asText(p.promised_date) &&
            ` · ${t("agents:ui.field.promisedDate")}: ${formatDate(asText(p.promised_date), i18n.language)}`}
          {typeof p.partial_quantity === "number" &&
            ` · ${t("agents:ui.field.partialQuantity")}: ${p.partial_quantity}`}
        </p>
        {asList(p.questions).length > 0 && (
          <>
            <p>{t("agents:ui.field.questions")}</p>
            <ul>
              {asList(p.questions).map((q, i) => (
                <li key={i}>{q}</li>
              ))}
            </ul>
          </>
        )}
        {asList(p.flags).length > 0 && (
          <p>
            {asList(p.flags).map((f) => (
              <span key={f} className="badge badge-warn">
                {t(`agents:chasing.flags.${f}`, { defaultValue: f })}
              </span>
            ))}
          </p>
        )}
      </div>
    );
  if (ex.agent === "supplier_watch" && asList(p.reasons).length > 0)
    return (
      <ul className="small">
        {asList(p.reasons).map((r) => (
          <li key={r}>{t(`agents:supplier.reason.${r}`, { defaultValue: r })}</li>
        ))}
      </ul>
    );
  if (ex.agent === "import" && Array.isArray(p.findings) && p.findings.length > 0)
    return (
      <ul className="small" data-testid="findings">
        {(p.findings as { code?: string; po_number?: string; rows?: number[]; blocking?: boolean }[]).map(
          (f, i) => (
            <li key={i}>
              {f.po_number} ·{" "}
              {t(`agents:integrations.finding.${f.code ?? ""}`, { defaultValue: f.code ?? "" })}
              {f.rows?.length ? ` · ${t("agents:ui.import.rows", { rows: f.rows.join(", ") })}` : ""}
              {f.blocking && <span className="badge badge-warn">{t("agents:ui.import.blocking")}</span>}
            </li>
          ),
        )}
      </ul>
    );
  if (ex.agent === "cost_analyst" && orgId && ex.subject_id)
    return <QuotationFactsPanel orgId={orgId} quotationId={ex.subject_id} />;
  return null;
}

function QuotationFactsPanel({ orgId, quotationId }: { orgId: string; quotationId: string }) {
  const { data, error } = useApi<QuotationFacts>(
    `/api/v1/orgs/${orgId}/cost/quotations/${quotationId}/facts`,
  );
  if (!data) return <Loading data={data} error={error} />;
  return <FactsTable facts={data} />;
}

/** The model's text behind an exception, labelled as an AI proposal. */
function SuggestionText({ orgId, id }: { orgId: string; id: string }) {
  const { t } = useTranslation();
  const { data, error } = useApi<SuggestionView>(`/api/v1/orgs/${orgId}/ai/suggestions/${id}`);
  if (!data) return <Loading data={data} error={error} />;
  const out = (data.output ?? {}) as Record<string, unknown>;
  return (
    <div className="sub-card ai-assist stack small" data-testid="suggestion">
      <h3>{t("agents:ui.detail.suggestion")}</h3>
      <p className="muted">{t("agents:ui.detail.aiNote", { model: data.model })}</p>
      {data.status === "failed" ? (
        <ErrorText code={data.error_code ?? UNKNOWN} />
      ) : (
        <dl>
          {Object.entries(out)
            .filter(([, v]) => v !== null && v !== "" && !(Array.isArray(v) && v.length === 0))
            .map(([k, v]) => (
              <div key={k}>
                <dt>{t(`agents:ui.out.${k}`, { defaultValue: k })}</dt>
                <dd className="pre">
                  {typeof v === "string" || typeof v === "number" || typeof v === "boolean"
                    ? String(v)
                    : JSON.stringify(v, null, 2)}
                </dd>
              </div>
            ))}
        </dl>
      )}
    </div>
  );
}
