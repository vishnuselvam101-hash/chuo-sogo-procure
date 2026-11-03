import { useState } from "react";
import { useTranslation } from "react-i18next";
import { readAsBase64, MAX_UPLOAD_BYTES } from "../lib/files";
import {
  formatDate,
  formatDateTime,
  formatMilliPrice,
  formatMilliQuantity,
  formatNumber,
} from "../lib/format";
import { P } from "../lib/perm";
import type { HistoryImport, HistoryPreview, HistoryRow, ItemAlias, MembershipView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { ConfirmButton, ErrorText, Loading, PageHeader } from "../components/ui";
import { BuyingCriteriaCard } from "./BuyingCriteria";
import { Opportunities } from "./Opportunities";

/** Field codes understood by the importer (procure_domain::history::Field). */
export const FIELDS = [
  "date",
  "supplier",
  "description",
  "manufacturer",
  "mpn",
  "quantity",
  "unit",
  "unit_price",
  "line_total",
  "currency",
  "lead_time_days",
  "notes",
] as const;

const CURRENCIES = ["JPY", "USD", "EUR", "CNY", "KRW", "INR"];

interface Chosen {
  name: string;
  data_base64: string;
}

/**
 * The customer's decision material: past purchases imported from their own Excel/CSV exports,
 * and their written buying criteria. Import is two-step: check (nothing stored), then import.
 */
export function PurchaseHistory({ active }: { active: MembershipView }) {
  const { t, i18n } = useTranslation();
  const base = `/api/v1/orgs/${active.org_id}`;
  const canManage = active.permissions.includes(P.managePurchaseHistory);
  const action = useAction();
  const imports = useApi<HistoryImport[]>(`${base}/purchase-history/imports`);
  const [file, setFile] = useState<Chosen | null>(null);
  const [currency, setCurrency] = useState("JPY");
  const [preview, setPreview] = useState<HistoryPreview | null>(null);
  const [mapping, setMapping] = useState<(string | null)[] | null>(null);
  const [done, setDone] = useState<HistoryImport | null>(null);
  const [confirming, setConfirming] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [submitted, setSubmitted] = useState("");
  const rows = useApi<HistoryRow[]>(
    `${base}/purchase-history?limit=200${submitted ? `&q=${encodeURIComponent(submitted)}` : ""}`,
  );

  const payload = (f: Chosen, m: (string | null)[] | null, p: HistoryPreview | null) => ({
    file_name: f.name,
    data_base64: f.data_base64,
    currency,
    ...(m && p ? { mapping: m, header_row: p.header_row } : {}),
  });

  async function check(f: Chosen, m: (string | null)[] | null, p: HistoryPreview | null) {
    setDone(null);
    const res = await action.run<HistoryPreview>(
      "POST",
      `${base}/purchase-history/preview`,
      payload(f, m, p),
    );
    setPreview(res?.body ?? null);
    setMapping(res?.body.mapping ?? null);
  }

  async function choose(list: FileList | null) {
    const picked = list?.[0];
    setPreview(null);
    setMapping(null);
    if (!picked) return;
    if (picked.size > MAX_UPLOAD_BYTES) {
      action.setError("validation_failed/file_size");
      return;
    }
    const f = { name: picked.name, data_base64: await readAsBase64(picked) };
    setFile(f);
    await check(f, null, null);
  }

  async function commit() {
    if (!file || !preview) return;
    const res = await action.run<HistoryImport>(
      "POST",
      `${base}/purchase-history/imports`,
      payload(file, mapping, preview),
    );
    if (!res) return;
    setDone(res.body);
    setFile(null);
    setPreview(null);
    setMapping(null);
    imports.reload();
    rows.reload();
  }

  async function withdraw(id: string) {
    setConfirming(null);
    if (await action.run("POST", `${base}/purchase-history/imports/${id}/withdraw`, {})) {
      imports.reload();
      rows.reload();
    }
  }

  const mappingChanged = preview && mapping && mapping.some((m, i) => m !== (preview.mapping[i] ?? null));

  return (
    <section>
      <PageHeader title={t("nav.history")} />
      <p className="lead">{t("history:lead")}</p>

      <Opportunities orgId={active.org_id} />

      {canManage && (
        <div className="card stack">
          <h2>{t("history:import.heading")}</h2>
          <p className="muted">{t("history:import.hint")}</p>
          <div className="row">
            <label className="grow">
              {t("history:import.file")}
              <input
                type="file"
                accept=".xlsx,.csv,.tsv,.txt,.pdf,.png,.jpg,.jpeg,.tif,.tiff"
                disabled={action.busy}
                onChange={(e) => void choose(e.target.files)}
              />
            </label>
            <label>
              {t("history:import.currency")}
              <select value={currency} onChange={(e) => setCurrency(e.target.value)}>
                {CURRENCIES.map((c) => (
                  <option key={c} value={c}>
                    {c}
                  </option>
                ))}
              </select>
            </label>
          </div>

          {preview && mapping && file && (
            <>
              {preview.ocr && (
                <div className="notice" role="note">
                  {t("history:import.ocrNotice")}
                </div>
              )}
              <p>
                {t("history:preview.summary", {
                  valid: formatNumber(preview.valid_rows, i18n.language),
                  total: formatNumber(preview.data_rows, i18n.language),
                })}
              </p>
              <h3>{t("history:preview.columns")}</h3>
              <div className="table-scroll">
                <table className="table">
                  <thead>
                    <tr>
                      {preview.headers.map((h, i) => (
                        <th key={i}>{h || "—"}</th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    <tr>
                      {preview.headers.map((_, i) => (
                        <td key={i}>
                          <select
                            aria-label={t("history:preview.columnOf", { name: preview.headers[i] || i + 1 })}
                            value={mapping[i] ?? ""}
                            onChange={(e) => {
                              const next = [...mapping];
                              next[i] = e.target.value || null;
                              setMapping(next);
                            }}
                          >
                            <option value="">{t("history:field.ignore")}</option>
                            {FIELDS.map((f) => (
                              <option key={f} value={f}>
                                {t(`history:field.${f}`)}
                              </option>
                            ))}
                          </select>
                        </td>
                      ))}
                    </tr>
                  </tbody>
                </table>
              </div>

              {Object.keys(preview.error_counts).length > 0 && (
                <div className="notice">
                  <strong>{t("history:preview.skipped")}</strong>
                  <ul>
                    {Object.entries(preview.error_counts).map(([code, n]) => (
                      <li key={code}>
                        {t(`history:reason.${code}`, { defaultValue: code })}:{" "}
                        {formatNumber(n, i18n.language)}
                        <span className="muted">
                          {" "}
                          ({t("history:preview.rows")}{" "}
                          {preview.errors
                            .filter((e) => e.code === code)
                            .slice(0, 10)
                            .map((e) => e.row)
                            .join(", ")}
                          )
                        </span>
                      </li>
                    ))}
                  </ul>
                </div>
              )}

              <h3>{t("history:preview.sample")}</h3>
              <HistoryTable
                rows={preview.sample.map((r) => ({ ...r, id: String(r.row), notes: "" }))}
                showRow
              />

              <div className="row">
                {mappingChanged ? (
                  <button
                    type="button"
                    className="primary"
                    disabled={action.busy}
                    onClick={() => void check(file, mapping, preview)}
                  >
                    {t("history:preview.recheck")}
                  </button>
                ) : (
                  <button
                    type="button"
                    className="primary"
                    disabled={action.busy || preview.valid_rows === 0}
                    onClick={() => void commit()}
                  >
                    {t("history:import.commit", { count: preview.valid_rows })}
                  </button>
                )}
              </div>
            </>
          )}
          {done && (
            <p role="status" className="success">
              {t("history:import.done", { imported: done.rows_imported, skipped: done.rows_skipped })}
            </p>
          )}
          <ErrorText code={action.error} />
        </div>
      )}

      <h2>{t("history:imports.heading")}</h2>
      <Loading data={imports.data} error={imports.error} />
      {imports.data?.length === 0 && <p className="muted">{t("history:imports.none")}</p>}
      {imports.data && imports.data.length > 0 && (
        <table className="table">
          <thead>
            <tr>
              <th>{t("history:imports.file")}</th>
              <th>{t("history:imports.source")}</th>
              <th className="num">{t("history:imports.rows")}</th>
              <th className="num">{t("history:imports.skipped")}</th>
              <th>{t("history:imports.by")}</th>
              <th>{t("history:imports.at")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {imports.data.map((im) => (
              <tr key={im.id} className={im.withdrawn_at ? "muted" : undefined}>
                <td>{im.file_name}</td>
                <td>{t(`history:source.${im.source}`, { defaultValue: im.source })}</td>
                <td className="num">{formatNumber(im.rows_imported, i18n.language)}</td>
                <td className="num">{formatNumber(im.rows_skipped, i18n.language)}</td>
                <td>{im.imported_by_name ?? "—"}</td>
                <td>{formatDateTime(im.imported_at, i18n.language, active.timezone)}</td>
                <td className="row-actions">
                  {im.withdrawn_at ? (
                    t("history:imports.withdrawn")
                  ) : canManage && confirming === im.id ? (
                    <>
                      <button type="button" className="danger" onClick={() => void withdraw(im.id)}>
                        {t("history:imports.confirmWithdraw")}
                      </button>
                      <button type="button" onClick={() => setConfirming(null)}>
                        {t("action.cancel")}
                      </button>
                    </>
                  ) : (
                    canManage && (
                      <button type="button" onClick={() => setConfirming(im.id)}>
                        {t("history:imports.withdraw")}
                      </button>
                    )
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <h2>{t("history:search.heading")}</h2>
      <form
        className="row"
        onSubmit={(e) => {
          e.preventDefault();
          setSubmitted(query.trim());
        }}
      >
        <input
          className="grow"
          type="search"
          maxLength={200}
          placeholder={t("history:search.placeholder")}
          aria-label={t("history:search.heading")}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <button type="submit">{t("history:search.go")}</button>
      </form>
      <Loading data={rows.data} error={rows.error} />
      {rows.data?.length === 0 && <p className="muted">{t("history:search.none")}</p>}
      {rows.data && rows.data.length > 0 && <HistoryTable rows={rows.data} />}

      <AliasList active={active} />
      <BuyingCriteriaCard active={active} />
    </section>
  );
}

/** Items confirmed as the same thing (written differently); removing one undoes the match. */
function AliasList({ active }: { active: MembershipView }) {
  const { t, i18n } = useTranslation();
  const path = `/api/v1/orgs/${active.org_id}/item-aliases`;
  const canManage = active.permissions.includes(P.managePurchaseHistory);
  const aliases = useApi<ItemAlias[]>(path);
  const action = useAction();
  const label = (key: string) => key.slice(2);

  async function remove(id: string) {
    if (await action.run("DELETE", `${path}/${id}`)) aliases.reload();
  }

  return (
    <div className="card stack">
      <h2>{t("history:aliases.heading")}</h2>
      <p className="muted">{t("history:aliases.hint")}</p>
      <Loading data={aliases.data} error={aliases.error} />
      {aliases.data?.length === 0 && <p className="muted">{t("history:aliases.none")}</p>}
      {aliases.data && aliases.data.length > 0 && (
        <table className="table">
          <thead>
            <tr>
              <th>{t("history:aliases.alias")}</th>
              <th>{t("history:aliases.canonical")}</th>
              <th>{t("history:imports.by")}</th>
              <th>{t("history:imports.at")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {aliases.data.map((a) => (
              <tr key={a.id}>
                <td>{label(a.alias_key)}</td>
                <td>
                  {label(a.canonical_key)}
                  {a.note && <span className="muted"> · {a.note}</span>}
                </td>
                <td>{a.created_by_name ?? "—"}</td>
                <td>{formatDateTime(a.created_at, i18n.language, active.timezone)}</td>
                <td className="row-actions">
                  {canManage && (
                    <ConfirmButton
                      disabled={action.busy}
                      label={t("history:aliases.remove")}
                      onConfirm={() => void remove(a.id)}
                    />
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <ErrorText code={action.error} />
    </div>
  );
}

type Row = Pick<
  HistoryRow,
  | "id"
  | "purchased_on"
  | "supplier_name"
  | "description"
  | "manufacturer"
  | "mpn"
  | "quantity_milli"
  | "unit"
  | "unit_price_milli"
  | "price_derived"
  | "currency"
> & { row?: number };

export function HistoryTable({ rows, showRow = false }: { rows: Row[]; showRow?: boolean }) {
  const { t, i18n } = useTranslation();
  const lang = i18n.language;
  return (
    <div className="table-scroll">
      <table className="table">
        <thead>
          <tr>
            {showRow && <th className="num">{t("history:preview.row")}</th>}
            <th>{t("history:field.date")}</th>
            <th>{t("history:field.supplier")}</th>
            <th>{t("history:field.description")}</th>
            <th>{t("history:field.mpn")}</th>
            <th className="num">{t("history:field.quantity")}</th>
            <th className="num">{t("history:field.unit_price")}</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.id}>
              {showRow && <td className="num">{r.row}</td>}
              <td>{formatDate(r.purchased_on, lang)}</td>
              <td>{r.supplier_name || "—"}</td>
              <td>
                {r.description}
                {r.manufacturer && <span className="muted"> · {r.manufacturer}</span>}
              </td>
              <td>{r.mpn || "—"}</td>
              <td className="num">
                {formatMilliQuantity(r.quantity_milli, lang)} {r.unit}
              </td>
              <td className="num" title={r.price_derived ? t("history:derived") : undefined}>
                {formatMilliPrice(r.unit_price_milli, r.currency, lang)}
                {r.price_derived && " *"}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
