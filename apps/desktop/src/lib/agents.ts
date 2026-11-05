// Presentation helpers for the agent layer. Labels only: the server decides what an agent
// may do, which action an exception carries and who may approve it.
import type { TFunction } from "i18next";
import type { components } from "./api-schema";
import { formatDate, formatMoney } from "./format";

type Schemas = components["schemas"];
export type AgentSummary = Schemas["AgentSummary"];
export type AgentRun = Schemas["AgentRun"];
export type AgentException = Schemas["AgentException"];
export type AgentSettings = Schemas["AgentSettings"];
export type StaffException = Schemas["StaffException"];
export type ChaseSettings = Schemas["ChaseSettings"];
export type MatchTolerancesView = Schemas["MatchTolerancesView"];
export type SuggestionView = Schemas["SuggestionView"];
export type QuotationFacts = Schemas["QuotationFacts"];
export type BriefView = Schemas["BriefView"];
export type CostModelView = Schemas["CostModelView"];
export type CostModelInput = Schemas["CostModelInput"];
export type ImportBatchView = Schemas["ImportBatchView"];
export type ProfileView = Schemas["ProfileView"];
export type ProfileConfig = Schemas["ProfileConfig"];
export type SupplierScorecard = Schemas["SupplierScorecard"];
export type CustomerComplianceDoc = Schemas["CustomerComplianceDoc"];
export type StaffComplianceDoc = Schemas["StaffComplianceDoc"];

/** Agents in the server's registry (`agent_runs.agent`). */
export const AGENTS = ["matching", "chasing", "cost_analyst", "supplier_watch", "import"] as const;
/** Agents that sweep the whole organisation, so a person can start one by hand. */
export const SWEEP_AGENTS = ["matching", "chasing", "supplier_watch"] as const;
export const SEVERITIES = ["critical", "warning", "info"] as const;
export const RUN_STATUSES = ["queued", "running", "succeeded", "failed", "skipped"] as const;
export const EXCEPTION_STATUSES = ["open", "approved", "edited", "rejected", "resolved_elsewhere"] as const;
export const IMPORT_KINDS = ["po", "receipt", "invoice"] as const;
export const DOC_KINDS = [
  "iso9001",
  "iso14001",
  "iatf16949",
  "qualified_invoice_registration",
  "anti_social_check",
  "insurance",
  "nda",
  "other",
] as const;
/** Discrepancy resolutions the server accepts (`orders::RESOLUTIONS`). */
export const RESOLUTIONS = ["accepted", "credit_note", "supplier_corrected", "written_off", "other"] as const;

/** Locale key of each proposed action's name. */
export const ACTION_LABEL: Record<string, string> = {
  resolve_discrepancy: "matching.action.resolve_discrepancy",
  request_credit_note: "matching.action.request_credit_note",
  send_chase: "chasing.action.send_chase",
  record_promised_date: "chasing.action.record_promised_date",
  confirm_delivery_date: "chasing.action.confirm_delivery_date",
  save_brief: "cost.action.save_brief",
  open_negotiation_task: "cost.action.open_negotiation_task",
  request_document_update: "supplier.action.request_document_update",
  flag_supplier_review: "supplier.action.flag_supplier_review",
  apply_import: "integrations.applyImport",
};

/**
 * What a person may change before approving, per action type. The server checks the same
 * rule (an edit never changes what the action is about); this only decides which inputs show.
 */
export const EDITABLE: Record<string, readonly string[]> = {
  resolve_discrepancy: ["resolution"],
  send_chase: ["subject", "body"],
  record_promised_date: ["date"],
  confirm_delivery_date: ["date", "note"],
  open_negotiation_task: ["title"],
  request_document_update: ["note"],
};

export type Action = Record<string, unknown> & { type?: string };

export function actionType(a: unknown): string | null {
  const type = (a as Action | null)?.type;
  return typeof type === "string" ? type : null;
}

export function canEdit(a: unknown): boolean {
  const type = actionType(a);
  return type !== null && (EDITABLE[type]?.length ?? 0) > 0;
}

const str = (v: unknown) => (typeof v === "string" ? v : "");

/** Values for an exception's text: money, dates and codes shown in the UI language. */
export function displayParams(params: Record<string, unknown>, t: TFunction, lang: string) {
  const out: Record<string, unknown> = { ...params };
  const currency = str(params.currency);
  for (const [k, v] of Object.entries(params)) {
    if (k.endsWith("_minor") && typeof v === "number" && currency) out[k] = formatMoney(v, currency, lang);
    else if (/(_on|_date)$/.test(k) && typeof v === "string" && /^\d{4}-\d{2}-\d{2}$/.test(v))
      out[k] = formatDate(v, lang);
  }
  if (typeof params.doc_kind === "string")
    out.doc_kind = t(`agents:supplier.docKind.${params.doc_kind}`, { defaultValue: params.doc_kind });
  if (typeof params.kind === "string" && (IMPORT_KINDS as readonly string[]).includes(params.kind))
    out.kind = t(`agents:integrations.kind.${params.kind}`);
  return out;
}

/** The exception's headline, from its locale key; never prose stored by the server. */
export function exceptionTitle(ex: AgentException, t: TFunction, lang: string): string {
  return t(`agents:${ex.title_key}`, {
    ...displayParams(ex.params as Record<string, unknown>, t, lang),
    defaultValue: ex.kind,
  });
}

/** A second paragraph when the title key has a sibling `body` (e.g. `cost.….title`/`.body`). */
export function exceptionBody(
  ex: AgentException,
  t: TFunction,
  exists: (key: string) => boolean,
  lang: string,
): string | null {
  if (!ex.title_key.endsWith(".title")) return null;
  const key = `agents:${ex.title_key.slice(0, -".title".length)}.body`;
  return exists(key) ? t(key, displayParams(ex.params as Record<string, unknown>, t, lang)) : null;
}

/** Basis points (100 = 1 %) as a percentage for display. */
export function bpsLabel(bps: number | null | undefined): string | null {
  if (bps === null || bps === undefined) return null;
  const whole = Math.trunc(bps / 100);
  const frac = Math.abs(bps % 100);
  return frac ? `${whole}.${String(frac).padStart(2, "0").replace(/0$/, "")}%` : `${whole}%`;
}
