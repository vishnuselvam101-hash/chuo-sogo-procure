import { useState } from "react";
import { useTranslation } from "react-i18next";
import { formatDateTime } from "../lib/format";
import type { InboxMessage, MembershipView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { ErrorText, Loading, PageHeader } from "../components/ui";

/**
 * Supplier messages the server could not match to an RFQ by itself: received FAXes and new
 * email threads. Our staff read each one and assign it to the RFQ it answers (in the
 * organisation they are working in), or set it aside.
 */
export function MailInbox({ active }: { active: MembershipView }) {
  const { t, i18n } = useTranslation();
  const { data, error, reload } = useApi<InboxMessage[]>("/api/v1/mail-inbox");
  const action = useAction();
  const [open, setOpen] = useState<{ id: string; text: string } | null>(null);
  const [number, setNumber] = useState("");

  async function read(id: string) {
    const res = await action.run<{ text: string }>("GET", `/api/v1/mail-inbox/${id}/text`);
    if (res) {
      setOpen({ id, text: res.body.text });
      setNumber("");
    }
  }
  async function assign(id: string) {
    const res = await action.run("POST", `/api/v1/mail-inbox/${id}/assign`, {
      org_id: active.org_id,
      rfq_number: number.trim(),
    });
    if (res) {
      setOpen(null);
      reload();
    }
  }
  async function setAside(id: string) {
    if (await action.run("POST", `/api/v1/mail-inbox/${id}/set-aside`)) {
      setOpen(null);
      reload();
    }
  }

  return (
    <section className="stack">
      <PageHeader title={t("nav.mailInbox")} />
      <p className="lead">{t("procurement:mail.inboxLead", { org: active.org_name })}</p>
      <Loading data={data} error={error} />
      {data && data.length === 0 && <p className="muted">{t("procurement:mail.inboxEmpty")}</p>}
      {data?.map((m) => (
        <div key={m.id} className="card stack">
          <div className="row">
            <strong>{m.subject || t("procurement:mail.noSubject")}</strong>
            <span className="muted small">
              {t(`procurement:mail.channel.${m.channel}`)} · {m.from_address} ·{" "}
              {formatDateTime(m.received_at, i18n.language, active.timezone)}
            </span>
            <button type="button" disabled={action.busy} onClick={() => void read(m.id)}>
              {t("procurement:mail.open")}
            </button>
          </div>
          {open?.id === m.id && (
            <>
              <pre className="pre">{open.text}</pre>
              <div className="row">
                <label className="grow">
                  {t("procurement:mail.rfqNumber")}
                  <input value={number} maxLength={40} onChange={(e) => setNumber(e.target.value)} />
                </label>
                <button
                  type="button"
                  className="primary"
                  disabled={action.busy || !number.trim()}
                  onClick={() => void assign(m.id)}
                >
                  {t("procurement:mail.assign")}
                </button>
                <button type="button" disabled={action.busy} onClick={() => void setAside(m.id)}>
                  {t("procurement:mail.setAside")}
                </button>
              </div>
            </>
          )}
        </div>
      ))}
      <ErrorText code={action.error} />
    </section>
  );
}
