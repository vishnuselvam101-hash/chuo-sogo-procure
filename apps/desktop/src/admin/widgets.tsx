import { useState } from "react";
import { useTranslation } from "react-i18next";

/** Organisation and account states, labelled from the admin namespace. */
export function AdminStatus({ status }: { status: string }) {
  const { t } = useTranslation();
  return (
    <span className={`badge badge-${status}`}>{t(`admin:status.${status}`, { defaultValue: status })}</span>
  );
}

/**
 * A dangerous action needs a second, explicit click ("Suspend" → "Confirm: suspend").
 * No browser dialogs: they are not available in every webview.
 */
export function ConfirmButton({
  label,
  confirmLabel,
  disabled,
  onConfirm,
}: {
  label: string;
  confirmLabel: string;
  disabled?: boolean;
  onConfirm: () => void;
}) {
  const { t } = useTranslation();
  const [armed, setArmed] = useState(false);
  if (!armed)
    return (
      <button type="button" disabled={disabled} onClick={() => setArmed(true)}>
        {label}
      </button>
    );
  return (
    <span className="row">
      <button
        type="button"
        className="danger"
        disabled={disabled}
        onClick={() => {
          setArmed(false);
          onConfirm();
        }}
      >
        {confirmLabel}
      </button>
      <button type="button" className="link" onClick={() => setArmed(false)}>
        {t("action.cancel")}
      </button>
    </span>
  );
}
