import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { useBridge } from "../lib/BridgeContext";
import { errorCode, ok } from "../lib/bridge";
import { useApi } from "../lib/useApi";
import type { MemberView, MembershipView } from "../lib/types";
import { ErrorText, PageHeader, StatusBadge } from "../components/ui";

const CUSTOMER_ROLES = ["customer_admin", "procurement_manager", "requester", "finance"];
const INTERNAL_ROLES = ["procurement_operator", "procurement_admin", "platform_super_admin"];

export function Members({ active, selfId }: { active: MembershipView; selfId: string }) {
  const { t } = useTranslation();
  const bridge = useBridge();
  const roles = active.org_kind === "platform" ? INTERNAL_ROLES : CUSTOMER_ROLES;
  const [error, setError] = useState<string | null>(null);
  const [form, setForm] = useState({ email: "", display_name: "", role: roles[roles.length - 1] ?? "" });
  const base = `/api/v1/orgs/${active.org_id}/members`;

  const { data: rows, error: loadError, reload } = useApi<MemberView[]>(base);

  async function add(e: FormEvent) {
    e.preventDefault();
    setError(null);
    const res = await bridge.api("POST", base, form);
    if (ok(res)) {
      setForm({ ...form, email: "", display_name: "" });
      reload();
    } else setError(errorCode(res));
  }

  async function setStatus(userId: string, status: "active" | "deactivated") {
    const res = await bridge.api("PATCH", `${base}/${userId}`, { status });
    if (ok(res)) reload();
    else setError(errorCode(res));
  }

  return (
    <section>
      <PageHeader title={t("nav.members")}>
        <button type="button" onClick={reload}>
          {t("action.refresh")}
        </button>
      </PageHeader>
      <form className="card inline-form" onSubmit={add}>
        <label>
          {t("auth.email")}
          <input
            type="email"
            required
            value={form.email}
            onChange={(e) => setForm({ ...form, email: e.target.value })}
          />
        </label>
        <label>
          {t("members.name")}
          <input
            required
            value={form.display_name}
            onChange={(e) => setForm({ ...form, display_name: e.target.value })}
          />
        </label>
        <label>
          {t("members.role")}
          <select value={form.role} onChange={(e) => setForm({ ...form, role: e.target.value })}>
            {roles.map((r) => (
              <option key={r} value={r}>
                {t(`role.${r}`)}
              </option>
            ))}
          </select>
        </label>
        <button type="submit" className="primary">
          {t("members.add")}
        </button>
      </form>
      <ErrorText code={error ?? loadError} />
      {rows === null ? (
        <p className="muted">{t("state.loading")}</p>
      ) : rows.length === 0 ? (
        <p className="muted">{t("state.empty")}</p>
      ) : (
        <table className="table">
          <thead>
            <tr>
              <th>{t("members.name")}</th>
              <th>{t("auth.email")}</th>
              <th>{t("members.role")}</th>
              <th>{t("members.status")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {rows.map((m) => (
              <tr key={m.user_id}>
                <td>{m.display_name}</td>
                <td>{m.email}</td>
                <td>{t(`role.${m.role}`)}</td>
                <td>
                  <StatusBadge
                    status={m.membership_status === "active" ? m.user_status : m.membership_status}
                  />
                </td>
                <td className="row-actions">
                  {m.user_id !== selfId && m.membership_status === "active" && (
                    <button type="button" onClick={() => void setStatus(m.user_id, "deactivated")}>
                      {t("members.deactivate")}
                    </button>
                  )}
                  {m.membership_status === "deactivated" && (
                    <button type="button" onClick={() => void setStatus(m.user_id, "active")}>
                      {t("members.activate")}
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
