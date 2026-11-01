import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { useApi } from "../lib/useApi";
import { useAction } from "../lib/useAction";
import { formatDate } from "../lib/format";
import type { InvoiceSummary, IssuedInvoice, IssuerProfile } from "../lib/types";
import { ErrorText, Loading, Money, PageHeader } from "../components/ui";
import { DocNumber, DocStatus, InvoiceView, PdfButton } from "../billing/InvoiceView";
import { InvoiceEditor } from "./InvoiceEditor";
import { ConfirmButton } from "./widgets";
import { ADMIN_API, NOT_FOUND } from "./types";

const STATUSES = ["", "draft", "issued", "overdue", "paid"] as const;
const ROUNDINGS = ["floor", "half_up", "ceil"] as const;
const EDIT_DRAFT = { kind: "edit" } as const;
const NEW_CREDIT = { kind: "credit" } as const;
const NEW_INVOICE = { kind: "new" } as const;

type View =
  { kind: "list" } | { kind: "new" } | { kind: "issuer" } | { kind: "doc"; orgId: string; id: string };

/** Issuer details printed on every invoice (registration number etc.). */
function IssuerSettings({ onDone }: { onDone: () => void }) {
  const { t } = useTranslation();
  const path = `${ADMIN_API}/billing/issuer`;
  const { data, error } = useApi<IssuerProfile>(path);
  if (!data && error !== NOT_FOUND) return <Loading data={data} error={error} />;
  return (
    <IssuerForm
      path={path}
      initial={data}
      onDone={onDone}
      key={data ? "loaded" : "empty"}
      title={t("billing:issuer")}
    />
  );
}

function IssuerForm({
  path,
  initial,
  onDone,
  title,
}: {
  path: string;
  initial: IssuerProfile | null;
  onDone: () => void;
  title: string;
}) {
  const { t } = useTranslation();
  const { run, busy, error } = useAction();
  const [f, setF] = useState<IssuerProfile>(
    initial ?? {
      legal_name: "",
      registration_number: "",
      address: "",
      phone: "",
      email: "",
      bank_details: "",
      payment_terms_days: 30,
      tax_rounding: "floor",
      number_prefix: "CS",
    },
  );
  const set = (k: keyof IssuerProfile) => (e: { target: { value: string } }) =>
    setF((x) => ({
      ...x,
      [k]: k === "payment_terms_days" ? Number.parseInt(e.target.value, 10) || 0 : e.target.value,
    }));

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (await run("PUT", path, f)) onDone();
  }

  return (
    <form className="card form-grid" onSubmit={submit} data-testid="issuer-form">
      <h2>{title}</h2>
      <p className="muted small">{t("billing:issuerHint")}</p>
      <label>
        {t("billing:legalName")}
        <input required maxLength={200} value={f.legal_name} onChange={set("legal_name")} />
      </label>
      <label>
        {t("billing:registrationNumber")}
        <input
          required
          pattern="T[0-9]{13}"

          value={f.registration_number}
          onChange={set("registration_number")}
        />
      </label>
      <label>
        {t("billing:address")}
        <input required maxLength={500} value={f.address} onChange={set("address")} />
      </label>
      <label>
        {t("billing:phone")}
        <input maxLength={50} value={f.phone} onChange={set("phone")} />
      </label>
      <label>
        {t("billing:email")}
        <input maxLength={200} value={f.email} onChange={set("email")} />
      </label>
      <label>
        {t("billing:bankDetails")}
        <textarea maxLength={1000} value={f.bank_details} onChange={set("bank_details")} />
      </label>
      <label>
        {t("billing:paymentTerms")}
        <input
          type="number"
          min={0}
          max={365}
          required
          value={f.payment_terms_days}
          onChange={set("payment_terms_days")}
        />
      </label>
      <label>
        {t("billing:rounding")}
        <select value={f.tax_rounding} onChange={set("tax_rounding")}>
          {ROUNDINGS.map((r) => (
            <option key={r} value={r}>
              {t(`billing:roundings.${r}`)}
            </option>
          ))}
        </select>
      </label>
      <label>
        {t("billing:prefix")}
        <input required pattern="[A-Z0-9]{1,8}" value={f.number_prefix} onChange={set("number_prefix")} />
      </label>
      <ErrorText code={error} />
      <div className="row">
        <button type="submit" className="primary" disabled={busy}>
          {t("action.save")}
        </button>
        <button type="button" onClick={onDone}>
          {t("action.cancel")}
        </button>
      </div>
    </form>
  );
}

/** One document: view, then the actions its state allows. */
function DocPage({
  orgId,
  id,
  onOpen,
  onBack,
}: {
  orgId: string;
  id: string;
  onOpen: (orgId: string, id: string) => void;
  onBack: () => void;
}) {
  const { t, i18n } = useTranslation();
  const path = `${ADMIN_API}/orgs/${orgId}/billing/invoices/${id}`;
  const { data, error, reload } = useApi<IssuedInvoice>(path);
  const { run, busy, error: actionError } = useAction();
  const [editing, setEditing] = useState<"edit" | "credit" | null>(null);
  const [paidOn, setPaidOn] = useState("");
  const [paidNote, setPaidNote] = useState("");

  if (!data) return <Loading data={data} error={error} />;
  if (editing)
    return (
      <InvoiceEditor
        mode={editing === "edit" ? { ...EDIT_DRAFT, doc: data } : { ...NEW_CREDIT, original: data }}
        onCancel={() => setEditing(null)}
        onSaved={(o, newId) => {
          setEditing(null);
          if (newId === id) reload();
          else onOpen(o, newId);
        }}
      />
    );

  return (
    <section>
      <PageHeader title={data.number ?? t("billing:draftOf", { kind: t(`billing:kinds.${data.kind}`) })}>
        <button type="button" onClick={onBack}>
          {t("action.back")}
        </button>
      </PageHeader>
      <p className="muted">{data.org_name}</p>
      <InvoiceView doc={data} />
      <ErrorText code={actionError} />
      <div className="card stack" data-testid="invoice-actions">
        {data.status === "draft" && (
          <div className="row">
            <ConfirmButton
              label={t("billing:issue")}
              confirmLabel={t("billing:confirmIssue")}
              disabled={busy}
              onConfirm={async () => {
                if (await run("POST", `${path}/issue`, {})) reload();
              }}
            />
            {data.kind === "invoice" && (
              <button type="button" onClick={() => setEditing("edit")}>
                {t("billing:edit")}
              </button>
            )}
            <ConfirmButton
              label={t("billing:deleteDraft")}
              confirmLabel={t("billing:confirmDelete")}
              disabled={busy}
              onConfirm={async () => {
                if (await run("DELETE", path)) onBack();
              }}
            />
          </div>
        )}
        {data.status === "draft" && <p className="muted small">{t("billing:issueHint")}</p>}
        {data.status !== "draft" && (
          <div className="row">
            <PdfButton doc={data} />
            {data.kind === "invoice" && (
              <button type="button" onClick={() => setEditing("credit")}>
                {t("billing:newCreditNote")}
              </button>
            )}
          </div>
        )}
        {data.kind === "invoice" && data.status === "issued" && (
          <form
            className="row"
            onSubmit={async (e) => {
              e.preventDefault();
              if (await run("POST", `${path}/payment`, { paid: true, paid_on: paidOn, note: paidNote }))
                reload();
            }}
          >
            <label>
              {t("billing:paidOn")}
              <input type="date" required value={paidOn} onChange={(e) => setPaidOn(e.target.value)} />
            </label>
            <label>
              {t("billing:paymentNote")}
              <input maxLength={500} value={paidNote} onChange={(e) => setPaidNote(e.target.value)} />
            </label>
            <button type="submit" disabled={busy}>
              {t("billing:markPaid")}
            </button>
          </form>
        )}
        {data.status === "paid" && (
          <ConfirmButton
            label={t("billing:undoPayment")}
            confirmLabel={t("billing:confirmUndoPayment")}
            disabled={busy}
            onConfirm={async () => {
              if (await run("POST", `${path}/payment`, { paid: false })) reload();
            }}
          />
        )}
      </div>
      {data.credit_notes.length > 0 && (
        <>
          <h2>{t("billing:creditNotes")}</h2>
          <table className="table">
            <tbody>
              {data.credit_notes.map((c) => (
                <tr key={c.id}>
                  <td>
                    <button type="button" className="link" onClick={() => onOpen(c.org_id, c.id)}>
                      <DocNumber doc={c} />
                    </button>
                  </td>
                  <td>{c.issue_date ? formatDate(c.issue_date, i18n.language) : "—"}</td>
                  <td>
                    <DocStatus doc={c} />
                  </td>
                  <td className="num">
                    <Money minor={c.total_minor} currency={c.currency} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
      {data.original_id && (
        <button type="button" className="link" onClick={() => onOpen(data.org_id, data.original_id ?? "")}>
          {t("billing:openOriginal")}
        </button>
      )}
    </section>
  );
}

/** Admin console billing: Chuo-Sogo's own invoices and credit notes to customers. */
export function Billing() {
  const { t, i18n } = useTranslation();
  const [view, setView] = useState<View>({ kind: "list" });
  const [status, setStatus] = useState("");
  const { data, error, reload } = useApi<InvoiceSummary[]>(
    `${ADMIN_API}/billing/invoices${status ? `?status=${status}` : ""}`,
  );
  const back = () => {
    setView({ kind: "list" });
    reload();
  };
  const open = (orgId: string, id: string) => setView({ kind: "doc", orgId, id });

  if (view.kind === "doc")
    return <DocPage key={view.id} orgId={view.orgId} id={view.id} onOpen={open} onBack={back} />;
  if (view.kind === "issuer") return <IssuerSettings onDone={back} />;
  if (view.kind === "new") return <InvoiceEditor mode={NEW_INVOICE} onCancel={back} onSaved={open} />;

  return (
    <section>
      <PageHeader title={t("admin:nav.billing")}>
        <select
          aria-label={t("billing:filterStatus")}
          value={status}
          onChange={(e) => setStatus(e.target.value)}
        >
          {STATUSES.map((s) => (
            <option key={s} value={s}>
              {s ? t(`billing:status.${s}`) : t("billing:allStatuses")}
            </option>
          ))}
        </select>
        <button type="button" onClick={() => setView({ kind: "issuer" })}>
          {t("billing:issuer")}
        </button>
        <button type="button" className="primary" onClick={() => setView({ kind: "new" })}>
          {t("billing:newInvoice")}
        </button>
      </PageHeader>
      <Loading data={data} error={error} />
      {data && data.length === 0 && <p className="muted">{t("billing:none")}</p>}
      {data && data.length > 0 && (
        <table className="table" data-testid="billing-list">
          <thead>
            <tr>
              <th>{t("billing:number")}</th>
              <th>{t("billing:kind")}</th>
              <th>{t("billing:customer")}</th>
              <th>{t("billing:issueDate")}</th>
              <th>{t("billing:dueDate")}</th>
              <th className="num">{t("billing:total")}</th>
              <th>{t("billing:statusLabel")}</th>
            </tr>
          </thead>
          <tbody>
            {data.map((d) => (
              <tr key={d.id}>
                <td>
                  <button type="button" className="link" onClick={() => open(d.org_id, d.id)}>
                    <DocNumber doc={d} />
                  </button>
                </td>
                <td>{t(`billing:kinds.${d.kind}`)}</td>
                <td>{d.org_name}</td>
                <td>{d.issue_date ? formatDate(d.issue_date, i18n.language) : "—"}</td>
                <td>{d.due_date ? formatDate(d.due_date, i18n.language) : "—"}</td>
                <td className="num">
                  <Money minor={d.total_minor} currency={d.currency} />
                </td>
                <td>
                  <DocStatus doc={d} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
