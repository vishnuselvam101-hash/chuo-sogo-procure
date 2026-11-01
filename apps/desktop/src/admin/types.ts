// Admin API shapes, generated from the backend's OpenAPI document.
import type { components } from "../lib/api-schema";

type Schemas = components["schemas"];

export type AdminMe = Schemas["AdminMe"];
export type OrgOverview = Schemas["OrgOverview"];
export type OrgSettings = Schemas["OrgSettings"];
export type AccountView = Schemas["AccountView"];
export type AssignmentView = Schemas["AssignmentView"];
export type SystemHealth = Schemas["SystemHealth"];
export type OrgHealth = Schemas["OrgHealth"];
export type CreateOrgRequest = Schemas["CreateOrgRequest"];

export const ADMIN_API = "/api/admin/v1";

/** Permission codes the console checks before showing a section (the server enforces them). */
export const PERM = {
  manageOrgs: "manage_organisations",
  manageInternalUsers: "manage_internal_users",
  manageAssignments: "manage_service_assignments",
  viewHealth: "view_system_health",
  viewAudit: "view_audit",
  manageBilling: "manage_billing",
  moderateListings: "moderate_listings",
  manageShop: "manage_shop",
} as const;

/** Chuo-Sogo works in Japan time; staff-wide views use it. */
export const PLATFORM_TZ = "Asia/Tokyo";

/** State codes sent to the server (labels come from `admin:status.*`). */
export const STATE = {
  active: "active",
  suspended: "suspended",
  deactivated: "deactivated",
  locked: "locked",
} as const;

export const NOT_FOUND = "not_found";
