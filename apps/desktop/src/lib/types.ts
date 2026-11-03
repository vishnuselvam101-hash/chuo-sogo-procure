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
/** A quoted line next to the customer's last purchase of the item (reference only). */
export type PriceCheck = Schemas["PriceCheck"];
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
export type HistoryPreview = Schemas["PreviewView"];
export type HistoryImport = Schemas["ImportView"];
export type HistoryRow = Schemas["HistoryRowView"];
export type ItemReference = Schemas["ItemReferenceView"];
export type SimilarItem = Schemas["SimilarItemView"];
export type ItemAlias = Schemas["AliasView"];
export type BuyingCriteria = Schemas["CriteriaView"];
export type BuyingCriteriaInput = Schemas["CriteriaInput"];

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
  /** Reading comfort; older prefs files and test fakes may leave these out. */
  text_size?: TextSize;
  high_contrast?: boolean;
  /** "server" or "local"; null only on a fresh install that offers "use on this PC only". */
  mode?: "server" | "local" | null;
}

/** Standalone mode ("use on this PC only"), from the native layer. */
export interface LocalStatus {
  available: boolean;
  phase: "off" | "starting" | "running" | "failed";
  error: string | null;
  server_url: string | null;
  needs_setup: boolean;
  ai: "off" | "starting" | "ready" | "failed";
  voice: "ready" | "no_model" | "no_ffmpeg";
  ocr: "ready" | "no_data" | "no_tesseract" | "no_pdf_tools";
}

export interface PackVariant {
  variant: string;
  title_en: string;
  title_ja: string;
  min_ram_gb: number;
  size: number;
  installed: boolean;
}

export interface PackInfo {
  id: string;
  kind: "core" | "large";
  version: string;
  title_en: string;
  title_ja: string;
  licence: string;
  licence_url: string | null;
  size: number;
  variants: PackVariant[];
  status: "installed" | "missing" | "installing" | "failed";
  done: number;
  total: number;
  error: string | null;
}

export interface PacksStatus {
  platform: string;
  ram_bytes: number;
  free_bytes: number;
  recommended_variant: string | null;
  packs: PackInfo[];
}

export type TextSize = "normal" | "large" | "xlarge";
export type InvoiceSummary = Schemas["InvoiceSummary"];
export type SupplierShortlist = Schemas["ShortlistView"];
export type OpportunityView = Schemas["OpportunityView"];
export type RecordList = Schemas["RecordList"];
export type MailStatus = Schemas["MailStatus"];
export type InboxMessage = Schemas["InboxMessage"];
export type SavingsView = Schemas["SavingsView"];
export type IssuedInvoice = Schemas["InvoiceDetail"];
export type InvoiceLineInput = Schemas["LineInput"];
export type IssuerProfile = Schemas["IssuerProfile"];
export type ListingSummary = Schemas["ListingSummary"];
export type ListingDetail = Schemas["ListingDetail"];
export type ListingInput = Schemas["ListingInput"];
export type ListingMedia = Schemas["MediaView"];
export type ShopSettings = Schemas["ShopSettings"];
export type ShopOrderSummary = Schemas["ShopOrderSummary"];
export type ShopOrder = Schemas["ShopOrderDetail"];
export type ShippingAddress = Schemas["ShippingAddress"];
