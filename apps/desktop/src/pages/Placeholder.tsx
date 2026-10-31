import { useTranslation } from "react-i18next";
import { PageHeader } from "../components/ui";

/** Honest stand-in for modules that are not built yet. */
export function Placeholder({ labelKey }: { labelKey: string }) {
  const { t } = useTranslation();
  return (
    <section>
      <PageHeader title={t(labelKey)} />
      <p className="card muted">{t("dashboard.notYet")}</p>
    </section>
  );
}
