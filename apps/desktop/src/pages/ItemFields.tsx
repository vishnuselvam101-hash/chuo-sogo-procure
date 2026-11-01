import { useTranslation } from "react-i18next";
import type { ItemInput } from "../lib/types";

export interface ItemDraft {
  key: number;
  description: string;
  manufacturer: string;
  mpn: string;
  quantity: string;
  unit: string;
}

let nextKey = 0;

export function emptyItem(): ItemDraft {
  nextKey += 1;
  return { key: nextKey, description: "", manufacturer: "", mpn: "", quantity: "1", unit: "pcs" };
}

/** Whole positive quantities only (v1); null when the quantity is not a valid integer. */
export function toItemInput(d: ItemDraft): ItemInput | null {
  if (!/^\d{1,10}$/.test(d.quantity.trim())) return null;
  const quantity = Number(d.quantity.trim());
  if (quantity < 1 || quantity > 1_000_000_000) return null;
  return {
    description: d.description,
    manufacturer: d.manufacturer,
    mpn: d.mpn,
    quantity,
    unit: d.unit.trim() || "pcs",
    notes: "",
  };
}

export function ItemFields({ value, onChange }: { value: ItemDraft; onChange: (v: ItemDraft) => void }) {
  const { t } = useTranslation();
  const set = (k: keyof ItemDraft) => (e: React.ChangeEvent<HTMLInputElement>) =>
    onChange({ ...value, [k]: e.target.value });
  return (
    <div className="item-fields">
      <label className="grow">
        {t("procurement:item.description")}
        <input required maxLength={1000} value={value.description} onChange={set("description")} />
      </label>
      <label>
        {t("procurement:item.manufacturer")}
        <input maxLength={200} value={value.manufacturer} onChange={set("manufacturer")} />
      </label>
      <label>
        {t("procurement:item.mpn")}
        <input maxLength={100} value={value.mpn} onChange={set("mpn")} />
      </label>
      <label className="narrow">
        {t("procurement:item.quantity")}
        <input required inputMode="numeric" value={value.quantity} onChange={set("quantity")} />
      </label>
      <label className="narrow">
        {t("procurement:item.unit")}
        <input required pattern="[a-z_]{1,16}" value={value.unit} onChange={set("unit")} />
      </label>
    </div>
  );
}
