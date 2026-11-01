import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { DEFAULT_LANGUAGE, LANGUAGES } from "../i18n";
import { useApi } from "../lib/useApi";
import { useAction } from "../lib/useAction";
import { ErrorText, Loading, PageHeader, StatusBadge } from "../components/ui";
import { ADMIN_API, type CreateOrgRequest, type OrgOverview } from "./types";

const DEFAULT_TIMEZONE = "Asia/Tokyo";
const CUSTOMER_KIND = "customer";
const KINDS = ["customer", "supplier"] as const;

function CreateOrg({ onCreated }: { onCreated: (orgId: string) => void }) {
  const { t } = useTranslation();
  const { run, busy, error } = useAction();
  const [form, setForm] = useState<CreateOrgRequest>({
    name: "",
    kind: CUSTOMER_KIND,
    admin_email: "",
    admin_name: "",
    default_locale: DEFAULT_LANGUAGE,
    timezone: DEFAULT_TIMEZONE,
  });
  const set = (k: keyof CreateOrgRequest) => (e: { target: { value: string } }) =>
    setForm((f) => ({ ...f, [k]: e.target.value }));

  async function submit(e: FormEvent) {
    e.preventDefault();
    const res = await run<{ org_id: string }>("POST", `${ADMIN_API}/orgs`, form);
    if (res) onCreated(res.body.org_id);
  }

  return (
    <form className="card form-grid" onSubmit={submit}>
      <h2>{t("admin:orgs.create")}</h2>
      <label>
        {t("admin:orgs.name")}
        <input required maxLength={200} value={form.name} onChange={set("name")} />
      </label>
      <label>
        {t("admin:orgs.kind")}
        <select value={form.kind ?? CUSTOMER_KIND} onChange={set("kind")}>
          {KINDS.map((k) => (
            <option key={k} value={k}>
              {t(`admin:kind.${k}`)}
            </option>
          ))}
        </select>
      </label>
      <label>
        {t("admin:orgs.adminName")}
        <input required maxLength={200} value={form.admin_name} onChange={set("admin_name")} />
      </label>
      <label>
        {t("admin:orgs.adminEmail")}
        <input type="email" required value={form.admin_email} onChange={set("admin_email")} />
      </label>
      <label>
        {t("admin:orgs.locale")}
        <select value={form.default_locale ?? DEFAULT_LANGUAGE} onChange={set("default_locale")}>
          {LANGUAGES.map((l) => (
            <option key={l.code} value={l.code}>
              {l.nativeName}
            </option>
          ))}
        </select>
      </label>
      <label>
        {t("admin:orgs.timezone")}
        <input required maxLength={64} value={form.timezone ?? ""} onChange={set("timezone")} />
      </label>
      <p className="muted small">{t("admin:orgs.createHint")}</p>
      <ErrorText code={error} />
      <button type="submit" className="primary" disabled={busy}>
        {t("admin:orgs.createSubmit")}
      </button>
    </form>
  );
}

/** Every organisation on the platform, with activity counts; opens the detail view. */
export function Orgs({ canCreate, onOpen }: { canCreate: boolean; onOpen: (orgId: string) => void }) {
  const { t, i18n } = useTranslation();
  const { data, error, reload } = useApi<OrgOverview[]>(`${ADMIN_API}/orgs`);
  const [creating, setCreating] = useState(false);
  const [filter, setFilter] = useState("");
  const n = (v: number) => new Intl.NumberFormat(i18n.language).format(v);
  const shown = (data ?? []).filter((o) => o.name.toLowerCase().includes(filter.trim().toLowerCase()));

  return (
    <section>
      <PageHeader title={t("admin:nav.orgs")}>
        <input
          type="search"
          placeholder={t("admin:orgs.filter")}
          aria-label={t("admin:orgs.filter")}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        {canCreate && (
          <button type="button" onClick={() => setCreating((c) => !c)}>
            {creating ? t("action.cancel") : t("admin:orgs.create")}
          </button>
        )}
        <button type="button" onClick={reload}>
          {t("action.refresh")}
        </button>
      </PageHeader>
      {creating && (
        <CreateOrg
          onCreated={(id) => {
            setCreating(false);
            reload();
            onOpen(id);
          }}
        />
      )}
      <Loading data={data} error={error} />
      {data && (
        <table className="table" data-testid="org-list">
          <thead>
            <tr>
              <th>{t("admin:orgs.name")}</th>
              <th>{t("admin:orgs.kind")}</th>
              <th>{t("admin:orgs.status")}</th>
              <th className="num">{t("admin:orgs.members")}</th>
              <th className="num">{t("admin:orgs.openRequests")}</th>
              <th className="num">{t("admin:orgs.staff")}</th>
            </tr>
          </thead>
          <tbody>
            {shown.map((o) => (
              <tr key={o.id}>
                <td>
                  <button type="button" className="link" onClick={() => onOpen(o.id)}>
                    {o.name}
                  </button>
                </td>
                <td>{t(`admin:kind.${o.kind}`, { defaultValue: o.kind })}</td>
                <td>
                  <StatusBadge status={o.status} />
                </td>
                <td className="num">{n(o.members)}</td>
                <td className="num">{n(o.open_requests)}</td>
                <td className="num">{n(o.assigned_staff)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
