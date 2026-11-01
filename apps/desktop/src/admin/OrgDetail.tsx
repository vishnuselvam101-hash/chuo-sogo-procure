import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { LANGUAGES } from "../i18n";
import { formatDateTime } from "../lib/format";
import { useApi } from "../lib/useApi";
import { useAction } from "../lib/useAction";
import type { AuditResponse, MemberView } from "../lib/types";
import { ErrorText, Loading, PageHeader } from "../components/ui";
import {
  ADMIN_API,
  NOT_FOUND,
  PERM,
  PLATFORM_TZ,
  STATE,
  type AccountView,
  type AssignmentView,
  type OrgOverview,
  type OrgSettings,
} from "./types";
import { AdminStatus, ConfirmButton } from "./widgets";

type Tab = "members" | "rules" | "staff" | "audit" | "profile";

/** Rules the platform applies to this organisation (defaults until first saved). */
function Rules({ orgId }: { orgId: string }) {
  const path = `${ADMIN_API}/orgs/${orgId}/settings`;
  const { data, error } = useApi<OrgSettings>(path);
  if (!data) return <Loading data={data} error={error} />;
  return <RulesForm path={path} initial={data} />;
}

function RulesForm({ path, initial }: { path: string; initial: OrgSettings }) {
  const { t } = useTranslation();
  const { run, busy, error: saveError } = useAction();
  const [form, setForm] = useState<OrgSettings>(initial);
  const [saved, setSaved] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setSaved(false);
    if (await run("PUT", path, form)) setSaved(true);
  }

  const num = (k: "identify_sla_hours" | "chase_default_hours") => (e: { target: { value: string } }) =>
    setForm({ ...form, [k]: Number.parseInt(e.target.value, 10) || 0 });

  return (
    <form className="card form-grid" onSubmit={submit} data-testid="org-rules">
      <label>
        {t("admin:rules.identifySla")}
        <input
          type="number"
          min={1}
          max={720}
          required
          value={form.identify_sla_hours}
          onChange={num("identify_sla_hours")}
        />
      </label>
      <label>
        {t("admin:rules.chaseDefault")}
        <input
          type="number"
          min={1}
          max={2160}
          required
          value={form.chase_default_hours}
          onChange={num("chase_default_hours")}
        />
      </label>
      <label>
        {t("admin:rules.taxRate")}
        {/* Whole percent in steps of 0.01; stored as basis points, never as a float on the server. */}
        <input
          type="number"
          min={0}
          max={100}
          step={0.01}
          required
          value={form.default_tax_rate_bps / 100}
          onChange={(e) =>
            setForm({ ...form, default_tax_rate_bps: Math.round(Number(e.target.value) * 100) })
          }
        />
      </label>
      <label className="checkbox">
        <input
          type="checkbox"
          checked={form.ai_enabled}
          onChange={(e) => setForm({ ...form, ai_enabled: e.target.checked })}
        />
        {t("admin:rules.aiEnabled")}
      </label>
      <p className="muted small">{t("admin:rules.hint")}</p>
      <ErrorText code={saveError} />
      {saved && (
        <p className="ok" role="status">
          {t("admin:saved")}
        </p>
      )}
      <button type="submit" className="primary" disabled={busy}>
        {t("action.save")}
      </button>
    </form>
  );
}

function Members({ orgId }: { orgId: string }) {
  const { t } = useTranslation();
  const { data, error } = useApi<MemberView[]>(`${ADMIN_API}/orgs/${orgId}/members`);
  return (
    <>
      <Loading data={data} error={error} />
      {data && (
        <table className="table" data-testid="org-members">
          <thead>
            <tr>
              <th>{t("members.name")}</th>
              <th>{t("admin:accounts.email")}</th>
              <th>{t("members.role")}</th>
              <th>{t("admin:orgs.status")}</th>
            </tr>
          </thead>
          <tbody>
            {data.map((m) => (
              <tr key={m.user_id}>
                <td>{m.display_name}</td>
                <td>{m.email}</td>
                <td>{t(`role.${m.role}`, { defaultValue: m.role })}</td>
                <td>
                  <AdminStatus status={m.membership_status} />
                  {m.user_status !== "active" && <AdminStatus status={m.user_status} />}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </>
  );
}

/** Chuo-Sogo staff who serve this customer organisation. */
function Staff({ orgId, canManage }: { orgId: string; canManage: boolean }) {
  const { t, i18n } = useTranslation();
  const path = `${ADMIN_API}/orgs/${orgId}/assignments`;
  const { data, error, reload } = useApi<AssignmentView[]>(path);
  const staff = useApi<AccountView[]>(`${ADMIN_API}/accounts`);
  const { run, busy, error: actionError } = useAction();
  const [pick, setPick] = useState("");
  const assigned = new Set((data ?? []).map((a) => a.user_id));
  const candidates = (staff.data ?? []).filter(
    (a) => a.account_kind === "internal" && a.status === "active" && !assigned.has(a.id),
  );

  return (
    <>
      <Loading data={data} error={error} />
      {canManage && (
        <div className="row">
          <select value={pick} onChange={(e) => setPick(e.target.value)} aria-label={t("admin:staff.pick")}>
            <option value="">{t("admin:staff.pick")}</option>
            {candidates.map((a) => (
              <option key={a.id} value={a.id}>
                {a.display_name} ({a.email})
              </option>
            ))}
          </select>
          <button
            type="button"
            disabled={!pick || busy}
            onClick={async () => {
              if (await run("POST", path, { user_id: pick })) {
                setPick("");
                reload();
              }
            }}
          >
            {t("admin:staff.assign")}
          </button>
        </div>
      )}
      <ErrorText code={actionError} />
      {data && data.length === 0 && <p className="muted">{t("admin:staff.none")}</p>}
      {data && data.length > 0 && (
        <table className="table" data-testid="org-staff">
          <thead>
            <tr>
              <th>{t("members.name")}</th>
              <th>{t("admin:accounts.email")}</th>
              <th>{t("admin:staff.since")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {data.map((a) => (
              <tr key={a.user_id}>
                <td>{a.display_name ?? "—"}</td>
                <td>{a.email ?? "—"}</td>
                <td>{formatDateTime(a.created_at, i18n.language, PLATFORM_TZ)}</td>
                <td>
                  {canManage && (
                    <ConfirmButton
                      label={t("admin:staff.unassign")}
                      confirmLabel={t("admin:staff.confirmUnassign")}
                      disabled={busy}
                      onConfirm={async () => {
                        if (await run("DELETE", `${path}/${a.user_id}`)) reload();
                      }}
                    />
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </>
  );
}

function AuditLog({ orgId, timezone }: { orgId: string; timezone: string }) {
  const { t, i18n } = useTranslation();
  const { data, error, reload } = useApi<AuditResponse>(`${ADMIN_API}/orgs/${orgId}/audit?limit=200`);
  return (
    <>
      <div className="row">
        <button type="button" onClick={reload}>
          {t("action.refresh")}
        </button>
      </div>
      <Loading data={data} error={error} />
      {data && (
        <>
          <p className={data.chain_valid ? "ok" : "error"} role="status" data-testid="audit-chain">
            {data.chain_valid
              ? t("audit.chainValid")
              : t("audit.chainBroken", { seq: data.first_broken_seq })}
          </p>
          <table className="table">
            <thead>
              <tr>
                <th>#</th>
                <th>{t("audit.when")}</th>
                <th>{t("audit.action")}</th>
                <th>{t("admin:audit.details")}</th>
              </tr>
            </thead>
            <tbody>
              {data.events.map((e) => (
                <tr key={e.seq}>
                  <td className="num">{e.seq}</td>
                  <td>{formatDateTime(e.created_at, i18n.language, timezone)}</td>
                  <td>
                    <code>{e.action}</code>
                  </td>
                  <td>
                    <details>
                      <summary>{e.entity_type}</summary>
                      <pre className="small">{JSON.stringify(e.data, null, 2)}</pre>
                    </details>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
    </>
  );
}

/** Name, language, time zone and suspension of an organisation. */
function Profile({ org, onChanged }: { org: OrgOverview; onChanged: () => void }) {
  const { t } = useTranslation();
  const { run, busy, error } = useAction();
  const [name, setName] = useState(org.name);
  const [locale, setLocale] = useState(org.default_locale);
  const [timezone, setTimezone] = useState(org.timezone);
  const [saved, setSaved] = useState(false);
  const path = `${ADMIN_API}/orgs/${org.id}`;

  async function submit(e: FormEvent) {
    e.preventDefault();
    setSaved(false);
    if (await run("PATCH", path, { name, default_locale: locale, timezone })) {
      setSaved(true);
      onChanged();
    }
  }

  return (
    <>
      <form className="card form-grid" onSubmit={submit}>
        <label>
          {t("admin:orgs.name")}
          <input required maxLength={200} value={name} onChange={(e) => setName(e.target.value)} />
        </label>
        <label>
          {t("admin:orgs.locale")}
          <select value={locale} onChange={(e) => setLocale(e.target.value)}>
            {LANGUAGES.map((l) => (
              <option key={l.code} value={l.code}>
                {l.nativeName}
              </option>
            ))}
          </select>
        </label>
        <label>
          {t("admin:orgs.timezone")}
          <input required maxLength={64} value={timezone} onChange={(e) => setTimezone(e.target.value)} />
        </label>
        {saved && (
          <p className="ok" role="status">
            {t("admin:saved")}
          </p>
        )}
        <button type="submit" className="primary" disabled={busy}>
          {t("action.save")}
        </button>
      </form>
      {org.kind !== "platform" && (
        <div className="card">
          <h2>{t("admin:orgs.access")}</h2>
          <p className="muted small">{t("admin:orgs.suspendHint")}</p>
          {org.status === "active" ? (
            <ConfirmButton
              label={t("admin:orgs.suspend")}
              confirmLabel={t("admin:orgs.confirmSuspend", { name: org.name })}
              disabled={busy}
              onConfirm={async () => {
                if (await run("PATCH", path, { status: STATE.suspended })) onChanged();
              }}
            />
          ) : (
            <button
              type="button"
              disabled={busy}
              onClick={async () => {
                if (await run("PATCH", path, { status: STATE.active })) onChanged();
              }}
            >
              {t("admin:orgs.reactivate")}
            </button>
          )}
        </div>
      )}
      <ErrorText code={error} />
    </>
  );
}

export function OrgDetail({
  orgId,
  permissions,
  onBack,
}: {
  orgId: string;
  permissions: string[];
  onBack: () => void;
}) {
  const { t } = useTranslation();
  const orgs = useApi<OrgOverview[]>(`${ADMIN_API}/orgs`);
  const [tab, setTab] = useState<Tab>("members");
  const org = orgs.data?.find((o) => o.id === orgId);
  const tabs: Tab[] = [
    "members",
    "rules",
    ...(org?.kind === "customer" ? (["staff"] as const) : []),
    "audit",
    "profile",
  ];

  return (
    <section>
      <PageHeader title={org?.name ?? t("admin:nav.orgs")}>
        <button type="button" onClick={onBack}>
          {t("action.back")}
        </button>
      </PageHeader>
      <Loading data={orgs.data} error={orgs.error} />
      {orgs.data && !org && <ErrorText code={NOT_FOUND} />}
      {org && (
        <>
          <p className="muted">
            {t(`admin:kind.${org.kind}`, { defaultValue: org.kind })} · <AdminStatus status={org.status} /> ·{" "}
            {org.timezone}
          </p>
          <div className="tabs" role="tablist">
            {tabs.map((id) => (
              <button
                key={id}
                type="button"
                role="tab"
                aria-selected={tab === id}
                className={tab === id ? "tab active" : "tab"}
                onClick={() => setTab(id)}
              >
                {t(`admin:tab.${id}`)}
              </button>
            ))}
          </div>
          {tab === "members" && <Members orgId={org.id} />}
          {tab === "rules" && <Rules orgId={org.id} />}
          {tab === "staff" && (
            <Staff orgId={org.id} canManage={permissions.includes(PERM.manageAssignments)} />
          )}
          {tab === "audit" && <AuditLog orgId={org.id} timezone={org.timezone} />}
          {tab === "profile" && (
            <Profile key={`${org.name}${org.status}`} org={org} onChanged={orgs.reload} />
          )}
        </>
      )}
    </section>
  );
}
