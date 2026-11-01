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
  | "listings"
  | "catalogue"
  | "reports"
  | "opsQueue"
  | "organisations"
  | "members"
  | "audit"
  | "settings";

export interface NavItem {
  id: PageId;
  /** Key in the `common` namespace (`nav.*`). */
  labelKey: string;
  /** False = module arrives in a later phase; shown honestly as such. */
  implemented: boolean;
}

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
    // Staff working inside a customer keep their work queue one click away.
    if (m.access === "assigned" && has("operate_requests")) items.push(item("opsQueue", true));
    if (m.access !== "assigned" && has("view_catalogue")) items.push(item("catalogue", true));
    if (has("approve_recommendation") || has("manage_approval_policy")) items.push(item("approvals", true));
    if (has("view_orders")) items.push(item("orders", true));
    if (has("view_finance")) items.push(item("invoices", true));
    // Chuo-Sogo's own invoices to this customer (customer members only; staff use the console).
    if (has("view_finance") && m.access !== "assigned") items.push(item("billing", true));
    if (has("view_reports")) items.push(item("reports"));
    if (has("manage_org_users")) items.push(item("members", true));
  }
  if (has("view_audit")) items.push(item("audit", true));
  items.push(item("settings", true));
  return items;
}
