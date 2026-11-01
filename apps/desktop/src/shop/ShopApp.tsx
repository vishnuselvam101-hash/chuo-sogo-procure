import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { DEFAULT_LANGUAGE, isLanguage, setDocumentLanguage, type LanguageCode } from "../i18n";
import { useBridge } from "../lib/BridgeContext";
import { ok } from "../lib/bridge";
import type { Me, Prefs, ShopSettings } from "../lib/types";
import { LanguageMenu } from "../components/LanguageMenu";
import { ShopAuth } from "./ShopAuth";
import { Account, Cart, Legal, Orders, Product, Products } from "./ShopPages";
import { loadCart, saveCart, type CartLine } from "./cart";

type Page = "products" | "cart" | "orders" | "account" | "legal";
const PAGES: Page[] = ["products", "cart", "orders", "account", "legal"];
const CONSUMER = "consumer";
const ORDERS: Page = "orders";
const PRODUCTS: Page = "products";

/** Chuo-Sogo Shop: the consumer app (phone and desktop). */
export function ShopApp() {
  const { t, i18n } = useTranslation();
  const bridge = useBridge();
  const [prefs, setPrefs] = useState<Prefs | null>(null);
  const [me, setMe] = useState<Me | null>(null);
  const [settings, setSettings] = useState<ShopSettings | null>(null);
  const [page, setPage] = useState<Page>("products");
  const [product, setProduct] = useState<string | null>(null);
  const [order, setOrder] = useState<string | null>(null);
  const [cart, setCartState] = useState<CartLine[]>([]);
  const [legalOnly, setLegalOnly] = useState(false);

  useEffect(() => {
    void bridge.prefsGet().then(setPrefs);
  }, [bridge]);
  const serverUrl = prefs?.server_url;
  useEffect(() => {
    if (!serverUrl) return;
    void bridge.api<ShopSettings>("GET", "/api/v1/shop/info").then((res) => ok(res) && setSettings(res.body));
  }, [bridge, serverUrl, me]);

  const account = me?.memberships.find((m) => m.org_kind === CONSUMER) ?? null;
  const language: LanguageCode = isLanguage(prefs?.language) ? prefs.language : DEFAULT_LANGUAGE;
  useEffect(() => {
    if (i18n.language !== language) void i18n.changeLanguage(language);
    setDocumentLanguage(language);
  }, [i18n, language]);

  const orgId = account?.org_id;
  const setCart = useCallback(
    (lines: CartLine[]) => {
      setCartState(lines);
      if (orgId) saveCart(orgId, lines);
    },
    [orgId],
  );

  const chooseLanguage = useCallback(
    async (code: LanguageCode | null) => {
      await bridge.prefsSetLanguage(code);
      setPrefs((p) => (p ? { ...p, language: code } : p));
    },
    [bridge],
  );

  if (!prefs) return null;
  if (legalOnly) return <Legal settings={settings} onBack={() => setLegalOnly(false)} />;
  if (!me)
    return (
      <ShopAuth
        serverUrl={prefs.server_url}
        onSignedIn={(m) => {
          setMe(m);
          const own = m.memberships.find((x) => x.org_kind === CONSUMER);
          // The cart kept on this device for this account.
          setCartState(own ? loadCart(own.org_id) : []);
        }}
        onLegal={() => setLegalOnly(true)}
      />
    );
  if (!account)
    return (
      <main className="login">
        <div className="card login-card">
          <p>{t("shop:notConsumer")}</p>
          <button type="button" onClick={() => void bridge.logout().then(() => setMe(null))}>
            {t("auth.signOut")}
          </button>
        </div>
      </main>
    );

  const base = `/api/v1/orgs/${account.org_id}/shop`;
  const count = cart.reduce((n, l) => n + l.quantity, 0);
  let body: React.ReactNode;
  if (page === "products" && product)
    body = <Product base={base} id={product} cart={cart} onCart={setCart} onBack={() => setProduct(null)} />;
  else if (page === "products") body = <Products base={base} onOpen={setProduct} />;
  else if (page === "cart")
    body = (
      <Cart
        base={base}
        cart={cart}
        onCart={setCart}
        settings={settings}
        onOrdered={(id) => {
          setOrder(id);
          setPage(ORDERS);
        }}
      />
    );
  else if (page === "orders")
    body = <Orders base={base} open={order} onOpen={setOrder} settings={settings} />;
  else if (page === "account")
    body = (
      <Account
        base={base}
        onClosed={() => {
          if (orgId) saveCart(orgId, []);
          setMe(null);
          setPage(PRODUCTS);
        }}
      />
    );
  else body = <Legal settings={settings} />;

  return (
    <div className="shop-shell">
      <header className="shop-header">
        <strong className="shop-brand">{t("shop:name")}</strong>
        <nav className="shop-nav" aria-label={t("shop:name")}>
          {PAGES.map((p) => (
            <button
              key={p}
              type="button"
              className={`nav-item${page === p ? " active" : ""}`}
              aria-current={page === p ? "page" : undefined}
              onClick={() => {
                setPage(p);
                setProduct(null);
                if (p !== "orders") setOrder(null);
              }}
            >
              {t(`shop:nav.${p}`)}
              {p === "cart" && count > 0 && <span className="badge cart-count">{count}</span>}
            </button>
          ))}
        </nav>
        <div className="row">
          <LanguageMenu current={language} onChange={(c) => void chooseLanguage(c)} />
          <button type="button" className="link" onClick={() => void bridge.logout().then(() => setMe(null))}>
            {t("auth.signOut")}
          </button>
        </div>
      </header>
      {settings && !settings.open && <p className="notice shop-closed">{t("shop:closed")}</p>}
      <main className="content">{body}</main>
    </div>
  );
}
