// Permission codes as returned by the backend (procure_domain::rbac::Permission::code).
// Presentation only: they decide which controls are shown; the backend checks every call.
export const P = {
  submitRequest: "submit_request",
  verifyProducts: "verify_products",
  manageRfqs: "manage_rfqs",
  recordQuotations: "record_quotations",
  reviewQuotations: "review_quotations",
  operateRequests: "operate_requests",
  approveRecommendation: "approve_recommendation",
  approveSuppliers: "approve_suppliers",
  manageApprovalPolicy: "manage_approval_policy",
  manageSuppliers: "manage_suppliers",
  manageProducts: "manage_products",
  viewInternalNotes: "view_internal_notes",
} as const;

export const AWAITING_APPROVAL = "awaiting_approval";
