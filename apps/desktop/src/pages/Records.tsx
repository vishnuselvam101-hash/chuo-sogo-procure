import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useBridge } from "../lib/BridgeContext";
import { errorCode, ok } from "../lib/bridge";
import { formatDate, parseMinor } from "../lib/format";
import type { MembershipView, RecordList } from "../lib/types";
import { useApi } from "../lib/useApi";
import { ErrorText, Loading, Money, PageHeader } from "../components/ui";

const KINDS = ["quotation", "order", "supplier_invoice", "fee_invoice"];

/**
 * The factory's electronic transaction records (電子帳簿保存法): every quotation, order and
 * invoice, found by date, amount and counterparty. Supplier files are kept as received and
 * can be saved with their SHA-256 for checking.
 */
export function Records({ active }: { active: MembershipView }) {
  const { t, i18n } = useTranslation();
  const bridge = useBridge();
  const [form, setForm] = useState({ from: "", to: "", counterparty: "", min: "", max: "", kind: "" });
  const [query, setQuery] = useState("");
  const [saved, setSaved] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { data, error: loadError } = useApi<RecordList>(`/api/v1/orgs/${active.org_id}/records${query}`);
  const lang = i18n.language;

  function search() {
    const q = new URLSearchParams();
    if (form.from) q.set("from", form.from);
    if (form.to) q.set("to", form.to);
    if (form.counterparty.trim()) q.set("counterparty", form.counterparty.trim());
    for (const [key, v] of [
      ["min_minor", form.min],
      ["max_minor", form.max],
    ] as const) {
      if (!v.trim()) continue;
      const minor = parseMinor(v, "JPY");
      if (minor === null) return setError("validation_failed/amount");
      q.set(key, String(minor));
    }
    if (form.kind) q.set("kind", form.kind);
    setError(null);
    const s = q.toString();
    setQuery(s ? `?${s}` : "");
  }

  async function save(id: string, name: string) {
    const res = await bridge.documentDownload(active.org_id, id, name);
    if (ok(res)) {
      setSaved(res.body.saved_to);
      setError(null);
    } else setError(errorCode(res));
  }

  return (
    <section className="stack">
      <PageHeader title={t("nav.records")} />
      <p className="lead">{t("records:lead")}</p>
      <div className="card row">
        <label>
          {t("records:from")}
          <input type="date" value={form.from} onChange={(e) => setForm({ ...form, from: e.target.value })} />
        </label>
        <label>
          {t("records:to")}
          <input type="date" value={form.to} onChange={(e) => setForm({ ...form, to: e.target.value })} />
        </label>
        <label className="grow">
          {t("records:counterparty")}
          <input
            maxLength={100}
            value={form.counterparty}
            onChange={(e) => setForm({ ...form, counterparty: e.target.value })}
          />
        </label>
        <label>
          {t("records:min")}
          <input
            inputMode="numeric"
            value={form.min}
            onChange={(e) => setForm({ ...form, min: e.target.value })}
          />
        </label>
        <label>
          {t("records:max")}
          <input
            inputMode="numeric"
            value={form.max}
            onChange={(e) => setForm({ ...form, max: e.target.value })}
          />
        </label>
        <label>
          {t("records:kind")}
          <select value={form.kind} onChange={(e) => setForm({ ...form, kind: e.target.value })}>
            <option value="">{t("records:allKinds")}</option>
            {KINDS.map((k) => (
              <option key={k} value={k}>
                {t(`records:kinds.${k}`)}
              </option>
            ))}
          </select>
        </label>
        <button type="button" className="primary" onClick={search}>
          {t("records:search")}
        </button>
      </div>
      <ErrorText code={error} />
      {saved && <p className="notice">{t("records:saved", { path: saved })}</p>}
      <Loading data={data} error={loadError} />
      {data && data.rows.length === 0 && <p className="muted">{t("records:none")}</p>}
      {data && data.rows.length > 0 && (
        <table className="table" data-testid="records">
          <thead>
            <tr>
              <th>{t("records:date")}</th>
              <th>{t("records:kind")}</th>
              <th>{t("records:counterparty")}</th>
              <th>{t("records:reference")}</th>
              <th className="num">{t("records:amount")}</th>
              <th>{t("records:document")}</th>
            </tr>
          </thead>
          <tbody>
            {data.rows.map((r) => (
              <tr key={`${r.kind}-${r.id}`}>
                <td>{formatDate(r.date, lang)}</td>
                <td>{t(`records:kinds.${r.kind}`)}</td>
                <td>{r.counterparty}</td>
                <td>
                  {r.reference} {r.request_number && <span className="muted small">{r.request_number}</span>}
                </td>
                <td className="num">
                  <Money minor={r.amount_minor} currency={r.currency} />
                </td>
                <td>
                  {r.document_id && r.document_name ? (
                    <>
                      <button
                        type="button"
                        className="link"
                        onClick={() => void save(r.document_id ?? "", r.document_name ?? "")}
                      >
                        {r.document_name}
                      </button>
                      <div className="muted small mono" title={r.document_sha256 ?? ""}>
                        {t("records:sha", { hash: r.document_sha256?.slice(0, 12) ?? "" })}
                      </div>
                    </>
                  ) : (
                    "—"
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {data?.truncated && <p className="muted small">{t("records:truncated")}</p>}
    </section>
  );
}
