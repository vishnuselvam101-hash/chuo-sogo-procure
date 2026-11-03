import { useTranslation } from "react-i18next";

/** Organisation and account states, labelled from the admin namespace. */
export function AdminStatus({ status }: { status: string }) {
  const { t } = useTranslation();
  return (
    <span className={`badge badge-${status}`}>{t(`admin:status.${status}`, { defaultValue: status })}</span>
  );
}

export { ConfirmButton } from "../components/ui";
