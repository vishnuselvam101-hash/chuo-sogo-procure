import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import {
  IMPORT_KINDS,
  type AgentException,
  type ImportBatchView,
  type ProfileConfig,
  type ProfileView,
} from "../lib/agents";
import { readAsBase64, MAX_UPLOAD_BYTES } from "../lib/files";
import { bpsToPercent, formatDate, formatDateTime, percentToBps } from "../lib/format";
import { P } from "../lib/perm";
import type { MembershipView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { ConfirmButton, EmptyState, ErrorText, FileInput, Loading, PageHeader } from "../components/ui";
import { ExceptionDetail } from "./AgentExceptions";

const TABS = ["imports", "profiles"] as const;
type Tab = (typeof TABS)[number];
const PREVIEWED = "previewed";
const IMPORT_FILES = ".csv,.tsv,.txt,.xlsx";
/** Field codes a column can be mapped to (server: `ColumnMap.field`). */
export const FIELDS = [
  "po_number",
  "request_number",
  "order_date",
  "received_date",
  "invoice_number",
  "invoice_date",
  "due_date",
  "line_no",
  "mpn",
  "description",
  "quantity",
  "unit_price",
  "line_amount",
  "currency",
  "tax_rate",
  "tax_amount",
  "shipping",
  "note",
] as const;
const ENCODINGS = ["auto", "utf-8", "shift_jis"] as const;
const DATE_FORMATS = ["auto", "ymd", "dmy", "mdy"] as const;
const DECIMAL_SEPARATORS = [".", ","] as const;
const THOUSANDS_SEPARATORS = [",", ".", " ", "'", ""] as const;

/** The import kinds this member may bring in (the server checks each upload again). */
export function importKindsFor(m: MembershipView): string[] {
  const has = (p: string) => m.permissions.includes(p);
  return IMPORT_KINDS.filter(
    (k) =>
      (k === "po" && has(P.recordOrders)) ||
      (k === "receipt" && has(P.receiveGoods)) ||
      (k === "invoice" && has(P.manageInvoices)),
  );
}

/** L7: bring in POs, receipts and supplier invoices from another system's files. */
export function Integrations({ active }: { active: MembershipView }) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<Tab>("imports");
  return (
    <section>
      <PageHeader title={t("nav.integrations")} />
      <p className="lead">{t("agents:ui.import.lead")}</p>
      <div className="tabs" role="tablist">
        {TABS.map((id) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={tab === id}
            className={`tab${tab === id ? " active" : ""}`}
            onClick={() => setTab(id)}
          >
            {t(`agents:ui.import.tab.${id}`)}
          </button>
        ))}
      </div>
      {tab === "imports" ? <Imports active={active} /> : <Profiles active={active} />}
    </section>
  );
}

function Imports({ active }: { active: MembershipView }) {
  const { t, i18n } = useTranslation();
  const base = `/api/v1/orgs/${active.org_id}/integrations`;
  const kinds = importKindsFor(active);
  const list = useApi<ImportBatchView[]>(`${base}/imports?limit=50`);
  const profiles = useApi<ProfileView[]>(`${base}/profiles`);
  const upload = useAction();
  const [kind, setKind] = useState(kinds[0] ?? "po");
  const [profile, setProfile] = useState("");
  const [file, setFile] = useState<File | null>(null);
  const [open, setOpen] = useState<string | null>(null);

  async function send(e: FormEvent) {
    e.preventDefault();
    if (!file) return;
    if (file.size === 0 || file.size > MAX_UPLOAD_BYTES)
      return upload.setError("validation_failed/file_size");
    const res = await upload.run<ImportBatchView>("POST", `${base}/imports`, {
      kind,
      file_name: file.name,
      data_base64: await readAsBase64(file),
      profile: profile || null,
    });
    if (res) {
      setFile(null);
      setOpen(res.body.id);
      list.reload();
    }
  }

  if (open)
    return (
      <BatchDetail
        key={open}
        active={active}
        id={open}
        onBack={() => {
          setOpen(null);
          list.reload();
        }}
      />
    );

  return (
    <div className="stack">
      {kinds.length > 0 && (
        <form className="card stack" onSubmit={(e) => void send(e)} aria-label={t("agents:ui.import.upload")}>
          <h2>{t("agents:ui.import.upload")}</h2>
          <div className="row">
            <label>
              {t("agents:ui.import.kind")}
              <select value={kind} onChange={(e) => setKind(e.target.value)}>
                {kinds.map((k) => (
                  <option key={k} value={k}>
                    {t(`agents:integrations.kind.${k}`)}
                  </option>
                ))}
              </select>
            </label>
            <label>
              {t("agents:ui.import.profile")}
              <select value={profile} onChange={(e) => setProfile(e.target.value)}>
                <option value="">{t("agents:ui.import.autoDetect")}</option>
                {(profiles.data ?? [])
                  .filter((p) => p.config.kind === kind)
                  .map((p) => (
                    <option key={p.name} value={p.name}>
                      {p.name}
                    </option>
                  ))}
              </select>
            </label>
            <label className="grow">
              {t("agents:ui.import.file")}
              <FileInput accept={IMPORT_FILES} onChange={(e) => setFile(e.target.files?.[0] ?? null)} />
            </label>
          </div>
          <p className="muted small">{t("agents:ui.import.fileHint")}</p>
          <div className="row">
            <button type="submit" className="primary" disabled={!file || upload.busy}>
              {t("agents:ui.import.preview")}
            </button>
          </div>
          <ErrorText code={upload.error} />
        </form>
      )}
      <h2>{t("agents:ui.import.batches")}</h2>
      <Loading data={list.data} error={list.error} />
      {list.data && list.data.length === 0 && <EmptyState hint={t("agents:ui.import.empty")} />}
      {list.data && list.data.length > 0 && (
        <table className="table" data-testid="batches">
          <thead>
            <tr>
              <th>{t("agents:ui.import.file")}</th>
              <th>{t("agents:ui.import.kind")}</th>
              <th className="num">{t("agents:ui.import.records")}</th>
              <th className="num">{t("agents:ui.import.errorRows")}</th>
              <th>{t("agents:ui.col.status")}</th>
              <th>{t("agents:ui.col.created")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {list.data.map((b) => (
              <tr key={b.id}>
                <td>{b.file_name}</td>
                <td>{t(`agents:integrations.kind.${b.kind}`, { defaultValue: b.kind })}</td>
                <td className="num">{b.records}</td>
                <td className="num">{b.error_rows}</td>
                <td>
                  <span className={`badge badge-batch-${b.status}`}>
                    {t(`agents:integrations.batchStatus.${b.status}`, { defaultValue: b.status })}
                  </span>
                </td>
                <td className="small">{formatDateTime(b.created_at, i18n.language, active.timezone)}</td>
                <td className="row-actions">
                  <button type="button" onClick={() => setOpen(b.id)}>
                    {t("agents:ui.inbox.open")}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

function BatchDetail({ active, id, onBack }: { active: MembershipView; id: string; onBack: () => void }) {
  const { t, i18n } = useTranslation();
  const org = `/api/v1/orgs/${active.org_id}`;
  const path = `${org}/integrations/imports/${id}`;
  const { data, error, reload } = useApi<ImportBatchView>(path);
  const exceptions = useApi<AgentException[]>(`${org}/agents/exceptions?agent=import&status=open`);
  const action = useAction();
  const canDecide = active.permissions.includes(P.decideAgentExceptions);
  if (!data) return <Loading data={data} error={error} />;
  const ex = exceptions.data?.find((e) => e.subject_id === data.id) ?? null;
  const refresh = () => {
    reload();
    exceptions.reload();
  };

  return (
    <div className="stack" data-testid="batch">
      <div className="row">
        <h2 className="grow">{data.file_name}</h2>
        <span className={`badge badge-batch-${data.status}`}>
          {t(`agents:integrations.batchStatus.${data.status}`, { defaultValue: data.status })}
        </span>
        <button type="button" onClick={onBack}>
          {t("action.back")}
        </button>
      </div>
      <p className="small">
        {t(`agents:integrations.kind.${data.kind}`, { defaultValue: data.kind })} ·{" "}
        {t("agents:ui.import.summary", {
          rows: data.data_rows,
          records: data.records,
          errors: data.error_rows,
        })}
        {data.failure_code &&
          ` · ${t(`agents:integrations.failure.${data.failure_code}`, { defaultValue: data.failure_code })}`}
      </p>
      {data.errors.length > 0 && (
        <>
          <h3>{t("agents:ui.import.rowErrors")}</h3>
          <table className="table" data-testid="row-errors">
            <thead>
              <tr>
                <th className="num">{t("agents:ui.import.row")}</th>
                <th>{t("agents:ui.import.field")}</th>
                <th>{t("agents:ui.import.problem")}</th>
              </tr>
            </thead>
            <tbody>
              {data.errors.map((e, i) => (
                <tr key={i}>
                  <td className="num">{e.row === 0 ? t("agents:ui.import.wholeFile") : e.row}</td>
                  <td>
                    {e.field ? t(`agents:integrations.field.${e.field}`, { defaultValue: e.field }) : "—"}
                  </td>
                  <td>{t(`agents:integrations.rowError.${e.code}`, { defaultValue: e.code })}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
      {data.staged.length > 0 && (
        <>
          <h3>{t("agents:ui.import.staged")}</h3>
          <div className="table-scroll">
            <table className="table" data-testid="staged">
              <thead>
                <tr>
                  <th>{t("agents:integrations.field.po_number")}</th>
                  <th>{t("agents:ui.import.reference")}</th>
                  <th>{t("agents:ui.import.date")}</th>
                  <th className="num">{t("agents:ui.import.lines")}</th>
                  <th className="num">{t("agents:ui.import.row")}</th>
                </tr>
              </thead>
              <tbody>
                {data.staged.map((s, i) => (
                  <tr key={i}>
                    <td>{s.po_number}</td>
                    <td>{s.invoice_number || s.request_number || "—"}</td>
                    <td>{s.date ? formatDate(s.date, i18n.language) : "—"}</td>
                    <td className="num">{s.lines.length}</td>
                    <td className="num">{s.rows.join(", ")}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
      {data.status === PREVIEWED && ex && (
        <ExceptionDetail
          ex={ex}
          orgId={active.org_id}
          timezone={active.timezone}
          canDecide={canDecide}
          onDecided={refresh}
          onClose={onBack}
        />
      )}
      {data.status === PREVIEWED && !ex && exceptions.data && (
        <p className="muted small">{t("agents:ui.import.waitingForAgent")}</p>
      )}
      {data.status === PREVIEWED && (
        <div className="row">
          <ConfirmButton
            label={t("agents:ui.import.discard")}
            disabled={action.busy}
            onConfirm={() => void action.run("POST", `${path}/discard`).then((r) => r && refresh())}
          />
        </div>
      )}
      <ErrorText code={action.error} />
    </div>
  );
}

interface ProfileForm {
  name: string;
  kind: string;
  encoding: string;
  date_format: string;
  decimal_separator: string;
  thousands_separator: string;
  tax: string;
  columns: { source: string; field: string }[];
}

function toForm(p: ProfileView | null, kind: string): ProfileForm {
  const c = p?.config;
  return {
    name: p?.name ?? "",
    kind: c?.kind ?? kind,
    encoding: c?.encoding ?? "auto",
    date_format: c?.date_format ?? "auto",
    decimal_separator: c?.decimal_separator ?? ".",
    thousands_separator: c?.thousands_separator ?? ",",
    tax: bpsToPercent(c?.default_tax_rate_bps ?? 1000),
    columns: c?.columns?.length ? c.columns.map((m) => ({ ...m })) : [{ source: "", field: "po_number" }],
  };
}

function Profiles({ active }: { active: MembershipView }) {
  const { t, i18n } = useTranslation();
  const base = `/api/v1/orgs/${active.org_id}/integrations/profiles`;
  const { data, error, reload } = useApi<ProfileView[]>(base);
  const action = useAction();
  const kinds = importKindsFor(active);
  const [form, setForm] = useState<ProfileForm | null>(null);

  async function save(e: FormEvent) {
    e.preventDefault();
    if (!form) return;
    const tax = percentToBps(form.tax);
    if (tax === null) return action.setError("validation_failed/default_tax_rate_bps");
    const body: ProfileConfig = {
      kind: form.kind,
      encoding: form.encoding,
      date_format: form.date_format,
      decimal_separator: form.decimal_separator,
      thousands_separator: form.thousands_separator,
      default_tax_rate_bps: tax,
      columns: form.columns
        .filter((c) => c.source.trim())
        .map((c) => ({ source: c.source.trim(), field: c.field })),
    };
    if (await action.run("PUT", `${base}/${encodeURIComponent(form.name.trim())}`, body)) {
      setForm(null);
      reload();
    }
  }
  async function remove(name: string) {
    if (await action.run("DELETE", `${base}/${encodeURIComponent(name)}`)) reload();
  }
  const setCol = (i: number, patch: Partial<{ source: string; field: string }>) =>
    form && setForm({ ...form, columns: form.columns.map((c, j) => (j === i ? { ...c, ...patch } : c)) });

  return (
    <div className="stack">
      <p className="muted small">{t("agents:ui.import.profilesLead")}</p>
      {kinds.length > 0 && !form && (
        <div className="row">
          <button type="button" className="primary" onClick={() => setForm(toForm(null, kinds[0] ?? "po"))}>
            {t("agents:ui.import.newProfile")}
          </button>
        </div>
      )}
      {form && (
        <form
          className="card stack"
          onSubmit={(e) => void save(e)}
          aria-label={t("agents:ui.import.profile")}
        >
          <div className="grid-2">
            <label>
              {t("agents:ui.import.profileName")}
              <input
                maxLength={60}
                value={form.name}
                onChange={(e) => setForm({ ...form, name: e.target.value })}
              />
            </label>
            <label>
              {t("agents:ui.import.kind")}
              <select value={form.kind} onChange={(e) => setForm({ ...form, kind: e.target.value })}>
                {kinds.map((k) => (
                  <option key={k} value={k}>
                    {t(`agents:integrations.kind.${k}`)}
                  </option>
                ))}
              </select>
            </label>
            <label>
              {t("agents:ui.import.encoding")}
              <select value={form.encoding} onChange={(e) => setForm({ ...form, encoding: e.target.value })}>
                {ENCODINGS.map((v) => (
                  <option key={v} value={v}>
                    {t(`agents:ui.import.encodingOption.${v.replace("-", "_")}`)}
                  </option>
                ))}
              </select>
            </label>
            <label>
              {t("agents:ui.import.dateFormat")}
              <select
                value={form.date_format}
                onChange={(e) => setForm({ ...form, date_format: e.target.value })}
              >
                {DATE_FORMATS.map((v) => (
                  <option key={v} value={v}>
                    {t(`agents:ui.import.dateOption.${v}`)}
                  </option>
                ))}
              </select>
            </label>
            <label>
              {t("agents:ui.import.decimalSeparator")}
              <select
                value={form.decimal_separator}
                onChange={(e) => setForm({ ...form, decimal_separator: e.target.value })}
              >
                {DECIMAL_SEPARATORS.map((v) => (
                  <option key={v} value={v}>
                    {v}
                  </option>
                ))}
              </select>
            </label>
            <label>
              {t("agents:ui.import.thousandsSeparator")}
              <select
                value={form.thousands_separator}
                onChange={(e) => setForm({ ...form, thousands_separator: e.target.value })}
              >
                {THOUSANDS_SEPARATORS.map((v) => (
                  <option key={v} value={v}>
                    {v === "" ? t("agents:ui.import.none") : v === " " ? t("agents:ui.import.space") : v}
                  </option>
                ))}
              </select>
            </label>
            <label>
              {t("agents:ui.import.defaultTax")}
              <input
                inputMode="decimal"
                value={form.tax}
                onChange={(e) => setForm({ ...form, tax: e.target.value })}
              />
            </label>
          </div>
          <h3>{t("agents:ui.import.columns")}</h3>
          {form.columns.map((c, i) => (
            <div key={i} className="row">
              <label className="grow">
                {t("agents:ui.import.sourceColumn")}
                <input
                  maxLength={200}
                  value={c.source}
                  onChange={(e) => setCol(i, { source: e.target.value })}
                />
              </label>
              <label>
                {t("agents:ui.import.field")}
                <select value={c.field} onChange={(e) => setCol(i, { field: e.target.value })}>
                  {FIELDS.map((f) => (
                    <option key={f} value={f}>
                      {t(`agents:integrations.field.${f}`)}
                    </option>
                  ))}
                </select>
              </label>
            </div>
          ))}
          <div className="row">
            <button
              type="button"
              onClick={() =>
                setForm({ ...form, columns: [...form.columns, { source: "", field: "description" }] })
              }
            >
              {t("agents:ui.import.addColumn")}
            </button>
          </div>
          <div className="row">
            <button type="submit" className="primary" disabled={action.busy || !form.name.trim()}>
              {t("action.save")}
            </button>
            <button type="button" onClick={() => setForm(null)}>
              {t("action.cancel")}
            </button>
          </div>
        </form>
      )}
      <ErrorText code={action.error} />
      <Loading data={data} error={error} />
      {data && data.length > 0 && (
        <table className="table" data-testid="profiles">
          <thead>
            <tr>
              <th>{t("agents:ui.import.profileName")}</th>
              <th>{t("agents:ui.import.kind")}</th>
              <th>{t("agents:ui.col.updated")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {data.map((p) => (
              <tr key={p.name}>
                <td>
                  {p.name}
                  {p.builtin && <span className="badge">{t("agents:ui.import.builtin")}</span>}
                </td>
                <td>{t(`agents:integrations.kind.${p.config.kind}`, { defaultValue: p.config.kind })}</td>
                <td className="small">
                  {p.updated_at ? formatDateTime(p.updated_at, i18n.language, active.timezone) : "—"}
                </td>
                <td className="row-actions">
                  {!p.builtin && kinds.includes(p.config.kind) && (
                    <>
                      <button type="button" onClick={() => setForm(toForm(p, p.config.kind))}>
                        {t("agents:ui.import.edit")}
                      </button>
                      <ConfirmButton label={t("action.remove")} onConfirm={() => void remove(p.name)} />
                    </>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
