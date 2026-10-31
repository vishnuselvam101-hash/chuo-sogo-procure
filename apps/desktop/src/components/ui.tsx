import { useTranslation } from "react-i18next";

export function ErrorText({ code }: { code: string | null }) {
  const { t } = useTranslation();
  if (!code) return null;
  return (
    <p role="alert" className="error">
      {t(`errors:${code}`, { defaultValue: t("errors:unknown") })}
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
