import type { MembershipView } from "./types";

export type PageId =
  | "dashboard"
  | "procurement"
  | "products"
  | "suppliers"
  | "rfqs"
  | "quotations"
  | "approvals"
  | "orders"
  | "invoices"
  | "billing"
  | "savings"
  | "history"
  | "records"
  | "listings"
  | "catalogue"
  | "reports"
  | "opsQueue"
  | "mailInbox"
  | "organisations"
  | "members"
  | "audit"
  | "settings"
  | "agents"
  | "exceptions"
  | "cost"
  | "integrations";

export interface NavItem {
  id: PageId;
  /** Key in the `common` namespace (`nav.*`). */
  labelKey: string;
  /** False = module arrives in a later phase; shown honestly as such. */
  implemented: boolean;
}

const MARKETPLACE = import.meta.env.VITE_MARKETPLACE === "on";

const item = (id: PageId, implemented = false): NavItem => ({ id, labelKey: `nav.${id}`, implemented });

/**
 * Navigation shaped by the permissions the backend returned for the active organisation.
 * Presentation only: the backend still checks every request.
 */
export function navFor(m: MembershipView): NavItem[] {
  const has = (p: string) => m.permissions.includes(p);
  const items: NavItem[] = [item("dashboard", true)];
  if (m.org_kind === "platform") {
    items.push(item("opsQueue", true), item("products", true));
    if (has("manage_suppliers")) items.push(item("suppliers", true));
    items.push(item("rfqs"), item("quotations"));
    if (has("view_reports")) items.push(item("reports"));
    if (has("manage_organisations")) items.push(item("organisations", true));
    if (has("manage_internal_users")) items.push(item("members", true));
  } else if (m.org_kind === "supplier") {
    if (has("manage_listings")) items.push(item("listings", true));
    if (has("manage_org_users")) items.push(item("members", true));
  } else {
    if (has("submit_request") || has("view_all_requests")) items.push(item("procurement", true));
    if (has("approve_suppliers") || has("manage_rfqs")) items.push(item("suppliers", true));
    // Past purchases and buying criteria: the decision material behind every request.
    if (has("manage_purchase_history") || has("view_all_requests")) items.push(item("history", true));
    // Staff working inside a customer keep their work queue one click away.
    if (m.access === "assigned" && has("operate_requests")) items.push(item("opsQueue", true));
    // Supplier replies (FAXes, new threads) the server could not match to an RFQ.
    if (m.access === "assigned" && has("record_quotations")) items.push(item("mailInbox", true));
    // We buy for the factory; we are not a marketplace. The supplier catalogue stays off
    // unless a build turns it on (VITE_MARKETPLACE=on).
    if (MARKETPLACE && m.access !== "assigned" && has("view_catalogue")) items.push(item("catalogue", true));
    if (has("approve_recommendation") || has("manage_approval_policy")) items.push(item("approvals", true));
    // Agents: their exceptions wait for a person; the console shows runs and settings.
    if (has("view_agents")) items.push(item("exceptions", true), item("agents", true));
    // Should-cost models and negotiation briefs sit next to the price records they use.
    if (has("manage_purchase_history") || has("view_all_requests")) items.push(item("cost", true));
    if (has("view_orders")) items.push(item("orders", true));
    if (has("view_finance")) items.push(item("invoices", true));
    // Chuo-Sogo's own invoices to this customer (customer members only; staff use the console).
    if (has("view_finance") && m.access !== "assigned") items.push(item("billing", true));
    // How we are paid: half of what each price went down, line by line.
    if (has("view_finance") || has("view_all_requests")) items.push(item("savings", true));
    // Electronic transaction records (電子帳簿保存法): quotations, orders, invoices.
    if (has("view_finance") || has("view_all_requests")) items.push(item("records", true));
    // Files from the factory's own systems: POs, receipts, supplier invoices.
    if (has("record_orders") || has("receive_goods") || has("manage_invoices"))
      items.push(item("integrations", true));
    if (has("view_reports")) items.push(item("reports"));
    if (has("manage_org_users")) items.push(item("members", true));
  }
  if (has("view_audit")) items.push(item("audit", true));
  items.push(item("settings", true));
  return items;
}
