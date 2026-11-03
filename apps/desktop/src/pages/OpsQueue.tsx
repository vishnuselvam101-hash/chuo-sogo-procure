import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "../lib/api-schema";
import { formatDate, formatDateTime } from "../lib/format";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { EmptyState, ErrorText, Loading, PageHeader, StatusBadge } from "../components/ui";

type Queue = components["schemas"]["Queue"];

/** Chuo-Sogo staff: open tasks and RFQ follow-ups across all assigned customers. */
export function OpsQueue({
  selfId,
  timezone,
  onOpen,
}: {
  selfId: string;
  timezone: string;
  onOpen: (orgId: string, requestId: string) => void;
}) {
  const { t, i18n } = useTranslation();
  const [mine, setMine] = useState(false);
  const { data, error, reload } = useApi<Queue>(`/api/v1/ops/queue${mine ? "?mine=true" : ""}`);
  const action = useAction();

  async function update(orgId: string, taskId: string, body: unknown) {
    if (await action.run("PATCH", `/api/v1/orgs/${orgId}/tasks/${taskId}`, body)) reload();
  }

  return (
    <section>
      <PageHeader title={t("nav.opsQueue")}>
        <label className="radio">
          <input type="checkbox" checked={mine} onChange={(e) => setMine(e.target.checked)} />
          {t("procurement:ops.onlyMine")}
        </label>
        <button type="button" onClick={reload}>
          {t("action.refresh")}
        </button>
      </PageHeader>
      <ErrorText code={action.error} />
      <Loading data={data} error={error} />
      {data && (
        <>
          <h2>{t("procurement:ops.tasks")}</h2>
          {data.tasks.length === 0 && <EmptyState hint={t("empty.tasks")} />}
          {data.tasks.length > 0 && (
            <table className="table">
              <thead>
                <tr>
                  <th>{t("procurement:ops.due")}</th>
                  <th>{t("procurement:ops.customer")}</th>
                  <th>{t("procurement:request.number")}</th>
                  <th>{t("procurement:ops.task")}</th>
                  <th>{t("procurement:ops.priority")}</th>
                  <th>{t("procurement:ops.assignee")}</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {data.tasks.map((task) => (
                  <tr key={task.id}>
                    <td>
                      {task.due_at
                        ? formatDateTime(task.due_at, i18n.language, task.org_timezone || timezone)
                        : "—"}
                      {task.overdue && (
                        <span className="badge badge-rejected">{t("procurement:ops.overdue")}</span>
                      )}
                    </td>
                    <td>{task.org_name}</td>
                    <td>
                      {task.request_id && task.request_number ? (
                        <button
                          type="button"
                          className="link"
                          onClick={() => onOpen(task.org_id, task.request_id ?? "")}
                        >
                          {task.request_number}
                        </button>
                      ) : (
                        "—"
                      )}
                    </td>
                    <td>
                      {task.title}
                      <div className="muted small">
                        {t(`procurement:taskKind.${task.kind}`, { defaultValue: task.kind })}
                      </div>
                    </td>
                    <td>
                      <StatusBadge status={task.priority} />
                    </td>
                    <td>{task.assignee_name ?? "—"}</td>
                    <td className="row-actions">
                      {task.assignee_id !== selfId && (
                        <button
                          type="button"
                          onClick={() => void update(task.org_id, task.id, { assignee_id: selfId })}
                        >
                          {t("procurement:ops.take")}
                        </button>
                      )}
                      <button
                        type="button"
                        onClick={() => void update(task.org_id, task.id, { status: "done" })}
                      >
                        {t("procurement:ops.done")}
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <h2>{t("procurement:ops.followUps")}</h2>
          {data.follow_ups.length === 0 && <EmptyState hint={t("empty.followUps")} />}
          {data.follow_ups.length > 0 && (
            <table className="table">
              <thead>
                <tr>
                  <th>{t("procurement:rfq.responseDue")}</th>
                  <th>{t("procurement:ops.customer")}</th>
                  <th>{t("procurement:request.number")}</th>
                  <th>{t("procurement:ops.waitingOn")}</th>
                </tr>
              </thead>
              <tbody>
                {data.follow_ups.map((fu) => (
                  <tr key={fu.rfq_id}>
                    <td>
                      {formatDate(fu.response_due, i18n.language)}{" "}
                      <span className="badge badge-rejected">{t("procurement:ops.overdue")}</span>
                    </td>
                    <td>{fu.org_name}</td>
                    <td>
                      <button type="button" className="link" onClick={() => onOpen(fu.org_id, fu.request_id)}>
                        {`${fu.request_number} / ${fu.rfq_number}`}
                      </button>
                    </td>
                    <td>{fu.waiting_on.join(", ")}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      )}
    </section>
  );
}
