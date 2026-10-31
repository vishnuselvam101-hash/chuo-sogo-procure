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
