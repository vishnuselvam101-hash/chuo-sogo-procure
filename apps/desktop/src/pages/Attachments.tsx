import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useBridge } from "../lib/BridgeContext";
import { errorCode, ok } from "../lib/bridge";
import { ACCEPTED_FILES, MAX_UPLOAD_BYTES, formatBytes, readAsBase64 } from "../lib/files";
import { formatDateTime } from "../lib/format";
import type { AttachmentView, ItemView, MembershipView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { ErrorText, Loading } from "../components/ui";

interface Props {
  active: MembershipView;
  requestId: string;
  selfId: string;
  items: ItemView[];
  canUpload: boolean;
}

/**
 * Files on a request. Downloads are saved by the native layer into the Downloads folder;
 * the app never opens or renders them.
 */
export function Attachments({ active, requestId, selfId, items, canUpload }: Props) {
  const { t, i18n } = useTranslation();
  const bridge = useBridge();
  const org = `/api/v1/orgs/${active.org_id}`;
  const { data, error, reload } = useApi<AttachmentView[]>(`${org}/requests/${requestId}/attachments`);
  const action = useAction();
  const fileInput = useRef<HTMLInputElement>(null);
  const [itemId, setItemId] = useState("");
  const [saved, setSaved] = useState<string | null>(null);

  async function upload(file: File) {
    setSaved(null);
    if (file.size > MAX_UPLOAD_BYTES) {
      action.setError("validation_failed/file_size");
      return;
    }
    const data_base64 = await readAsBase64(file);
    const res = await action.run("POST", `${org}/requests/${requestId}/attachments`, {
      file_name: file.name,
      data_base64,
      item_id: itemId || null,
    });
    if (fileInput.current) fileInput.current.value = "";
    if (res) reload();
  }

  async function download(a: AttachmentView) {
    action.setError(null);
    try {
      const res = await bridge.download(active.org_id, a.id, a.file_name);
      if (ok(res)) setSaved(res.body.saved_to);
      else action.setError(errorCode(res));
    } catch (e) {
      // Native-side refusals (file name, size, Downloads folder) arrive as plain codes.
      action.setError(typeof e === "string" ? e : "unknown");
    }
  }

  async function remove(a: AttachmentView) {
    if (await action.run("DELETE", `${org}/attachments/${a.id}`)) reload();
  }

  const itemLabel = (id: string | null | undefined) => {
    const item = items.find((i) => i.id === id);
    return item ? `${item.line_no}. ${item.description}` : "";
  };

  return (
    <div className="card stack">
      <h2>{t("procurement:attachment.heading")}</h2>
      <Loading data={data} error={error} />
      {data && data.length === 0 && <p className="muted">{t("state.empty")}</p>}
      {data && data.length > 0 && (
        <table className="table">
          <thead>
            <tr>
              <th>{t("procurement:attachment.file")}</th>
              <th>{t("procurement:attachment.item")}</th>
              <th className="num">{t("procurement:attachment.size")}</th>
              <th>{t("procurement:attachment.uploadedBy")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {data.map((a) => (
              <tr key={a.id}>
                <td>{a.file_name}</td>
                <td className="small">{itemLabel(a.item_id)}</td>
                <td className="num">{formatBytes(a.size_bytes, i18n.language)}</td>
                <td className="small">
                  {a.uploaded_by_name ?? ""}
                  <div className="muted">{formatDateTime(a.created_at, i18n.language, active.timezone)}</div>
                </td>
                <td className="row-actions">
                  <button type="button" onClick={() => void download(a)}>
                    {t("procurement:attachment.download")}
                  </button>
                  {a.uploaded_by === selfId && (
                    <button type="button" onClick={() => void remove(a)}>
                      {t("procurement:item.remove")}
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {saved && (
        <p className="ok small" role="status">
          {t("procurement:attachment.savedTo", { path: saved })}
        </p>
      )}
      {canUpload && (
        <div className="row">
          {items.length > 0 && (
            <label>
              {t("procurement:attachment.item")}
              <select value={itemId} onChange={(e) => setItemId(e.target.value)}>
                <option value="">{t("procurement:attachment.wholeRequest")}</option>
                {items.map((i) => (
                  <option key={i.id} value={i.id}>
                    {itemLabel(i.id)}
                  </option>
                ))}
              </select>
            </label>
          )}
          <label className="grow">
            {t("procurement:attachment.add")}
            <input
              ref={fileInput}
              type="file"
              accept={ACCEPTED_FILES}
              disabled={action.busy}
              onChange={(e) => {
                const file = e.target.files?.[0];
                if (file) void upload(file);
              }}
            />
          </label>
        </div>
      )}
      {canUpload && <p className="muted small">{t("procurement:attachment.allowed")}</p>}
      <ErrorText code={action.error} />
    </div>
  );
}
