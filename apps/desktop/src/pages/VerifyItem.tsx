import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { ItemView, ProductView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { ErrorText, StatusBadge } from "../components/ui";

const VERIFIED = "verified";
const HTTPS = "https://";

/**
 * Operator: link a request item to a verified catalogue record. If none exists yet, create
 * one with its evidence and verify it in the same step. The backend refuses unverified
 * records, so an item can only be verified against evidence.
 */
export function VerifyItem({
  item,
  onVerify,
}: {
  item: ItemView;
  onVerify: (productId: string, note: string) => Promise<void>;
}) {
  const { t } = useTranslation();
  const [q, setQ] = useState(item.mpn || item.description);
  const { data, reload } = useApi<ProductView[]>(
    `/api/v1/products?status=${VERIFIED}&q=${encodeURIComponent(q.trim())}`,
  );
  const [chosen, setChosen] = useState("");
  const [note, setNote] = useState("");
  const [creating, setCreating] = useState(false);
  const [draft, setDraft] = useState({ manufacturer: item.manufacturer, mpn: item.mpn, url: "" });
  const action = useAction();

  async function createAndVerify() {
    const res = await action.run<{ id: string }>("POST", "/api/v1/products", {
      manufacturer: draft.manufacturer,
      mpn: draft.mpn,
      description: item.description,
      evidence: [{ kind: "datasheet", url: draft.url.trim(), note: "" }],
    });
    if (!res) return;
    if (await action.run("POST", `/api/v1/products/${res.body.id}/status`, { status: VERIFIED, note: "" })) {
      setCreating(false);
      setChosen(res.body.id);
      reload();
    }
  }

  return (
    <div className="stack sub-card">
      <label>
        {t("procurement:verify.search")}
        <input value={q} onChange={(e) => setQ(e.target.value)} />
      </label>
      {data && data.length === 0 && <p className="muted small">{t("procurement:verify.noMatch")}</p>}
      {data?.map((p) => (
        <label key={p.id} className="radio">
          <input
            type="radio"
            name={`product-${item.id}`}
            checked={chosen === p.id}
            onChange={() => setChosen(p.id)}
          />
          {`${p.manufacturer} ${p.mpn}`} <span className="muted small">{p.description}</span>
          <StatusBadge status={p.status} />
        </label>
      ))}
      <button type="button" className="link" onClick={() => setCreating(!creating)}>
        {t("procurement:verify.createRecord")}
      </button>
      {creating && (
        <div className="row">
          <label>
            {t("procurement:item.manufacturer")}
            <input
              value={draft.manufacturer}
              onChange={(e) => setDraft({ ...draft, manufacturer: e.target.value })}
            />
          </label>
          <label>
            {t("procurement:item.mpn")}
            <input value={draft.mpn} onChange={(e) => setDraft({ ...draft, mpn: e.target.value })} />
          </label>
          <label className="grow">
            {t("procurement:product.evidenceUrl")}
            <input
              type="url"
              placeholder={HTTPS}
              value={draft.url}
              onChange={(e) => setDraft({ ...draft, url: e.target.value })}
            />
          </label>
          <button
            type="button"
            disabled={
              action.busy || !draft.manufacturer.trim() || !draft.mpn.trim() || !draft.url.startsWith(HTTPS)
            }
            onClick={() => void createAndVerify()}
          >
            {t("procurement:verify.createAndVerify")}
          </button>
        </div>
      )}
      <label>
        {t("procurement:verify.note")}
        <input maxLength={1000} value={note} onChange={(e) => setNote(e.target.value)} />
      </label>
      <div className="row">
        <button
          type="button"
          className="primary"
          disabled={!chosen || action.busy}
          onClick={() => void onVerify(chosen, note)}
        >
          {t("procurement:item.confirmVerified")}
        </button>
      </div>
      <ErrorText code={action.error} />
    </div>
  );
}
