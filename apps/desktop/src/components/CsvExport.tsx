import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useBridge } from "../lib/BridgeContext";
import { errorCode, ok } from "../lib/bridge";
import type { MembershipView } from "../lib/types";
import { ErrorText } from "./ui";

/** Saves the orders or savings-ledger CSV for the factory's own systems (one month or all). */
export function CsvExport({ active, kind }: { active: MembershipView; kind: "orders" | "savings" }) {
  const { t } = useTranslation();
  const bridge = useBridge();
  const [month, setMonth] = useState("");
  const [saved, setSaved] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function save() {
    setBusy(true);
    try {
      const res = await bridge.csvDownload(active.org_id, kind, month || null);
      if (ok(res)) {
        setSaved(res.body.saved_to);
        setError(null);
      } else setError(errorCode(res));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="row small">
      <label>
        {t("common:csv.month")}
        <input type="month" value={month} onChange={(e) => setMonth(e.target.value)} />
      </label>
      <button type="button" disabled={busy} onClick={() => void save()}>
        {t(`common:csv.${kind}`)}
      </button>
      {saved && <span className="muted">{t("common:csv.saved", { path: saved })}</span>}
      <ErrorText code={error} />
    </div>
  );
}
