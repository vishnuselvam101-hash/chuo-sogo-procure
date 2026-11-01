import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import type { Created, MembershipView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { ErrorText, PageHeader } from "../components/ui";
import { ItemFields, emptyItem, toItemInput, type ItemDraft } from "./ItemFields";

export const CURRENCIES = ["JPY", "USD", "EUR", "INR", "CNY", "KRW"];

interface Props {
  active: MembershipView;
  onCancel: () => void;
  onCreated: (id: string) => void;
}

/** Creates a draft. Submitting to Chuo-Sogo is a separate, explicit step on the detail page. */
export function NewRequest({ active, onCancel, onCreated }: Props) {
  const { t } = useTranslation();
  const { error, busy, run, setError } = useAction();
  const [form, setForm] = useState({
    title: "",
    description: "",
    currency: "JPY",
    needed_by: "",
    delivery_location: "",
  });
  const [items, setItems] = useState<ItemDraft[]>([emptyItem()]);

  async function save(e: FormEvent) {
    e.preventDefault();
    const parsed = items.map(toItemInput);
    if (parsed.some((i) => i === null)) {
      setError("validation_failed/quantity");
      return;
    }
    const res = await run<Created>("POST", `/api/v1/orgs/${active.org_id}/requests`, {
      ...form,
      needed_by: form.needed_by || null,
      items: parsed,
    });
    if (res) onCreated(res.body.id);
  }

  return (
    <section>
      <PageHeader title={t("procurement:request.new")}>
        <button type="button" onClick={onCancel}>
          {t("action.cancel")}
        </button>
      </PageHeader>
      <form className="card stack" onSubmit={save}>
        <label>
          {t("procurement:request.title")}
          <input
            required
            maxLength={200}
            value={form.title}
            onChange={(e) => setForm({ ...form, title: e.target.value })}
          />
        </label>
        <div className="row">
          <label>
            {t("procurement:request.currency")}
            <select value={form.currency} onChange={(e) => setForm({ ...form, currency: e.target.value })}>
              {CURRENCIES.map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </select>
          </label>
          <label>
            {t("procurement:request.neededBy")}
            <input
              type="date"
              value={form.needed_by}
              onChange={(e) => setForm({ ...form, needed_by: e.target.value })}
            />
          </label>
        </div>
        <label>
          {t("procurement:request.deliveryLocation")}
          <input
            maxLength={500}
            value={form.delivery_location}
            onChange={(e) => setForm({ ...form, delivery_location: e.target.value })}
          />
        </label>
        <label>
          {t("procurement:request.description")}
          <textarea
            rows={3}
            maxLength={4000}
            value={form.description}
            onChange={(e) => setForm({ ...form, description: e.target.value })}
          />
        </label>
        <h2>{t("procurement:item.heading")}</h2>
        {items.map((item, i) => (
          <div key={item.key} className="item-row">
            <ItemFields value={item} onChange={(v) => setItems(items.map((x, j) => (j === i ? v : x)))} />
            {items.length > 1 && (
              <button type="button" onClick={() => setItems(items.filter((_, j) => j !== i))}>
                {t("procurement:item.remove")}
              </button>
            )}
          </div>
        ))}
        <div className="row">
          <button type="button" onClick={() => setItems([...items, emptyItem()])}>
            {t("procurement:item.add")}
          </button>
          <button type="submit" className="primary" disabled={busy}>
            {t("procurement:request.saveDraft")}
          </button>
        </div>
        <ErrorText code={error} />
      </form>
    </section>
  );
}
