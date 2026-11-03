import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "../lib/api-schema";
import { formatDateTime } from "../lib/format";
import { P } from "../lib/perm";
import type { MembershipView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { EmptyState, ErrorText, Loading } from "../components/ui";

type Comment = components["schemas"]["CommentView"];
const CUSTOMER = "customer";
const INTERNAL = "internal";

/** Conversation on a request. Staff can also write internal notes the customer never sees. */
export function Comments({ active, requestId }: { active: MembershipView; requestId: string }) {
  const { t, i18n } = useTranslation();
  const path = `/api/v1/orgs/${active.org_id}/requests/${requestId}/comments`;
  const { data, error, reload } = useApi<Comment[]>(path);
  const action = useAction();
  const [body, setBody] = useState("");
  const [visibility, setVisibility] = useState(CUSTOMER);
  const canInternal = active.permissions.includes(P.viewInternalNotes);

  async function send() {
    if (await action.run("POST", path, { body, visibility })) {
      setBody("");
      reload();
    }
  }

  return (
    <div className="card stack">
      <h2>{t("procurement:comment.heading")}</h2>
      <Loading data={data} error={error} />
      {data && data.length === 0 && <EmptyState hint={t("empty.comments")} />}
      <ul className="plain stack">
        {data?.map((c) => (
          <li key={c.id} className={c.visibility === INTERNAL ? "internal-note" : undefined}>
            <div className="small muted">
              {c.author_name ?? ""} · {formatDateTime(c.created_at, i18n.language, active.timezone)}
              {c.visibility === INTERNAL && (
                <span className="badge badge-warn">{t("procurement:comment.internal")}</span>
              )}
            </div>
            <div className="pre">{c.body}</div>
          </li>
        ))}
      </ul>
      <label>
        {t("procurement:comment.add")}
        <textarea rows={2} maxLength={4000} value={body} onChange={(e) => setBody(e.target.value)} />
      </label>
      <div className="row">
        {canInternal && (
          <label className="compact">
            {t("procurement:comment.visibility")}
            <select value={visibility} onChange={(e) => setVisibility(e.target.value)}>
              <option value={CUSTOMER}>{t("procurement:comment.shared")}</option>
              <option value={INTERNAL}>{t("procurement:comment.internal")}</option>
            </select>
          </label>
        )}
        <button
          type="button"
          className="primary"
          disabled={!body.trim() || action.busy}
          onClick={() => void send()}
        >
          {t("procurement:comment.send")}
        </button>
      </div>
      <ErrorText code={action.error} />
    </div>
  );
}
