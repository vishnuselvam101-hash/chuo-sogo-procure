// API shapes come from the backend's OpenAPI document (api/openapi.json → api-schema.ts,
// regenerate with `pnpm gen:api`). Only the native-layer types below are hand-written.
import type { components } from "./api-schema";

type Schemas = components["schemas"];

export type MembershipView = Schemas["MembershipView"];
export type InvitationView = Schemas["InvitationView"];
export type Me = Schemas["MeResponse"];
export type MemberView = Schemas["MemberView"];
export type AuditEvent = Schemas["AuditView"];
export type AuditResponse = Schemas["AuditResponse"];
export type OrgView = Schemas["OrgView"];
export type ErrorBody = Schemas["ErrorBody"];
export type RequestSummary = Schemas["RequestSummary"];
export type RequestDetail = Schemas["RequestDetail"];
export type ItemView = Schemas["ItemView"];
export type ItemInput = Schemas["ItemInput"];
export type RfqDetail = Schemas["RfqDetail"];
export type ComparisonView = Schemas["ComparisonView"];
export type ComparisonResult = Schemas["ComparisonResult"];
export type RecommendationDetail = Schemas["RecommendationDetail"];
export type OutboxMessage = Schemas["OutboxMessage"];
export type SupplierView = Schemas["SupplierView"];
export type OrgSupplierView = Schemas["OrgSupplierView"];
export type PolicyView = Schemas["PolicyView"];
export type TierView = Schemas["TierView"];
export type Created = Schemas["Created"];
export type AttachmentView = Schemas["AttachmentView"];
export type ProductView = Schemas["ProductView"];
export type ProductDetail = Schemas["ProductDetail"];

/** What the Rust layer's `api_request`/`auth_login` commands return. */
export interface ApiResponse<T = unknown> {
  status: number;
  body: T | ErrorBody | null;
  request_id: string;
}

/** Local preferences held by the Rust layer (apps/desktop/src-tauri/src/prefs.rs). */
export interface Prefs {
  language: string | null;
  server_url: string;
}
export type InvoiceSummary = Schemas["InvoiceSummary"];
export type IssuedInvoice = Schemas["InvoiceDetail"];
export type InvoiceLineInput = Schemas["LineInput"];
export type IssuerProfile = Schemas["IssuerProfile"];
export type ListingSummary = Schemas["ListingSummary"];
export type ListingDetail = Schemas["ListingDetail"];
export type ListingInput = Schemas["ListingInput"];
export type ListingMedia = Schemas["MediaView"];
