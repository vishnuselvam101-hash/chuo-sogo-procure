import { formatMoney } from "../lib/format";
import { useTranslation } from "react-i18next";

export function ErrorText({ code }: { code: string | null }) {
  const { t } = useTranslation();
  if (!code) return null;
  const [general, detail] = code.split("/");
  const keys = detail ? [`errors:detail.${detail}`, `errors:${general}`] : [`errors:${general}`];
  return (
    <p role="alert" className="error">
      {t(keys, { defaultValue: t("errors:unknown") })}
    </p>
  );
}

export function StatusBadge({ status }: { status: string }) {
  const { t } = useTranslation();
  return <span className={`badge badge-${status}`}>{t(`status.${status}`, { defaultValue: status })}</span>;
}

export function PageHeader({ title, children }: { title: string; children?: React.ReactNode }) {
  return (
    <header className="page-header">
      <h1>{title}</h1>
      <div className="page-actions">{children}</div>
    </header>
  );
}

/** Money in integer minor units, formatted for the UI language. */
export function Money({ minor, currency }: { minor: number | null | undefined; currency: string }) {
  const { i18n } = useTranslation();
  if (minor === null || minor === undefined) return <span className="muted">—</span>;
  return <span className="num">{formatMoney(minor, currency, i18n.language)}</span>;
}

export function Loading({ data, error }: { data: unknown; error: string | null }) {
  const { t } = useTranslation();
  if (error) return <ErrorText code={error} />;
  if (data === null) return <p className="muted">{t("state.loading")}</p>;
  return null;
}
