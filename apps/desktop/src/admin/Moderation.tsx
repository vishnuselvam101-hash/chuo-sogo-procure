import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useApi } from "../lib/useApi";
import { useAction } from "../lib/useAction";
import { formatDateTime } from "../lib/format";
import type { ListingDetail, ListingSummary } from "../lib/types";
import { ErrorText, Loading, PageHeader } from "../components/ui";
import { ListingPhoto, ListingStatusBadge, ListingView, Price } from "../catalogue/ListingView";
import { ConfirmButton } from "./widgets";
import { ADMIN_API, PLATFORM_TZ } from "./types";

const STATUSES = ["submitted", "approved", "rejected", "draft", "retired"] as const;
const ACTIONS = { approve: "approve", reject: "reject", retire: "retire" } as const;

function Review({ id, onBack }: { id: string; onBack: () => void }) {
  const { t } = useTranslation();
  const path = `${ADMIN_API}/listings/${id}`;
  const { data, error, reload } = useApi<ListingDetail>(path);
  const { run, busy, error: actionError } = useAction();
  const [note, setNote] = useState("");
  if (!data) return <Loading data={data} error={error} />;
  const act = async (action: string) => {
    if (await run("POST", `${path}/${action}`, { note })) {
      setNote("");
      reload();
    }
  };
  return (
    <section>
      <PageHeader title={data.title || t("catalogue:untitled")}>
        <ListingStatusBadge status={data.status} />
        <button type="button" onClick={onBack}>
          {t("action.back")}
        </button>
      </PageHeader>
      {data.duplicates.length > 0 && (
        <div className="notice" data-testid="duplicates">
          <p>{t("catalogue:duplicatesFound", { count: data.duplicates.length })}</p>
          <ul>
            {data.duplicates.map((d) => (
              <li key={d.id}>
                {d.supplier_name}: {d.title} (<ListingStatusBadge status={d.status} />)
              </li>
            ))}
          </ul>
        </div>
      )}
      <ListingView l={data} mediaPath={(m) => `${path}/media/${m}/content`} />
      {data.review_note && (
        <p className="muted">
          {t("catalogue:lastNote")}: {data.review_note}
        </p>
      )}
      {(data.status === "submitted" || data.status === "approved") && (
        <div className="card stack" data-testid="moderation-actions">
          <label>
            {t("catalogue:reviewNote")}
            <textarea maxLength={2000} value={note} onChange={(e) => setNote(e.target.value)} />
          </label>
          <p className="muted small">{t("catalogue:reviewHint")}</p>
          <div className="row">
            {data.status === "submitted" && (
              <>
                <button
                  type="button"
                  className="primary"
                  disabled={busy}
                  onClick={() => void act(ACTIONS.approve)}
                >
                  {t("catalogue:approve")}
                </button>
                <button
                  type="button"
                  disabled={busy || !note.trim()}
                  onClick={() => void act(ACTIONS.reject)}
                >
                  {t("catalogue:reject")}
                </button>
              </>
            )}
            {data.status === "approved" && (
              <ConfirmButton
                label={t("catalogue:retire")}
                confirmLabel={t("catalogue:confirmRetire")}
                disabled={busy}
                onConfirm={() => void act(ACTIONS.retire)}
              />
            )}
          </div>
          <ErrorText code={actionError} />
        </div>
      )}
    </section>
  );
}

/** Review queue: suppliers' listings waiting for Chuo-Sogo's approval. */
export function Moderation() {
  const { t, i18n } = useTranslation();
  const [status, setStatus] = useState<string>(STATUSES[0]);
  const { data, error, reload } = useApi<ListingSummary[]>(`${ADMIN_API}/listings?status=${status}`);
  const [open, setOpen] = useState<string | null>(null);
  if (open)
    return (
      <Review
        key={open}
        id={open}
        onBack={() => {
          setOpen(null);
          reload();
        }}
      />
    );
  return (
    <section>
      <PageHeader title={t("admin:nav.listings")}>
        <select
          aria-label={t("catalogue:statusLabel")}
          value={status}
          onChange={(e) => setStatus(e.target.value)}
        >
          {STATUSES.map((s) => (
            <option key={s} value={s}>
              {t(`catalogue:status.${s}`)}
            </option>
          ))}
        </select>
        <button type="button" onClick={reload}>
          {t("action.refresh")}
        </button>
      </PageHeader>
      <Loading data={data} error={error} />
      {data && data.length === 0 && <p className="muted">{t("catalogue:queueEmpty")}</p>}
      {data && data.length > 0 && (
        <table className="table" data-testid="moderation-list">
          <thead>
            <tr>
              <th />
              <th>{t("catalogue:titleLabel")}</th>
              <th>{t("catalogue:supplier")}</th>
              <th>{t("catalogue:model")}</th>
              <th>{t("catalogue:price")}</th>
              <th>{t("catalogue:submittedAt")}</th>
            </tr>
          </thead>
          <tbody>
            {data.map((l) => (
              <tr key={l.id}>
                <td>
                  {l.cover_media_id && (
                    <ListingPhoto
                      path={`${ADMIN_API}/listings/${l.id}/media/${l.cover_media_id}/content`}
                      alt=""
                      className="thumb"
                    />
                  )}
                </td>
                <td>
                  <button type="button" className="link" onClick={() => setOpen(l.id)}>
                    {l.title || t("catalogue:untitled")}
                  </button>
                </td>
                <td>{l.supplier_name}</td>
                <td>
                  <code>{l.model_number}</code>
                </td>
                <td>
                  <Price l={l} />
                </td>
                <td>{l.submitted_at ? formatDateTime(l.submitted_at, i18n.language, PLATFORM_TZ) : "—"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
