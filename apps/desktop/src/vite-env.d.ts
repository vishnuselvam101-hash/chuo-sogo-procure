/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** "admin" builds the Chuo-Sogo admin console instead of the business app. */
  readonly VITE_APP_KIND: "admin" | "shop" | "";
  /** "on" shows the supplier catalogue to customers (off: we buy for the factory). */
  readonly VITE_MARKETPLACE?: string;
}
