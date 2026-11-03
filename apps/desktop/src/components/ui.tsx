import { formatMoney } from "../lib/format";
import { useEffect, useRef, useState } from "react";
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

/** An empty list says so, and says what to do about it. */
export function EmptyState({ hint }: { hint?: string }) {
  const { t } = useTranslation();
  return (
    <div className="empty-state" role="status">
      <p className="empty-title">{t("state.empty")}</p>
      {hint && <p className="empty-hint">{hint}</p>}
    </div>
  );
}

/**
 * A dangerous action needs a second, explicit click ("Remove" → "Yes, Remove").
 * No browser dialogs: they are not available in every webview.
 */
export function ConfirmButton({
  label,
  confirmLabel,
  disabled,
  className,
  onConfirm,
}: {
  label: string;
  confirmLabel?: string;
  disabled?: boolean;
  className?: string;
  onConfirm: () => void;
}) {
  const { t } = useTranslation();
  const [armed, setArmed] = useState(false);
  const yes = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (armed) yes.current?.focus();
  }, [armed]);
  if (!armed)
    return (
      <button type="button" className={className} disabled={disabled} onClick={() => setArmed(true)}>
        {label}
      </button>
    );
  return (
    <span className="confirm" role="group" aria-label={t("action.confirmAsk")}>
      <span className="confirm-ask">{t("action.confirmAsk")}</span>
      <button
        ref={yes}
        type="button"
        className="danger"
        disabled={disabled}
        onClick={() => {
          setArmed(false);
          onConfirm();
        }}
      >
        {confirmLabel ?? t("action.confirmDo", { action: label })}
      </button>
      <button type="button" onClick={() => setArmed(false)}>
        {t("action.cancel")}
      </button>
    </span>
  );
}
