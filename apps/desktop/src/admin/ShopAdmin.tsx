import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { useApi } from "../lib/useApi";
import { useAction } from "../lib/useAction";
import { formatDateTime, minorToInput, parseMinor } from "../lib/format";
import type { ShopOrder, ShopOrderSummary, ShopSettings } from "../lib/types";
import { ErrorText, Loading, Money, PageHeader } from "../components/ui";
import { OrderStatus } from "../shop/ShopPages";
import { ConfirmButton } from "./widgets";
import { ADMIN_API, PLATFORM_TZ } from "./types";

const JPY = "JPY";
const STATUSES = ["placed", "confirmed", "paid", "shipped", "completed", "cancelled"] as const;
const ACTIONS = {
  confirm: "confirm",
  paid: "paid",
  ship: "ship",
  complete: "complete",
  cancel: "cancel",
} as const;
const NOTICE_FIELDS = [
  "seller_name",
  "responsible_person",
  "seller_address",
  "seller_phone",
  "seller_email",
  "price_info",
  "payment_info",
  "delivery_info",
  "returns_policy",
  "privacy_policy",
] as const;

function Settings({ onDone }: { onDone: () => void }) {
  const { t } = useTranslation();
  const path = `${ADMIN_API}/shop/settings`;
  const { data, error } = useApi<ShopSettings>(path);
  if (!data) return <Loading data={data} error={error} />;
  return <SettingsForm path={path} initial={data} onDone={onDone} title={t("shop:admin.settings")} />;
}

function SettingsForm({
  path,
  initial,
  onDone,
  title,
}: {
  path: string;
  initial: ShopSettings;
  onDone: () => void;
  title: string;
}) {
  const { t } = useTranslation();
  const { run, busy, error, setError } = useAction();
  const [f, setF] = useState(initial);
  const [fee, setFee] = useState(minorToInput(initial.shipping_fee_minor, JPY));
  const [free, setFree] = useState(
    initial.free_shipping_over_minor === null || initial.free_shipping_over_minor === undefined
      ? ""
      : minorToInput(initial.free_shipping_over_minor, JPY),
  );

  async function submit(e: FormEvent) {
    e.preventDefault();
    const shipping = parseMinor(fee, JPY);
    const freeOver = free.trim() ? parseMinor(free, JPY) : null;
    if (shipping === null || (free.trim() && freeOver === null)) return setError("validation_failed/amount");
    if (await run("PUT", path, { ...f, shipping_fee_minor: shipping, free_shipping_over_minor: freeOver }))
      onDone();
  }

  return (
    <form className="card stack" onSubmit={submit} data-testid="shop-settings">
      <h2>{title}</h2>
      <label className="checkbox">
        <input type="checkbox" checked={f.open} onChange={(e) => setF({ ...f, open: e.target.checked })} />
        {t("shop:admin.open")}
      </label>
      <p className="muted small">{t("shop:admin.openHint")}</p>
      <div className="form-grid">
        <label>
          {t("shop:admin.shippingFee")}
          <input inputMode="numeric" value={fee} onChange={(e) => setFee(e.target.value)} />
        </label>
        <label>
          {t("shop:admin.freeOver")}
          <input inputMode="numeric" value={free} onChange={(e) => setFree(e.target.value)} />
        </label>
        <label>
          {t("shop:admin.paymentDays")}
          <input
            type="number"
            min={1}
            max={60}
            value={f.payment_days}
            onChange={(e) => setF({ ...f, payment_days: Number.parseInt(e.target.value, 10) || 7 })}
          />
        </label>
      </div>
      <h3>{t("shop:legal.title")}</h3>
      {NOTICE_FIELDS.map((k) => (
        <label key={k}>
          {t(`shop:admin.field.${k}`)}
          <textarea
            rows={k === "privacy_policy" || k === "returns_policy" ? 4 : 1}
            value={f[k]}
            onChange={(e) => setF({ ...f, [k]: e.target.value })}
          />
        </label>
      ))}
      <ErrorText code={error} />
      <div className="row">
        <button type="submit" className="primary" disabled={busy}>
          {t("action.save")}
        </button>
        <button type="button" onClick={onDone}>
          {t("action.cancel")}
        </button>
      </div>
    </form>
  );
}

function OrderAdmin({ id, onBack }: { id: string; onBack: () => void }) {
  const { t, i18n } = useTranslation();
  const path = `${ADMIN_API}/shop/orders/${id}`;
  const { data, error, reload } = useApi<ShopOrder>(path);
  const { run, busy, error: actionError } = useAction();
  const [paidOn, setPaidOn] = useState("");
  const [carrier, setCarrier] = useState("");
  const [tracking, setTracking] = useState("");
  const [reason, setReason] = useState("");
  if (!data) return <Loading data={data} error={error} />;
  const act = async (body: object) => {
    if (await run("POST", `${path}/action`, body)) reload();
  };
  return (
    <section>
      <PageHeader title={data.number}>
        <OrderStatus status={data.status} />
        <button type="button" onClick={onBack}>
          {t("action.back")}
        </button>
      </PageHeader>
      <dl className="facts card">
        <dt>{t("shop:admin.customer")}</dt>
        <dd>
          {data.ship_to.name} · {data.contact_email}
        </dd>
        <dt>{t("shop:shipTo")}</dt>
        <dd>
          {`〒${data.ship_to.postal_code} ${data.ship_to.prefecture}${data.ship_to.address_line}`} ·{" "}
          {data.ship_to.phone}
        </dd>
        <dt>{t("shop:admin.placed")}</dt>
        <dd>{formatDateTime(data.placed_at, i18n.language, PLATFORM_TZ)}</dd>
        {data.note && (
          <>
            <dt>{t("shop:note")}</dt>
            <dd>{data.note}</dd>
          </>
        )}
        {data.invoice_number && (
          <>
            <dt>{t("shop:admin.invoice")}</dt>
            <dd>{data.invoice_number}</dd>
          </>
        )}
        {data.credit_note_number && (
          <>
            <dt>{t("shop:admin.creditNote")}</dt>
            <dd>{data.credit_note_number}</dd>
          </>
        )}
        {data.tracking_number && (
          <>
            <dt>{t("shop:tracking")}</dt>
            <dd>
              {data.carrier} {data.tracking_number}
            </dd>
          </>
        )}
      </dl>
      <table className="table">
        <tbody>
          {data.lines.map((l) => (
            <tr key={l.position}>
              <td>
                {l.title} <code className="small">{l.model_number}</code>
              </td>
              <td className="num">× {l.quantity}</td>
              <td className="num">
                <Money minor={l.unit_price_minor * l.quantity} currency={JPY} />
              </td>
            </tr>
          ))}
          <tr>
            <td>{t("shop:shipping")}</td>
            <td />
            <td className="num">
              <Money minor={data.shipping_minor} currency={JPY} />
            </td>
          </tr>
          <tr>
            <th>{t("shop:total")}</th>
            <td />
            <td className="num strong">
              <Money minor={data.total_minor} currency={JPY} />
            </td>
          </tr>
        </tbody>
      </table>
      <div className="card stack" data-testid="order-actions">
        {data.status === "placed" && (
          <>
            <p className="muted small">{t("shop:admin.confirmHint")}</p>
            <ConfirmButton
              label={t("shop:admin.confirm")}
              confirmLabel={t("shop:admin.confirmSure")}
              disabled={busy}
              onConfirm={() => void act({ action: ACTIONS.confirm })}
            />
          </>
        )}
        {data.status === "confirmed" && (
          <div className="row">
            <label>
              {t("shop:admin.paidOn")}
              <input type="date" value={paidOn} onChange={(e) => setPaidOn(e.target.value)} />
            </label>
            <button
              type="button"
              disabled={busy || !paidOn}
              onClick={() => void act({ action: ACTIONS.paid, paid_on: paidOn })}
            >
              {t("shop:admin.markPaid")}
            </button>
          </div>
        )}
        {data.status === "paid" && (
          <div className="row">
            <label>
              {t("shop:admin.carrier")}
              <input value={carrier} onChange={(e) => setCarrier(e.target.value)} />
            </label>
            <label>
              {t("shop:tracking")}
              <input value={tracking} onChange={(e) => setTracking(e.target.value)} />
            </label>
            <button
              type="button"
              disabled={busy || !carrier.trim() || !tracking.trim()}
              onClick={() => void act({ action: ACTIONS.ship, carrier, tracking_number: tracking })}
            >
              {t("shop:admin.ship")}
            </button>
          </div>
        )}
        {data.status === "shipped" && (
          <button type="button" disabled={busy} onClick={() => void act({ action: ACTIONS.complete })}>
            {t("shop:admin.complete")}
          </button>
        )}
        {["placed", "confirmed", "paid"].includes(data.status) && (
          <div className="row">
            <label>
              {t("shop:admin.cancelReason")}
              <input value={reason} onChange={(e) => setReason(e.target.value)} />
            </label>
            <ConfirmButton
              label={t("shop:admin.cancel")}
              confirmLabel={t("shop:admin.cancelSure")}
              disabled={busy || !reason.trim()}
              onConfirm={() => void act({ action: ACTIONS.cancel, reason })}
            />
          </div>
        )}
        <ErrorText code={actionError} />
      </div>
    </section>
  );
}

/** Admin console: consumer shop orders and the shop's rules. */
export function ShopAdmin() {
  const { t, i18n } = useTranslation();
  const [status, setStatus] = useState<string>(STATUSES[0]);
  const [open, setOpen] = useState<string | null>(null);
  const [settings, setSettings] = useState(false);
  const { data, error, reload } = useApi<ShopOrderSummary[]>(`${ADMIN_API}/shop/orders?status=${status}`);
  if (settings) return <Settings onDone={() => setSettings(false)} />;
  if (open)
    return (
      <OrderAdmin
        key={open}
        id={open}
        onBack={() => {
          setOpen(null);
          reload();
        }}
      />
    );
  return (
    <section>
      <PageHeader title={t("admin:nav.shop")}>
        <select
          aria-label={t("shop:admin.status")}
          value={status}
          onChange={(e) => setStatus(e.target.value)}
        >
          {STATUSES.map((s) => (
            <option key={s} value={s}>
              {t(`shop:status.${s}`)}
            </option>
          ))}
        </select>
        <button type="button" onClick={() => setSettings(true)}>
          {t("shop:admin.settings")}
        </button>
        <button type="button" onClick={reload}>
          {t("action.refresh")}
        </button>
      </PageHeader>
      <Loading data={data} error={error} />
      {data && data.length === 0 && <p className="muted">{t("shop:admin.none")}</p>}
      {data && data.length > 0 && (
        <table className="table" data-testid="shop-orders">
          <tbody>
            {data.map((o) => (
              <tr key={o.id}>
                <td>
                  <button type="button" className="link" onClick={() => setOpen(o.id)}>
                    {o.number}
                  </button>
                </td>
                <td>{o.ship_name}</td>
                <td>{formatDateTime(o.placed_at, i18n.language, PLATFORM_TZ)}</td>
                <td className="num">
                  <Money minor={o.total_minor} currency={JPY} />
                </td>
                <td>
                  <OrderStatus status={o.status} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
