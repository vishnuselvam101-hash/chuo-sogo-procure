import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "../lib/api-schema";
import { formatDate, formatDateTime } from "../lib/format";
import type { MembershipView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { ErrorText, Loading, StatusBadge } from "../components/ui";

type Task = components["schemas"]["TaskView"];
type Communication = components["schemas"]["CommunicationView"];
type TimeSheet = components["schemas"]["TimeSheet"];

const TASK_KINDS = ["follow_up", "exception", "source", "compare", "other"];
const PRIORITIES = ["normal", "high", "urgent", "low"];
const DIRECTIONS = ["outbound", "inbound"];
const CHANNELS = ["email", "phone", "meeting", "chat", "other"];

/** Chuo-Sogo only: tasks, supplier communications and service time for one request. */
export function StaffPanel({
  active,
  requestId,
  selfId,
}: {
  active: MembershipView;
  requestId: string;
  selfId: string;
}) {
  const { t, i18n } = useTranslation();
  const org = `/api/v1/orgs/${active.org_id}`;
  const base = `${org}/requests/${requestId}`;
  const tasks = useApi<Task[]>(`${base}/tasks`);
  const comms = useApi<Communication[]>(`${base}/communications`);
  const time = useApi<TimeSheet>(`${base}/time`);
  const action = useAction();
  const [task, setTask] = useState({ kind: "follow_up", title: "", priority: "normal", due: "" });
  const [comm, setComm] = useState({ direction: "outbound", channel: "email", summary: "" });
  const [minutes, setMinutes] = useState("");
  const [note, setNote] = useState("");

  async function addTask() {
    const res = await action.run("POST", `${org}/tasks`, {
      request_id: requestId,
      kind: task.kind,
      title: task.title,
      priority: task.priority,
      // The server turns the date into the end of that day in the customer's timezone.
      due_date: task.due || null,
      assignee_id: selfId,
    });
    if (res) {
      setTask({ ...task, title: "", due: "" });
      tasks.reload();
    }
  }
  async function done(id: string) {
    if (await action.run("PATCH", `${org}/tasks/${id}`, { status: "done" })) tasks.reload();
  }
  async function logComm() {
    if (await action.run("POST", `${base}/communications`, comm)) {
      setComm({ ...comm, summary: "" });
      comms.reload();
    }
  }
  async function logTime() {
    const m = /^\d{1,4}$/.test(minutes.trim()) ? Number(minutes) : NaN;
    if (Number.isNaN(m)) return action.setError("validation_failed/minutes");
    if (await action.run("POST", `${base}/time`, { minutes: m, note })) {
      setMinutes("");
      setNote("");
      time.reload();
    }
  }

  return (
    <details className="card internal-panel" open>
      <summary>{t("procurement:ops.internalPanel")}</summary>
      <div className="stack">
        <h3>{t("procurement:ops.tasks")}</h3>
        <Loading data={tasks.data} error={tasks.error} />
        <ul className="plain stack">
          {tasks.data?.map((k) => (
            <li key={k.id} className="row">
              <StatusBadge status={k.status} />
              <span>{k.title}</span>
              <span className="muted small">
                {t(`procurement:taskKind.${k.kind}`, { defaultValue: k.kind })}
                {k.due_at && ` · ${formatDateTime(k.due_at, i18n.language, active.timezone)}`}
                {k.assignee_name && ` · ${k.assignee_name}`}
              </span>
              {k.overdue && <span className="badge badge-rejected">{t("procurement:ops.overdue")}</span>}
              {(k.status === "open" || k.status === "in_progress") && (
                <button type="button" onClick={() => void done(k.id)}>
                  {t("procurement:ops.done")}
                </button>
              )}
            </li>
          ))}
        </ul>
        <div className="row">
          <label>
            {t("procurement:ops.kind")}
            <select value={task.kind} onChange={(e) => setTask({ ...task, kind: e.target.value })}>
              {TASK_KINDS.map((k) => (
                <option key={k} value={k}>
                  {t(`procurement:taskKind.${k}`)}
                </option>
              ))}
            </select>
          </label>
          <label className="grow">
            {t("procurement:ops.task")}
            <input
              maxLength={200}
              value={task.title}
              onChange={(e) => setTask({ ...task, title: e.target.value })}
            />
          </label>
          <label>
            {t("procurement:ops.priority")}
            <select value={task.priority} onChange={(e) => setTask({ ...task, priority: e.target.value })}>
              {PRIORITIES.map((p) => (
                <option key={p} value={p}>
                  {t(`status.${p}`)}
                </option>
              ))}
            </select>
          </label>
          <label>
            {t("procurement:ops.due")}
            <input type="date" value={task.due} onChange={(e) => setTask({ ...task, due: e.target.value })} />
          </label>
          <button type="button" disabled={!task.title.trim() || action.busy} onClick={() => void addTask()}>
            {t("procurement:ops.addTask")}
          </button>
        </div>

        <h3>{t("procurement:ops.communications")}</h3>
        <ul className="plain stack">
          {comms.data?.map((c) => (
            <li key={c.id}>
              <span className="muted small">
                {formatDateTime(c.occurred_at, i18n.language, active.timezone)} ·{" "}
                {t(`procurement:direction.${c.direction}`)} · {t(`procurement:channel.${c.channel}`)}
                {c.supplier_name && ` · ${c.supplier_name}`}
              </span>
              <div className="pre">{c.summary}</div>
            </li>
          ))}
        </ul>
        <div className="row">
          <label>
            {t("procurement:ops.direction")}
            <select value={comm.direction} onChange={(e) => setComm({ ...comm, direction: e.target.value })}>
              {DIRECTIONS.map((d) => (
                <option key={d} value={d}>
                  {t(`procurement:direction.${d}`)}
                </option>
              ))}
            </select>
          </label>
          <label>
            {t("procurement:ops.channel")}
            <select value={comm.channel} onChange={(e) => setComm({ ...comm, channel: e.target.value })}>
              {CHANNELS.map((c) => (
                <option key={c} value={c}>
                  {t(`procurement:channel.${c}`)}
                </option>
              ))}
            </select>
          </label>
          <label className="grow">
            {t("procurement:ops.summary")}
            <input
              maxLength={4000}
              value={comm.summary}
              onChange={(e) => setComm({ ...comm, summary: e.target.value })}
            />
          </label>
          <button type="button" disabled={!comm.summary.trim() || action.busy} onClick={() => void logComm()}>
            {t("procurement:ops.log")}
          </button>
        </div>

        <h3>{t("procurement:ops.time")}</h3>
        {time.data && (
          <p className="small">{t("procurement:ops.totalTime", { minutes: time.data.total_minutes })}</p>
        )}
        <ul className="plain small">
          {time.data?.entries.map((e) => (
            <li key={e.id}>
              {formatDate(e.work_date, i18n.language)} · {e.user_name ?? ""} ·{" "}
              {t("procurement:ops.minutes", { minutes: e.minutes })} {e.note && `· ${e.note}`}
            </li>
          ))}
        </ul>
        <div className="row">
          <label className="narrow">
            {t("procurement:ops.minutesLabel")}
            <input inputMode="numeric" value={minutes} onChange={(e) => setMinutes(e.target.value)} />
          </label>
          <label className="grow">
            {t("procurement:ops.note")}
            <input maxLength={1000} value={note} onChange={(e) => setNote(e.target.value)} />
          </label>
          <button type="button" disabled={!minutes.trim() || action.busy} onClick={() => void logTime()}>
            {t("procurement:ops.log")}
          </button>
        </div>
        <ErrorText code={action.error} />
      </div>
    </details>
  );
}
