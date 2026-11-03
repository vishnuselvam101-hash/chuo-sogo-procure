import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "../lib/api-schema";
import { MAX_UPLOAD_BYTES, readAsBase64 } from "../lib/files";
import type { MembershipView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { ErrorText } from "../components/ui";

type ImportResult = components["schemas"]["ImportRequestsResponse"];

/** Purchase requests from the factory's own CSV / Excel file: one draft per request name. */
export function RequestImport({ active, onImported }: { active: MembershipView; onImported: () => void }) {
  const { t } = useTranslation();
  const action = useAction();
  const [result, setResult] = useState<ImportResult | null>(null);

  async function upload(file: File) {
    if (file.size > MAX_UPLOAD_BYTES) return action.setError("validation_failed/file_size");
    const data = await readAsBase64(file);
    const r = await action.run<ImportResult>("POST", `/api/v1/orgs/${active.org_id}/requests/import`, {
      file_name: file.name,
      data_base64: data,
    });
    if (r) {
      setResult(r.body);
      onImported();
    }
  }

  return (
    <div className="card stack">
      <h2>{t("procurement:import.heading")}</h2>
      <p className="muted small">{t("procurement:import.explain")}</p>
      <input
        type="file"
        accept=".csv,.tsv,.xlsx"
        aria-label={t("procurement:import.heading")}
        disabled={action.busy}
        onChange={(e) => {
          const f = e.target.files?.[0];
          if (f) void upload(f);
          e.target.value = "";
        }}
      />
      {result && (
        <div className="small">
          <p>{t("procurement:import.created", { requests: result.created.length, lines: result.lines })}</p>
          {result.skipped.length > 0 && (
            <ul>
              {result.skipped.map((s) => (
                <li key={s.row}>
                  {t("procurement:import.skipped", {
                    row: s.row,
                    reason: t(`procurement:import.reason.${s.reason}`),
                  })}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
      <ErrorText code={action.error} />
    </div>
  );
}
