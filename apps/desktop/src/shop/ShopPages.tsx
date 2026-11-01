import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { useApi } from "../lib/useApi";
import { useAction } from "../lib/useAction";
import { formatDate, formatDateTime } from "../lib/format";
import type {
  ListingDetail,
  ListingSummary,
  ShippingAddress,
  ShopOrder,
  ShopOrderSummary,
  ShopSettings,
} from "../lib/types";
import { ErrorText, Loading, Money, PageHeader } from "../components/ui";
import { ListingPhoto, ListingView } from "../catalogue/ListingView";
import { PdfButton } from "../billing/InvoiceView";
import { addToCart, expectedTotal, type CartLine } from "./cart";

const JPY = "JPY";
const INVOICE = "invoice";
const CREDIT_NOTE = "credit_note";
const ISSUED = "issued";
const TZ = "Asia/Tokyo";

export function OrderStatus({ status }: { status: string }) {
  const { t } = useTranslation();
  return <span className={`badge badge-order-${status}`}>{t(`shop:status.${status}`)}</span>;
}

/** Product grid with search. */
export function Products({ base, onOpen }: { base: string; onOpen: (id: string) => void }) {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const [search, setSearch] = useState("");
  const { data, error } = useApi<ListingSummary[]>(
    search ? `${base}/products?q=${encodeURIComponent(search)}` : `${base}/products`,
  );
  return (
    <section>
      <PageHeader title={t("shop:nav.products")}>
        <form
          className="row"
          onSubmit={(e) => {
            e.preventDefault();
            setSearch(query.trim());
          }}
        >
          <input
            type="search"
            maxLength={100}
            aria-label={t("shop:search")}
            placeholder={t("shop:search")}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <button type="submit">{t("shop:find")}</button>
        </form>
      </PageHeader>
      <Loading data={data} error={error} />
      {data && data.length === 0 && <p className="muted">{t("shop:noProducts")}</p>}
      <div className="card-grid" data-testid="shop-products">
        {data?.map((l) => (
          <button type="button" key={l.id} className="card product-card" onClick={() => onOpen(l.id)}>
            {l.cover_media_id && (
              <ListingPhoto
                path={`${base}/products/${l.id}/media/${l.cover_media_id}/content`}
                alt=""
                className="photo"
              />
            )}
            <strong>{l.title || l.model_number}</strong>
            <span className="muted small">
              {l.manufacturer} · <code>{l.model_number}</code>
            </span>
            <span className="price">
              <Money minor={l.consumer_price_minor ?? 0} currency={JPY} />{" "}
              <span className="small">{t("shop:taxIncl")}</span>
            </span>
          </button>
        ))}
      </div>
    </section>
  );
}

/** Product page with "add to cart". */
export function Product({
  base,
  id,
  cart,
  onCart,
  onBack,
}: {
  base: string;
  id: string;
  cart: CartLine[];
  onCart: (lines: CartLine[]) => void;
  onBack: () => void;
}) {
  const { t } = useTranslation();
  const { data, error } = useApi<ListingDetail>(`${base}/products/${id}`);
  const [qty, setQty] = useState(1);
  const [added, setAdded] = useState(false);
  if (!data) return <Loading data={data} error={error} />;
  const price = data.consumer_price_minor ?? 0;
  const maxQty = Math.min(1000, data.stock ?? 1000);
  return (
    <section>
      <PageHeader title={data.title || data.model_number}>
        <button type="button" onClick={onBack}>
          {t("action.back")}
        </button>
      </PageHeader>
      <div className="card row buy-box">
        <span className="price big">
          <Money minor={price} currency={JPY} /> <span className="small">{t("shop:taxIncl")}</span>
        </span>
        {maxQty > 0 ? (
          <>
            <label>
              {t("shop:quantity")}
              <input
                type="number"
                min={1}
                max={maxQty}
                value={qty}
                onChange={(e) =>
                  setQty(Math.max(1, Math.min(maxQty, Number.parseInt(e.target.value, 10) || 1)))
                }
              />
            </label>
            <button
              type="button"
              className="primary"
              onClick={() => {
                onCart(
                  addToCart(cart, {
                    listing_id: data.id,
                    title: data.title || data.title_en || data.model_number,
                    model_number: data.model_number,
                    unit_price_minor: price,
                    quantity: qty,
                  }),
                );
                setAdded(true);
              }}
            >
              {t("shop:addToCart")}
            </button>
            {added && (
              <span className="ok" role="status">
                {t("shop:added")}
              </span>
            )}
          </>
        ) : (
          <span className="warn-text">{t("shop:soldOut")}</span>
        )}
      </div>
      <ListingView l={data} mediaPath={(m) => `${base}/products/${id}/media/${m}/content`} />
    </section>
  );
}

const PREFECTURE_HINT = "東京都";

/** Cart and checkout (address, bank transfer). */
export function Cart({
  base,
  cart,
  onCart,
  settings,
  onOrdered,
}: {
  base: string;
  cart: CartLine[];
  onCart: (lines: CartLine[]) => void;
  settings: ShopSettings | null;
  onOrdered: (orderId: string) => void;
}) {
  const { t } = useTranslation();
  const { run, busy, error } = useAction();
  const [ship, setShip] = useState<ShippingAddress>({
    name: "",
    postal_code: "",
    prefecture: "",
    address_line: "",
    phone: "",
  });
  const [note, setNote] = useState("");
  const totals = expectedTotal(cart, settings?.shipping_fee_minor ?? 0, settings?.free_shipping_over_minor);
  const set = (k: keyof ShippingAddress) => (e: { target: { value: string } }) =>
    setShip((s) => ({ ...s, [k]: e.target.value }));

  async function order(e: FormEvent) {
    e.preventDefault();
    const res = await run<{ id: string }>("POST", `${base}/orders`, {
      lines: cart.map((l) => ({ listing_id: l.listing_id, quantity: l.quantity })),
      ship_to: ship,
      note,
      expected_total_minor: totals.total,
    });
    if (res?.body) {
      onCart([]);
      onOrdered(res.body.id);
    }
  }

  if (cart.length === 0)
    return (
      <section>
        <PageHeader title={t("shop:nav.cart")} />
        <p className="muted">{t("shop:cartEmpty")}</p>
      </section>
    );

  return (
    <section>
      <PageHeader title={t("shop:nav.cart")} />
      <table className="table" data-testid="cart">
        <thead>
          <tr>
            <th>{t("shop:product")}</th>
            <th className="num">{t("shop:unitPrice")}</th>
            <th className="num">{t("shop:quantity")}</th>
            <th className="num">{t("shop:amount")}</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {cart.map((l) => (
            <tr key={l.listing_id}>
              <td>
                {l.title} <code className="small">{l.model_number}</code>
              </td>
              <td className="num">
                <Money minor={l.unit_price_minor} currency={JPY} />
              </td>
              <td className="num">
                <input
                  type="number"
                  min={1}
                  max={1000}
                  aria-label={t("shop:quantity")}
                  value={l.quantity}
                  onChange={(e) =>
                    onCart(
                      cart.map((x) =>
                        x.listing_id === l.listing_id
                          ? {
                              ...x,
                              quantity: Math.max(1, Math.min(1000, Number.parseInt(e.target.value, 10) || 1)),
                            }
                          : x,
                      ),
                    )
                  }
                />
              </td>
              <td className="num">
                <Money minor={l.unit_price_minor * l.quantity} currency={JPY} />
              </td>
              <td>
                <button
                  type="button"
                  className="link"
                  onClick={() => onCart(cart.filter((x) => x.listing_id !== l.listing_id))}
                >
                  {t("shop:remove")}
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <dl className="facts card">
        <dt>{t("shop:itemsTotal")}</dt>
        <dd>
          <Money minor={totals.items} currency={JPY} />
        </dd>
        <dt>{t("shop:shipping")}</dt>
        <dd>
          <Money minor={totals.shipping} currency={JPY} />
          {settings?.free_shipping_over_minor !== null &&
            settings?.free_shipping_over_minor !== undefined && (
              <span className="muted small">
                {" "}
                ({t("shop:freeOver")} <Money minor={settings.free_shipping_over_minor} currency={JPY} />)
              </span>
            )}
        </dd>
        <dt>{t("shop:total")}</dt>
        <dd className="strong" data-testid="cart-total">
          <Money minor={totals.total} currency={JPY} /> <span className="small">{t("shop:taxIncl")}</span>
        </dd>
      </dl>
      {!settings?.open && <p className="warn-text">{t("shop:closed")}</p>}
      <form className="card form-grid" onSubmit={order} data-testid="checkout">
        <h2>{t("shop:shipTo")}</h2>
        <label>
          {t("shop:addr.name")}
          <input required maxLength={100} autoComplete="name" value={ship.name} onChange={set("name")} />
        </label>
        <label>
          {t("shop:addr.postal")}
          <input
            required
            inputMode="numeric"
            autoComplete="postal-code"
            placeholder="100-0001"
            value={ship.postal_code}
            onChange={set("postal_code")}
          />
        </label>
        <label>
          {t("shop:addr.prefecture")}
          <input
            required
            maxLength={10}
            autoComplete="address-level1"
            placeholder={PREFECTURE_HINT}
            value={ship.prefecture}
            onChange={set("prefecture")}
          />
        </label>
        <label>
          {t("shop:addr.line")}
          <input
            required
            maxLength={300}
            autoComplete="street-address"
            value={ship.address_line}
            onChange={set("address_line")}
          />
        </label>
        <label>
          {t("shop:addr.phone")}
          <input required type="tel" autoComplete="tel" value={ship.phone} onChange={set("phone")} />
        </label>
        <label>
          {t("shop:note")}
          <input maxLength={1000} value={note} onChange={(e) => setNote(e.target.value)} />
        </label>
        <p className="muted small">{t("shop:paymentHint")}</p>
        <ErrorText code={error} />
        <button type="submit" className="primary" disabled={busy || !settings?.open}>
          {t("shop:placeOrder")}
        </button>
      </form>
    </section>
  );
}

function OrderPage({
  base,
  id,
  onBack,
  settings,
}: {
  base: string;
  id: string;
  onBack: () => void;
  settings: ShopSettings | null;
}) {
  const { t, i18n } = useTranslation();
  const { data, error, reload } = useApi<ShopOrder>(`${base}/orders/${id}`);
  const { run, busy, error: actionError } = useAction();
  if (!data) return <Loading data={data} error={error} />;
  const when = (iso: string | null | undefined) => (iso ? formatDateTime(iso, i18n.language, TZ) : null);
  const steps: [string, string | null | undefined][] = [
    ["placed", data.placed_at],
    ["confirmed", data.confirmed_at],
    ["paid", data.paid_at],
    ["shipped", data.shipped_at],
    ["completed", data.completed_at],
  ];
  return (
    <section>
      <PageHeader title={data.number}>
        <OrderStatus status={data.status} />
        <button type="button" onClick={onBack}>
          {t("action.back")}
        </button>
      </PageHeader>
      {data.status === "confirmed" && data.invoice_number && (
        <div className="notice stack" data-testid="pay-instructions">
          <strong>{t("shop:payNow", { total: data.total_minor.toLocaleString(i18n.language) })}</strong>
          <span>
            {t("shop:payBy", {
              invoice: data.invoice_number,
              due: data.invoice_due_date ? formatDate(data.invoice_due_date, i18n.language) : "",
            })}
          </span>
          {settings?.payment_info && <span className="pre small">{settings.payment_info}</span>}
        </div>
      )}
      {data.status === "cancelled" && (
        <p className="warn-text">{t("shop:cancelledBecause", { reason: data.cancel_reason || "—" })}</p>
      )}
      <ol className="timeline" data-testid="order-timeline">
        {steps.map(([s, at]) => (
          <li key={s} className={at ? "done" : ""}>
            {t(`shop:status.${s}`)} {at && <span className="muted small">{when(at)}</span>}
          </li>
        ))}
      </ol>
      {data.tracking_number && (
        <p>
          {t("shop:tracking")}: {data.carrier} <code>{data.tracking_number}</code>
        </p>
      )}
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
            <td className="small muted">
              {t("shop:taxContained", { tax: data.tax_minor.toLocaleString(i18n.language) })}
            </td>
            <td className="num strong">
              <Money minor={data.total_minor} currency={JPY} />
            </td>
          </tr>
        </tbody>
      </table>
      <p className="small muted">
        {`${data.ship_to.name} · 〒${data.ship_to.postal_code} ${data.ship_to.prefecture}${data.ship_to.address_line} · ${data.ship_to.phone}`}
      </p>
      <div className="row">
        {data.invoice_id && data.invoice_number && (
          <PdfButton
            doc={{
              id: data.invoice_id,
              org_id: data.org_id,
              number: data.invoice_number,
              org_name: "",
              kind: INVOICE,
              status: ISSUED,
              original_id: null,
              issue_date: null,
              due_date: null,
              currency: JPY,
              total_minor: data.total_minor,
              overdue: false,
              created_at: data.placed_at,
            }}
          />
        )}
        {data.credit_note_id && data.credit_note_number && (
          <PdfButton
            doc={{
              id: data.credit_note_id,
              org_id: data.org_id,
              number: data.credit_note_number,
              org_name: "",
              kind: CREDIT_NOTE,
              status: ISSUED,
              original_id: data.invoice_id ?? null,
              issue_date: null,
              due_date: null,
              currency: JPY,
              total_minor: data.total_minor,
              overdue: false,
              created_at: data.placed_at,
            }}
          />
        )}
        {data.status === "placed" && (
          <button
            type="button"
            disabled={busy}
            onClick={async () => {
              if (await run("POST", `${base}/orders/${id}/cancel`, {})) reload();
            }}
          >
            {t("shop:cancelOrder")}
          </button>
        )}
      </div>
      <ErrorText code={actionError} />
    </section>
  );
}

export function Orders({
  base,
  open,
  onOpen,
  settings,
}: {
  base: string;
  open: string | null;
  onOpen: (id: string | null) => void;
  settings: ShopSettings | null;
}) {
  const { t, i18n } = useTranslation();
  const { data, error } = useApi<ShopOrderSummary[]>(`${base}/orders`);
  if (open)
    return <OrderPage key={open} base={base} id={open} settings={settings} onBack={() => onOpen(null)} />;
  return (
    <section>
      <PageHeader title={t("shop:nav.orders")} />
      <Loading data={data} error={error} />
      {data && data.length === 0 && <p className="muted">{t("shop:noOrders")}</p>}
      {data && data.length > 0 && (
        <table className="table" data-testid="my-orders">
          <tbody>
            {data.map((o) => (
              <tr key={o.id}>
                <td>
                  <button type="button" className="link" onClick={() => onOpen(o.id)}>
                    {o.number}
                  </button>
                </td>
                <td>{formatDateTime(o.placed_at, i18n.language, TZ)}</td>
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

/** 特定商取引法に基づく表記, payment, delivery, returns and privacy (public). */
export function Legal({ settings, onBack }: { settings: ShopSettings | null; onBack?: () => void }) {
  const { t } = useTranslation();
  if (!settings) return null;
  const rows: [string, string][] = [
    ["seller", settings.seller_name],
    ["responsible", settings.responsible_person],
    ["address", settings.seller_address],
    ["phone", settings.seller_phone],
    ["email", settings.seller_email],
    ["price", settings.price_info],
    ["payment", settings.payment_info],
    ["delivery", settings.delivery_info],
    ["returns", settings.returns_policy],
  ];
  return (
    <section className="content">
      <PageHeader title={t("shop:nav.legal")}>
        {onBack && (
          <button type="button" onClick={onBack}>
            {t("action.back")}
          </button>
        )}
      </PageHeader>
      <h2>{t("shop:legal.title")}</h2>
      <dl className="facts" data-testid="legal-notice">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt>{t(`shop:legal.${k}`)}</dt>
            <dd className="pre">{v || "—"}</dd>
          </div>
        ))}
      </dl>
      <h2>{t("shop:legal.privacy")}</h2>
      <p className="pre">{settings.privacy_policy || "—"}</p>
    </section>
  );
}

const DELETE_WORD = "DELETE";

/** The consumer's own data, and closing the account. */
export function Account({ base, onClosed }: { base: string; onClosed: () => void }) {
  const { t } = useTranslation();
  const { run, busy, error } = useAction();
  const [data, setData] = useState<unknown>(null);
  const [confirm, setConfirm] = useState("");
  return (
    <section>
      <PageHeader title={t("shop:nav.account")} />
      <div className="card stack">
        <h2>{t("shop:account.dataTitle")}</h2>
        <p className="muted small">{t("shop:account.dataHint")}</p>
        <div className="row">
          <button
            type="button"
            disabled={busy}
            onClick={async () => {
              const res = await run("GET", `${base}/my-data`);
              if (res) setData(res.body);
            }}
          >
            {t("shop:account.showData")}
          </button>
        </div>
        {data !== null && (
          <pre className="small wrap" data-testid="my-data">
            {JSON.stringify(data, null, 2)}
          </pre>
        )}
      </div>
      <div className="card stack">
        <h2>{t("shop:account.closeTitle")}</h2>
        <p className="muted small">{t("shop:account.closeHint")}</p>
        <label>
          {t("shop:account.typeDelete")}
          <input value={confirm} onChange={(e) => setConfirm(e.target.value)} />
        </label>
        <button
          type="button"
          className="danger"
          disabled={busy || confirm !== DELETE_WORD}
          onClick={async () => {
            if (await run("POST", `${base}/close-account`, { confirm })) onClosed();
          }}
        >
          {t("shop:account.close")}
        </button>
        <ErrorText code={error} />
      </div>
    </section>
  );
}
