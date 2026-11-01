import { useState } from "react";
import { useTranslation } from "react-i18next";
import { formatDateTime } from "../lib/format";
import { useApi } from "../lib/useApi";
import { useAction } from "../lib/useAction";
import { ErrorText, Loading, PageHeader } from "../components/ui";
import { ADMIN_API, PLATFORM_TZ, STATE, type AccountView } from "./types";
import { AdminStatus, ConfirmButton } from "./widgets";

/**
 * Search every account on the platform; deactivate/reactivate, unlock after failed sign-ins,
 * or reset the authenticator. Never the signed-in administrator's own account.
 */
export function Accounts({ selfId, canManage }: { selfId: string; canManage: boolean }) {
  const { t, i18n } = useTranslation();
  const [query, setQuery] = useState("");
  const [search, setSearch] = useState("");
  const { data, error, reload } = useApi<AccountView[]>(
    `${ADMIN_API}/accounts${search ? `?q=${encodeURIComponent(search)}` : ""}`,
  );
  const { run, busy, error: actionError } = useAction();
  const [done, setDone] = useState<string | null>(null);

  async function update(id: string, body: object, message: string) {
    setDone(null);
    if (await run("PATCH", `${ADMIN_API}/accounts/${id}`, body)) {
      setDone(message);
      reload();
    }
  }

  return (
    <section>
      <PageHeader title={t("admin:nav.accounts")}>
        <form
          className="row"
          onSubmit={(e) => {
            e.preventDefault();
            setSearch(query.trim());
          }}
        >
          <input
            type="search"
            maxLength={200}
            placeholder={t("admin:accounts.search")}
            aria-label={t("admin:accounts.search")}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <button type="submit">{t("admin:accounts.find")}</button>
        </form>
      </PageHeader>
      <ErrorText code={actionError} />
      {done && (
        <p className="ok" role="status">
          {done}
        </p>
      )}
      <Loading data={data} error={error} />
      {data && data.length === 0 && <p className="muted">{t("admin:accounts.none")}</p>}
      {data && data.length > 0 && (
        <table className="table" data-testid="account-list">
          <thead>
            <tr>
              <th>{t("members.name")}</th>
              <th>{t("admin:accounts.email")}</th>
              <th>{t("admin:accounts.kind")}</th>
              <th>{t("admin:orgs.status")}</th>
              <th>{t("admin:accounts.mfa")}</th>
              <th>{t("admin:accounts.created")}</th>
              {canManage && <th />}
            </tr>
          </thead>
          <tbody>
            {data.map((a) => {
              const self = a.id === selfId;
              return (
                <tr key={a.id}>
                  <td>{a.display_name}</td>
                  <td>{a.email}</td>
                  <td>{t(`admin:accountKind.${a.account_kind}`, { defaultValue: a.account_kind })}</td>
                  <td>
                    <AdminStatus status={a.status} />
                    {a.locked && <AdminStatus status={STATE.locked} />}
                  </td>
                  <td>{a.mfa_enabled ? t("admin:accounts.mfaOn") : "—"}</td>
                  <td>{formatDateTime(a.created_at, i18n.language, PLATFORM_TZ)}</td>
                  {canManage && (
                    <td className="row">
                      {self ? (
                        <span className="muted small">{t("admin:accounts.self")}</span>
                      ) : (
                        <>
                          {a.status === "active" && (
                            <ConfirmButton
                              label={t("admin:accounts.deactivate")}
                              confirmLabel={t("admin:accounts.confirmDeactivate", { name: a.display_name })}
                              disabled={busy}
                              onConfirm={() =>
                                void update(
                                  a.id,
                                  { status: STATE.deactivated },
                                  t("admin:accounts.deactivated"),
                                )
                              }
                            />
                          )}
                          {a.status === "deactivated" && (
                            <button
                              type="button"
                              disabled={busy}
                              onClick={() =>
                                void update(a.id, { status: STATE.active }, t("admin:accounts.reactivated"))
                              }
                            >
                              {t("admin:accounts.reactivate")}
                            </button>
                          )}
                          {a.locked && (
                            <button
                              type="button"
                              disabled={busy}
                              onClick={() =>
                                void update(a.id, { unlock: true }, t("admin:accounts.unlocked"))
                              }
                            >
                              {t("admin:accounts.unlock")}
                            </button>
                          )}
                          {a.mfa_enabled && (
                            <ConfirmButton
                              label={t("admin:accounts.resetMfa")}
                              confirmLabel={t("admin:accounts.confirmResetMfa")}
                              disabled={busy}
                              onConfirm={() =>
                                void update(a.id, { reset_mfa: true }, t("admin:accounts.mfaReset"))
                              }
                            />
                          )}
                        </>
                      )}
                    </td>
                  )}
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </section>
  );
}
