// Shapes returned by the procurement API (crates/server/src/api).
// TODO(phase1-item6): replace with types generated from the OpenAPI document.

export type OrgKind = "platform" | "customer";

export interface MembershipView {
  org_id: string;
  org_name: string;
  org_kind: OrgKind;
  default_locale: string;
  timezone: string;
  role: string;
  permissions: string[];
}

export interface InvitationView {
  org_id: string;
  org_name: string;
  role: string;
}

export interface Me {
  user: {
    id: string;
    email: string;
    display_name: string;
    locale: string | null;
    account_kind: "internal" | "customer";
  };
  memberships: MembershipView[];
  invitations: InvitationView[];
}

export interface ApiResponse<T = unknown> {
  status: number;
  body: T | { error?: { code?: string; detail?: string } } | null;
  request_id: string;
}

export interface Prefs {
  language: string | null;
  server_url: string;
}

export interface MemberView {
  user_id: string;
  email: string;
  display_name: string;
  role: string;
  membership_status: string;
  user_status: string;
}

export interface AuditEvent {
  seq: number;
  actor_user_id: string | null;
  action: string;
  entity_type: string;
  entity_id: string | null;
  data: unknown;
  request_id: string | null;
  created_at: string;
}

export interface AuditResponse {
  events: AuditEvent[];
  chain_valid: boolean;
  first_broken_seq: number | null;
}

export interface OrgView {
  id: string;
  kind: OrgKind;
  name: string;
  default_locale: string;
  timezone: string;
  status: string;
}
