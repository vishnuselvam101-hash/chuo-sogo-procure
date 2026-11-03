import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import type { MembershipView, OrgSupplierView, SupplierView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { EmptyState, ErrorText, Loading, PageHeader, StatusBadge } from "../components/ui";
import { P } from "../lib/perm";

const SUPPLIER_STATUSES = ["discovered", "contacted", "under_review", "verified", "inactive"];

/**
 * Platform org: Chuo-Sogo's supplier directory (staff only).
 * Customer org: suppliers proposed to this customer and its decisions; assigned staff propose.
 */
/** How quote requests can go to a supplier (`suppliers.rfq_channel`). */
const RFQ_CHANNELS = ["email", "fax"] as const;

export function Suppliers({ active }: { active: MembershipView }) {
  return active.org_kind === "platform" ? <Directory active={active} /> : <OrgSuppliers active={active} />;
}

function Directory({ active }: { active: MembershipView }) {
  const { t } = useTranslation();
  const [filter, setFilter] = useState("");
  const { data, error, reload } = useApi<SupplierView[]>(
    `/api/v1/suppliers${filter ? `?status=${filter}` : ""}`,
  );
  const action = useAction();
  const canEdit = active.permissions.includes(P.manageSuppliers);
  const [form, setForm] = useState({
    name: "",
    country: "JP",
    email: "",
    website: "",
    categories: "",
    notes: "",
    fax: "",
    rfq_channel: RFQ_CHANNELS[0] as string,
  });

  async function add(e: FormEvent) {
    e.preventDefault();
    const res = await action.run("POST", "/api/v1/suppliers", {
      ...form,
      email: form.email || null,
      website: form.website || null,
      fax: form.fax || null,
      categories: form.categories
        .split(",")
        .map((c) => c.trim())
        .filter(Boolean),
    });
    if (res) {
      setForm({
        name: "",
        country: "JP",
        email: "",
        website: "",
        categories: "",
        notes: "",
        fax: "",
        rfq_channel: RFQ_CHANNELS[0],
      });
      reload();
    }
  }

  async function setStatus(id: string, status: string) {
    if (await action.run("POST", `/api/v1/suppliers/${id}/status`, { status, note: "" })) reload();
  }

  return (
    <section>
      <PageHeader title={t("nav.suppliers")}>
        <select
          aria-label={t("procurement:list.filter")}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        >
          <option value="">{t("procurement:list.all")}</option>
          {SUPPLIER_STATUSES.map((s) => (
            <option key={s} value={s}>
              {t(`status.${s}`)}
            </option>
          ))}
        </select>
        <button type="button" onClick={reload}>
          {t("action.refresh")}
        </button>
      </PageHeader>
      {canEdit && (
        <form className="card inline-form" onSubmit={add}>
          <label>
            {t("procurement:supplier.name")}
            <input
              required
              maxLength={200}
              value={form.name}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
            />
          </label>
          <label className="narrow">
            {t("procurement:supplier.country")}
            <input
              required
              pattern="[A-Za-z]{2}"
              value={form.country}
              onChange={(e) => setForm({ ...form, country: e.target.value })}
            />
          </label>
          <label>
            {t("auth.email")}
            <input
              type="email"
              value={form.email}
              onChange={(e) => setForm({ ...form, email: e.target.value })}
            />
          </label>
          <label>
            {t("procurement:supplier.fax")}
            <input
              inputMode="tel"
              value={form.fax}
              onChange={(e) => setForm({ ...form, fax: e.target.value })}
            />
          </label>
          <label>
            {t("procurement:supplier.rfqChannel")}
            <select
              value={form.rfq_channel}
              onChange={(e) => setForm({ ...form, rfq_channel: e.target.value })}
            >
              {RFQ_CHANNELS.map((c) => (
                <option key={c} value={c}>
                  {t(`procurement:mail.channel.${c}`)}
                </option>
              ))}
            </select>
          </label>
          <label>
            {t("procurement:supplier.website")}
            <input
              type="url"
              value={form.website}
              onChange={(e) => setForm({ ...form, website: e.target.value })}
            />
          </label>
          <label>
            {t("procurement:supplier.categories")}
            <input
              value={form.categories}
              onChange={(e) => setForm({ ...form, categories: e.target.value })}
            />
          </label>
          <label>
            {t("procurement:supplier.notes")}
            <input
              maxLength={4000}
              value={form.notes}
              onChange={(e) => setForm({ ...form, notes: e.target.value })}
            />
          </label>
          <button type="submit" className="primary" disabled={action.busy}>
            {t("procurement:supplier.add")}
          </button>
        </form>
      )}
      <ErrorText code={action.error} />
      <Loading data={data} error={error} />
      {data && data.length === 0 && <EmptyState hint={t("empty.suppliers")} />}
      {data && data.length > 0 && (
        <table className="table">
          <thead>
            <tr>
              <th>{t("procurement:supplier.name")}</th>
              <th>{t("procurement:supplier.country")}</th>
              <th>{t("procurement:supplier.categories")}</th>
              <th>{t("procurement:supplier.notes")}</th>
              <th>{t("procurement:request.status")}</th>
            </tr>
          </thead>
          <tbody>
            {data.map((s) => (
              <tr key={s.id}>
                <td>
                  {s.name}
                  {s.email && <div className="muted small">{s.email}</div>}
                  {s.fax && (
                    <div className="muted small">
                      {t("procurement:supplier.faxShort", { fax: s.fax })}
                      {s.rfq_channel === RFQ_CHANNELS[1] ? ` · ${t("procurement:supplier.byFax")}` : ""}
                    </div>
                  )}
                </td>
                <td>{s.country}</td>
                <td>{s.categories.join(", ")}</td>
                <td className="small">{s.notes}</td>
                <td>
                  {canEdit ? (
                    <select
                      aria-label={t("procurement:request.status")}
                      value={s.status}
                      onChange={(e) => void setStatus(s.id, e.target.value)}
                    >
                      {SUPPLIER_STATUSES.map((st) => (
                        <option key={st} value={st}>
                          {t(`status.${st}`)}
                        </option>
                      ))}
                    </select>
                  ) : (
                    <StatusBadge status={s.status} />
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

function OrgSuppliers({ active }: { active: MembershipView }) {
  const { t } = useTranslation();
  const base = `/api/v1/orgs/${active.org_id}/suppliers`;
  const { data, error, reload } = useApi<OrgSupplierView[]>(base);
  const action = useAction();
  const [reason, setReason] = useState("");
  const canDecide = active.permissions.includes(P.approveSuppliers);
  const canPropose = active.access === "assigned" && active.permissions.includes(P.manageRfqs);

  async function decide(id: string, decision: "approved" | "rejected") {
    if (await action.run("PUT", `${base}/${id}/decision`, { decision, reason })) {
      setReason("");
      reload();
    }
  }

  return (
    <section>
      <PageHeader title={t("nav.suppliers")}>
        <button type="button" onClick={reload}>
          {t("action.refresh")}
        </button>
      </PageHeader>
      {canPropose && <Propose base={base} onDone={reload} known={(data ?? []).map((s) => s.supplier_id)} />}
      {canDecide && (
        <label className="card">
          {t("procurement:supplier.reason")}
          <input maxLength={2000} value={reason} onChange={(e) => setReason(e.target.value)} />
        </label>
      )}
      <ErrorText code={action.error} />
      <Loading data={data} error={error} />
      {data && data.length === 0 && <EmptyState />}
      {data && data.length > 0 && (
        <table className="table">
          <thead>
            <tr>
              <th>{t("procurement:supplier.name")}</th>
              <th>{t("procurement:supplier.country")}</th>
              <th>{t("procurement:supplier.categories")}</th>
              <th>{t("procurement:supplier.decision")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {data.map((s) => (
              <tr key={s.supplier_id}>
                <td>
                  {s.name}
                  {s.website && <div className="muted small">{s.website}</div>}
                </td>
                <td>{s.country}</td>
                <td>{s.categories.join(", ")}</td>
                <td>
                  <StatusBadge status={s.decision} />
                  {s.reason && <div className="muted small">{s.reason}</div>}
                </td>
                <td className="row-actions">
                  {canDecide && s.decision !== "approved" && (
                    <button
                      type="button"
                      className="primary"
                      onClick={() => void decide(s.supplier_id, "approved")}
                    >
                      {t("procurement:supplier.approve")}
                    </button>
                  )}
                  {canDecide && s.decision !== "rejected" && (
                    <button
                      type="button"
                      disabled={!reason.trim()}
                      title={t("procurement:supplier.reasonRequired")}
                      onClick={() => void decide(s.supplier_id, "rejected")}
                    >
                      {t("procurement:supplier.reject")}
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

function Propose({ base, known, onDone }: { base: string; known: string[]; onDone: () => void }) {
  const { t } = useTranslation();
  const { data } = useApi<SupplierView[]>("/api/v1/suppliers?status=verified");
  const action = useAction();
  const [chosen, setChosen] = useState("");
  const candidates = (data ?? []).filter((s) => !known.includes(s.id));
  if (candidates.length === 0) return null;
  return (
    <div className="card inline-form">
      <label>
        {t("procurement:supplier.propose")}
        <select value={chosen} onChange={(e) => setChosen(e.target.value)}>
          <option value="">—</option>
          {candidates.map((s) => (
            <option key={s.id} value={s.id}>
              {`${s.name} (${s.country})`}
            </option>
          ))}
        </select>
      </label>
      <button
        type="button"
        className="primary"
        disabled={!chosen || action.busy}
        onClick={() =>
          void action.run("POST", `${base}/${chosen}/proposal`).then((r) => {
            if (r) {
              setChosen("");
              onDone();
            }
          })
        }
      >
        {t("procurement:supplier.proposeButton")}
      </button>
      <ErrorText code={action.error} />
    </div>
  );
}
