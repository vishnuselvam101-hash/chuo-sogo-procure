import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { useBridge } from "../lib/BridgeContext";
import { errorCode, ok } from "../lib/bridge";
import { useApi } from "../lib/useApi";
import type { OrgView } from "../lib/types";
import { ErrorText, PageHeader, StatusBadge } from "../components/ui";
import { LANGUAGES } from "../i18n";

export function Organisations() {
  const { t } = useTranslation();
  const bridge = useBridge();
  const [error, setError] = useState<string | null>(null);
  const { data: rows, error: loadError, reload } = useApi<OrgView[]>("/api/v1/platform/organisations");
  const empty = { name: "", admin_email: "", admin_name: "", default_locale: "ja" };
  const [form, setForm] = useState(empty);

  async function create(e: FormEvent) {
    e.preventDefault();
    setError(null);
    const res = await bridge.api("POST", "/api/v1/platform/organisations", form);
    if (ok(res)) {
      setForm(empty);
      reload();
    } else setError(errorCode(res));
  }

  return (
    <section>
      <PageHeader title={t("nav.organisations")} />
      <form className="card inline-form" onSubmit={create}>
        <label>
          {t("orgs.name")}
          <input required value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
        </label>
        <label>
          {t("orgs.adminEmail")}
          <input
            type="email"
            required
            value={form.admin_email}
            onChange={(e) => setForm({ ...form, admin_email: e.target.value })}
          />
        </label>
        <label>
          {t("orgs.adminName")}
          <input
            required
            value={form.admin_name}
            onChange={(e) => setForm({ ...form, admin_name: e.target.value })}
          />
        </label>
        <label>
          {t("nav.language")}
          <select
            value={form.default_locale}
            onChange={(e) => setForm({ ...form, default_locale: e.target.value })}
          >
            {LANGUAGES.map((l) => (
              <option key={l.code} value={l.code}>
                {l.nativeName}
              </option>
            ))}
          </select>
        </label>
        <button type="submit" className="primary">
          {t("orgs.create")}
        </button>
      </form>
      <ErrorText code={error ?? loadError} />
      {rows && (
        <table className="table">
          <thead>
            <tr>
              <th>{t("orgs.name")}</th>
              <th>{t("nav.language")}</th>
              <th>{t("members.status")}</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((o) => (
              <tr key={o.id}>
                <td>{o.name}</td>
                <td>{LANGUAGES.find((l) => l.code === o.default_locale)?.nativeName ?? o.default_locale}</td>
                <td>
                  <StatusBadge status={o.status} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
