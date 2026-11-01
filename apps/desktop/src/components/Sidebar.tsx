import { useTranslation } from "react-i18next";
import type { LanguageCode } from "../i18n";
import { navFor, type PageId } from "../lib/nav";
import type { MembershipView } from "../lib/types";
import { LanguageMenu } from "./LanguageMenu";

interface Props {
  memberships: MembershipView[];
  active: MembershipView;
  page: PageId;
  language: LanguageCode;
  onNavigate: (page: PageId) => void;
  onSwitchOrg: (orgId: string) => void;
  onLanguage: (code: LanguageCode) => void;
  onSignOut: () => void;
}

export function Sidebar(props: Props) {
  const { t } = useTranslation();
  const { active } = props;
  return (
    <nav className="sidebar" aria-label={t("app.name")}>
      <div className="brand">
        <div className="brand-name">{t("app.name")}</div>
        <div className="brand-workspace">
          {t(
            active.org_kind === "platform"
              ? "workspace.operations"
              : active.access === "assigned"
                ? "workspace.serving"
                : "workspace.customer",
          )}
        </div>
      </div>
      {props.memberships.length > 1 ? (
        <label className="org-switch">
          <span className="visually-hidden">{t("org.switch")}</span>
          <select value={active.org_id} onChange={(e) => props.onSwitchOrg(e.target.value)}>
            {props.memberships.map((m) => (
              <option key={m.org_id} value={m.org_id}>
                {m.org_name}
              </option>
            ))}
          </select>
        </label>
      ) : (
        <div className="org-name">{active.org_name}</div>
      )}
      <ul className="nav-list">
        {navFor(active).map((item) => (
          <li key={item.id}>
            <button
              type="button"
              className={`nav-item${props.page === item.id ? " active" : ""}`}
              aria-current={props.page === item.id ? "page" : undefined}
              onClick={() => props.onNavigate(item.id)}
            >
              {t(item.labelKey)}
            </button>
          </li>
        ))}
      </ul>
      <div className="sidebar-footer">
        <LanguageMenu current={props.language} onChange={props.onLanguage} />
        <button type="button" className="nav-item" onClick={props.onSignOut}>
          {t("auth.signOut")}
        </button>
      </div>
    </nav>
  );
}
