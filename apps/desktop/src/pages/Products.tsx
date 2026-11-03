import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { P } from "../lib/perm";
import type { MembershipView, ProductDetail, ProductView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { EmptyState, ErrorText, Loading, PageHeader, StatusBadge } from "../components/ui";

export const EVIDENCE_KINDS = [
  "datasheet",
  "catalogue",
  "manufacturer_page",
  "supplier_confirmation",
  "other",
];
const PRODUCT_STATUSES = ["draft", "verified", "retired"];
const HTTPS_PREFIX = "https://";

/** "key=value" per line → specs object; returns null on a malformed line. */
export function parseSpecs(text: string): Record<string, string> | null {
  const specs: Record<string, string> = {};
  for (const line of text.split("\n")) {
    if (!line.trim()) continue;
    const i = line.indexOf("=");
    if (i <= 0) return null;
    specs[line.slice(0, i).trim()] = line.slice(i + 1).trim();
  }
  return specs;
}

/** Chuo-Sogo's product catalogue (staff): records, evidence and verification. */
export function Products({ active }: { active: MembershipView }) {
  const { t } = useTranslation();
  const [q, setQ] = useState("");
  const [status, setStatus] = useState("");
  const [open, setOpen] = useState<string | null>(null);
  const params = new URLSearchParams();
  if (q.trim()) params.set("q", q.trim());
  if (status) params.set("status", status);
  const query = params.toString();
  const { data, error, reload } = useApi<ProductView[]>(`/api/v1/products${query ? `?${query}` : ""}`);
  const canEdit = active.permissions.includes(P.manageProducts);

  return (
    <section>
      <PageHeader title={t("nav.products")}>
        <input
          type="search"
          aria-label={t("procurement:product.search")}
          placeholder={t("procurement:product.search")}
          value={q}
          onChange={(e) => setQ(e.target.value)}
        />
        <select
          aria-label={t("procurement:list.filter")}
          value={status}
          onChange={(e) => setStatus(e.target.value)}
        >
          <option value="">{t("procurement:list.all")}</option>
          {PRODUCT_STATUSES.map((s) => (
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
        <NewProduct
          onCreated={(id) => {
            setOpen(id);
            reload();
          }}
        />
      )}
      <Loading data={data} error={error} />
      {data && data.length === 0 && <EmptyState hint={t("empty.products")} />}
      {data && data.length > 0 && (
        <table className="table clickable">
          <thead>
            <tr>
              <th>{t("procurement:item.manufacturer")}</th>
              <th>{t("procurement:item.mpn")}</th>
              <th>{t("procurement:item.description")}</th>
              <th className="num">{t("procurement:product.evidenceCount")}</th>
              <th>{t("procurement:request.status")}</th>
            </tr>
          </thead>
          <tbody>
            {data.map((p) => (
              <tr key={p.id} onClick={() => setOpen(open === p.id ? null : p.id)}>
                <td>{p.manufacturer}</td>
                <td>
                  <button type="button" className="link">
                    {p.mpn}
                  </button>
                </td>
                <td>{p.description}</td>
                <td className="num">{p.evidence_count}</td>
                <td>
                  <StatusBadge status={p.status} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {open && <ProductPanel productId={open} canEdit={canEdit} onChanged={reload} />}
    </section>
  );
}

function NewProduct({ onCreated }: { onCreated: (id: string) => void }) {
  const { t } = useTranslation();
  const action = useAction();
  const empty = {
    manufacturer: "",
    mpn: "",
    description: "",
    category: "",
    specs: "",
    kind: "datasheet",
    url: "",
    note: "",
  };
  const [form, setForm] = useState(empty);
  const set = (k: keyof typeof empty) => (e: { target: { value: string } }) =>
    setForm({ ...form, [k]: e.target.value });

  async function submit(e: FormEvent) {
    e.preventDefault();
    const specs = parseSpecs(form.specs);
    if (!specs) return action.setError("validation_failed/specs");
    const evidence =
      form.url.trim() || form.note.trim()
        ? [{ kind: form.kind, url: form.url.trim() || null, note: form.note }]
        : [];
    const res = await action.run<{ id: string }>("POST", "/api/v1/products", {
      manufacturer: form.manufacturer,
      mpn: form.mpn,
      description: form.description,
      category: form.category,
      specs,
      evidence,
    });
    if (res) {
      setForm(empty);
      onCreated(res.body.id);
    }
  }

  return (
    <details className="card">
      <summary>{t("procurement:product.new")}</summary>
      <form className="stack" onSubmit={submit}>
        <div className="row">
          <label>
            {t("procurement:item.manufacturer")}
            <input required maxLength={200} value={form.manufacturer} onChange={set("manufacturer")} />
          </label>
          <label>
            {t("procurement:item.mpn")}
            <input required maxLength={100} value={form.mpn} onChange={set("mpn")} />
          </label>
          <label>
            {t("procurement:product.category")}
            <input maxLength={100} value={form.category} onChange={set("category")} />
          </label>
        </div>
        <label>
          {t("procurement:item.description")}
          <input maxLength={1000} value={form.description} onChange={set("description")} />
        </label>
        <label>
          {t("procurement:product.specs")}
          <textarea
            rows={3}
            placeholder={t("procurement:product.specsHint")}
            value={form.specs}
            onChange={set("specs")}
          />
        </label>
        <EvidenceFields value={form} onChange={(v) => setForm({ ...form, ...v })} />
        <div className="row">
          <button type="submit" className="primary" disabled={action.busy}>
            {t("procurement:product.create")}
          </button>
        </div>
        <ErrorText code={action.error} />
      </form>
    </details>
  );
}

interface EvidenceDraft {
  kind: string;
  url: string;
  note: string;
}

function EvidenceFields({ value, onChange }: { value: EvidenceDraft; onChange: (v: EvidenceDraft) => void }) {
  const { t } = useTranslation();
  return (
    <div className="row">
      <label>
        {t("procurement:product.evidenceKind")}
        <select value={value.kind} onChange={(e) => onChange({ ...value, kind: e.target.value })}>
          {EVIDENCE_KINDS.map((k) => (
            <option key={k} value={k}>
              {t(`procurement:evidence.${k}`)}
            </option>
          ))}
        </select>
      </label>
      <label className="grow">
        {t("procurement:product.evidenceUrl")}
        <input
          type="url"
          pattern="https://.+"
          placeholder={HTTPS_PREFIX}
          value={value.url}
          onChange={(e) => onChange({ ...value, url: e.target.value })}
        />
      </label>
      <label className="grow">
        {t("procurement:product.evidenceNote")}
        <input
          maxLength={2000}
          value={value.note}
          onChange={(e) => onChange({ ...value, note: e.target.value })}
        />
      </label>
    </div>
  );
}

/** One record with its evidence. Staff edit via the platform API; customers read via their org. */
export function ProductPanel({
  productId,
  canEdit,
  onChanged,
  orgId,
}: {
  productId: string;
  canEdit: boolean;
  onChanged?: () => void;
  orgId?: string;
}) {
  const { t } = useTranslation();
  const path = orgId ? `/api/v1/orgs/${orgId}/products/${productId}` : `/api/v1/products/${productId}`;
  const { data, error, reload } = useApi<ProductDetail>(path);
  const action = useAction();
  const [evidence, setEvidence] = useState<EvidenceDraft>({ kind: "datasheet", url: "", note: "" });
  if (!data) return <Loading data={data} error={error} />;
  const p = data.product;
  const refresh = () => {
    reload();
    onChanged?.();
  };

  async function addEvidence() {
    const res = await action.run("POST", `/api/v1/products/${productId}/evidence`, {
      kind: evidence.kind,
      url: evidence.url.trim() || null,
      note: evidence.note,
    });
    if (res) {
      setEvidence({ kind: "datasheet", url: "", note: "" });
      refresh();
    }
  }
  async function setStatus(status: string) {
    if (await action.run("POST", `/api/v1/products/${productId}/status`, { status, note: "" })) refresh();
  }

  return (
    <div className="card stack">
      <div className="row">
        <strong>{`${p.manufacturer} ${p.mpn}`}</strong>
        <StatusBadge status={p.status} />
      </div>
      {p.description && <p className="pre">{p.description}</p>}
      {Object.keys(p.specs).length > 0 && (
        <table className="table">
          <tbody>
            {Object.entries(p.specs).map(([k, v]) => (
              <tr key={k}>
                <th>{k}</th>
                <td>{v}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <h3>{t("procurement:product.evidence")}</h3>
      {canEdit && <p className="muted small">{t("procurement:product.visibility")}</p>}
      {data.evidence.length === 0 && <p className="muted">{t("procurement:product.noEvidence")}</p>}
      <ul className="plain">
        {data.evidence.map((e) => (
          <li key={e.id}>
            <span className="badge">{t(`procurement:evidence.${e.kind}`)}</span> {e.url ?? ""}{" "}
            {e.note && <span className="muted">{e.note}</span>}
          </li>
        ))}
      </ul>
      {canEdit && (
        <>
          <EvidenceFields value={evidence} onChange={setEvidence} />
          <div className="row">
            <button
              type="button"
              disabled={action.busy || (!evidence.url.trim() && !evidence.note.trim())}
              onClick={() => void addEvidence()}
            >
              {t("procurement:product.addEvidence")}
            </button>
            {p.status !== "verified" && (
              <button type="button" className="primary" onClick={() => void setStatus("verified")}>
                {t("procurement:product.markVerified")}
              </button>
            )}
            {p.status !== "retired" && (
              <button type="button" onClick={() => void setStatus("retired")}>
                {t("procurement:product.retire")}
              </button>
            )}
          </div>
        </>
      )}
      <ErrorText code={action.error} />
    </div>
  );
}
