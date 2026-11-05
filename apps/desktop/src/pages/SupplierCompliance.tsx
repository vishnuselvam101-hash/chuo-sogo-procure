import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import {
  DOC_KINDS,
  bpsLabel,
  type CustomerComplianceDoc,
  type StaffComplianceDoc,
  type SupplierScorecard,
} from "../lib/agents";
import { readAsBase64, ACCEPTED_FILES, MAX_UPLOAD_BYTES } from "../lib/files";
import { formatDate } from "../lib/format";
import { P } from "../lib/perm";
import type { MembershipView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { ConfirmButton, EmptyState, ErrorText, FileInput, Loading } from "../components/ui";

/** Customer members who work with suppliers or read the agents' findings (server: same list). */
export function seesSupplierWatch(m: MembershipView): boolean {
  return [P.viewAgents, P.approveSuppliers, P.reviewQuotations, P.manageRfqs].some((p) =>
    m.permissions.includes(p),
  );
}

function Expiry({ on }: { on: string | null | undefined }) {
  const { t, i18n } = useTranslation();
  return on ? (
    <>{formatDate(on, i18n.language)}</>
  ) : (
    <span className="muted">{t("agents:ui.supplier.noExpiry")}</span>
  );
}

/** A supplier's compliance documents as this customer may see them (no notes, no files). */
export function OrgSupplierDocs({ active, supplierId }: { active: MembershipView; supplierId: string }) {
  const { t } = useTranslation();
  const { data, error } = useApi<CustomerComplianceDoc[]>(
    `/api/v1/orgs/${active.org_id}/suppliers/${supplierId}/compliance-docs`,
  );
  return (
    <div className="sub-card stack" data-testid="org-docs">
      <h3>{t("agents:ui.supplier.docs")}</h3>
      <Loading data={data} error={error} />
      {data && data.length === 0 && <p className="muted small">{t("agents:ui.supplier.docsEmpty")}</p>}
      {data && data.length > 0 && (
        <table className="table">
          <thead>
            <tr>
              <th>{t("agents:ui.supplier.kind")}</th>
              <th>{t("agents:ui.supplier.title")}</th>
              <th>{t("agents:ui.supplier.reference")}</th>
              <th>{t("agents:ui.supplier.expires")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {data.map((d) => (
              <tr key={d.id}>
                <td>{t(`agents:supplier.docKind.${d.kind}`, { defaultValue: d.kind })}</td>
                <td>{d.title}</td>
                <td>{d.reference}</td>
                <td>
                  <Expiry on={d.expires_on} />
                </td>
                <td>
                  {d.verified ? (
                    <span className="badge badge-verified">{t("agents:ui.supplier.verified")}</span>
                  ) : (
                    <span className="badge badge-unverified">{t("agents:ui.supplier.unverified")}</span>
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

/** Previous month in Japan time as `YYYY-MM` (the latest complete month). */
function lastMonth(now = new Date()): string {
  const jst = new Date(now.getTime() + 9 * 3600_000);
  const y = jst.getUTCFullYear();
  // getUTCMonth() is 0-based, so it already is last month's 1-based number.
  const m = jst.getUTCMonth();
  return m === 0 ? `${y - 1}-12` : `${y}-${String(m).padStart(2, "0")}`;
}

/** QCD scorecards of this customer's suppliers for one month. */
export function Scorecards({ active }: { active: MembershipView }) {
  const { t, i18n } = useTranslation();
  const [month, setMonth] = useState(lastMonth());
  const { data, error } = useApi<SupplierScorecard[]>(
    `/api/v1/orgs/${active.org_id}/supplier-scorecards?month=${month}`,
  );
  const pct = (v: number | null | undefined) => bpsLabel(v) ?? t("agents:supplier.scorecard.noData");
  return (
    <div className="card stack" data-testid="scorecards">
      <div className="row">
        <h2 className="grow">{t("agents:ui.supplier.scorecards")}</h2>
        <label>
          {t("agents:ui.supplier.month")}
          <input type="month" value={month} onChange={(e) => e.target.value && setMonth(e.target.value)} />
        </label>
      </div>
      <Loading data={data} error={error} />
      {data && data.length === 0 && <EmptyState hint={t("agents:ui.supplier.scorecardsEmpty")} />}
      {data && data.length > 0 && (
        <div className="table-scroll">
          <table className="table">
            <thead>
              <tr>
                <th>{t("procurement:supplier.name")}</th>
                <th className="num">{t("agents:supplier.scorecard.onTime")}</th>
                <th className="num">{t("agents:supplier.scorecard.discrepancy")}</th>
                <th className="num">{t("agents:supplier.scorecard.replyRate")}</th>
                <th className="num">{t("agents:supplier.scorecard.medianReplyHours")}</th>
                <th className="num">{t("agents:supplier.scorecard.priceScore")}</th>
              </tr>
            </thead>
            <tbody>
              {data.map((c) => (
                <tr key={c.supplier_id}>
                  <td>{c.supplier_name}</td>
                  <td className="num">{pct(c.on_time_bps)}</td>
                  <td className="num">{pct(c.discrepancy_bps)}</td>
                  <td className="num">{pct(c.reply_rate_bps)}</td>
                  <td className="num">
                    {c.median_reply_hours === null || c.median_reply_hours === undefined
                      ? t("agents:supplier.scorecard.noData")
                      : new Intl.NumberFormat(i18n.language).format(c.median_reply_hours)}
                  </td>
                  <td className="num">{pct(c.price_score_bps)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

const EMPTY_DOC = {
  kind: "iso9001" as string,
  title: "",
  reference: "",
  issued_on: "",
  expires_on: "",
  customer_visible: false,
  notes: "",
};

/** Chuo-Sogo staff: keep a supplier's certificates and registrations, verify and retire them. */
export function StaffSupplierDocs({ supplierId }: { supplierId: string }) {
  const { t } = useTranslation();
  const base = `/api/v1/suppliers/${supplierId}/compliance-docs`;
  const { data, error, reload } = useApi<StaffComplianceDoc[]>(base);
  const action = useAction();
  const [form, setForm] = useState(EMPTY_DOC);
  const [file, setFile] = useState<File | null>(null);

  async function add(e: FormEvent) {
    e.preventDefault();
    if (file && file.size > MAX_UPLOAD_BYTES) return action.setError("validation_failed/file_size");
    const res = await action.run("POST", base, {
      ...form,
      issued_on: form.issued_on || null,
      expires_on: form.expires_on || null,
      file_name: file?.name ?? null,
      data_base64: file ? await readAsBase64(file) : null,
    });
    if (res) {
      setForm(EMPTY_DOC);
      setFile(null);
      reload();
    }
  }
  const update = async (d: StaffComplianceDoc, patch: Record<string, unknown>) => {
    const res = await action.run("PUT", `${base}/${d.id}`, {
      customer_visible: d.customer_visible,
      title: d.title,
      reference: d.reference,
      notes: d.notes,
      issued_on: d.issued_on ?? null,
      expires_on: d.expires_on ?? null,
      ...patch,
    });
    if (res) reload();
  };

  return (
    <div className="sub-card stack" data-testid="staff-docs">
      <h3>{t("agents:ui.supplier.docs")}</h3>
      <Loading data={data} error={error} />
      {data && data.length === 0 && <p className="muted small">{t("agents:ui.supplier.docsEmpty")}</p>}
      {data && data.length > 0 && (
        <table className="table">
          <thead>
            <tr>
              <th>{t("agents:ui.supplier.kind")}</th>
              <th>{t("agents:ui.supplier.title")}</th>
              <th>{t("agents:ui.supplier.reference")}</th>
              <th>{t("agents:ui.supplier.expires")}</th>
              <th>{t("agents:ui.supplier.customerVisible")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {data.map((d) => (
              <tr key={d.id} className={d.retired_at ? "muted" : undefined}>
                <td>{t(`agents:supplier.docKind.${d.kind}`, { defaultValue: d.kind })}</td>
                <td>
                  {d.title}
                  {d.notes && <div className="muted small">{d.notes}</div>}
                </td>
                <td>{d.reference}</td>
                <td>
                  <Expiry on={d.expires_on} />
                </td>
                <td>
                  <input
                    type="checkbox"
                    aria-label={t("agents:ui.supplier.customerVisible")}
                    checked={d.customer_visible}
                    disabled={action.busy || Boolean(d.retired_at)}
                    onChange={(e) => void update(d, { customer_visible: e.target.checked })}
                  />
                </td>
                <td className="row-actions">
                  {d.retired_at ? (
                    <span className="badge">{t("agents:ui.supplier.retired")}</span>
                  ) : (
                    <>
                      {d.verified_at ? (
                        <span className="badge badge-verified">{t("agents:ui.supplier.verified")}</span>
                      ) : (
                        <button
                          type="button"
                          disabled={action.busy}
                          onClick={() =>
                            void action.run("POST", `${base}/${d.id}/verify`).then((r) => r && reload())
                          }
                        >
                          {t("agents:ui.supplier.verify")}
                        </button>
                      )}
                      <ConfirmButton
                        label={t("agents:ui.supplier.retire")}
                        disabled={action.busy}
                        onConfirm={() => void update(d, { retired: true })}
                      />
                    </>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <form className="stack" onSubmit={(e) => void add(e)} aria-label={t("agents:ui.supplier.addDoc")}>
        <h4>{t("agents:ui.supplier.addDoc")}</h4>
        <div className="grid-2">
          <label>
            {t("agents:ui.supplier.kind")}
            <select value={form.kind} onChange={(e) => setForm({ ...form, kind: e.target.value })}>
              {DOC_KINDS.map((k) => (
                <option key={k} value={k}>
                  {t(`agents:supplier.docKind.${k}`)}
                </option>
              ))}
            </select>
          </label>
          <label>
            {t("agents:ui.supplier.title")}
            <input
              maxLength={200}
              value={form.title}
              onChange={(e) => setForm({ ...form, title: e.target.value })}
            />
          </label>
          <label>
            {t("agents:ui.supplier.reference")}
            <input
              maxLength={100}
              value={form.reference}
              onChange={(e) => setForm({ ...form, reference: e.target.value })}
            />
          </label>
          <label>
            {t("agents:ui.supplier.issued")}
            <input
              type="date"
              value={form.issued_on}
              onChange={(e) => setForm({ ...form, issued_on: e.target.value })}
            />
          </label>
          <label>
            {t("agents:ui.supplier.expires")}
            <input
              type="date"
              value={form.expires_on}
              onChange={(e) => setForm({ ...form, expires_on: e.target.value })}
            />
          </label>
          <label>
            {t("agents:ui.supplier.notes")}
            <input
              maxLength={2000}
              value={form.notes}
              onChange={(e) => setForm({ ...form, notes: e.target.value })}
            />
          </label>
          <label>
            {t("agents:ui.supplier.file")}
            <FileInput accept={ACCEPTED_FILES} onChange={(e) => setFile(e.target.files?.[0] ?? null)} />
          </label>
        </div>
        {form.kind === "qualified_invoice_registration" && (
          <p className="muted small">{t("agents:ui.supplier.registrationHint")}</p>
        )}
        <label className="radio">
          <input
            type="checkbox"
            checked={form.customer_visible}
            onChange={(e) => setForm({ ...form, customer_visible: e.target.checked })}
          />
          {t("agents:ui.supplier.customerVisible")}
        </label>
        <div className="row">
          <button type="submit" className="primary" disabled={action.busy}>
            {t("agents:ui.supplier.addDoc")}
          </button>
        </div>
      </form>
      <ErrorText code={action.error} />
    </div>
  );
}
