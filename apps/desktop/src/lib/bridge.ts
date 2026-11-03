// The only way the UI talks to the outside world: typed calls into the Rust layer.
// The session token never enters JavaScript.
import { invoke } from "@tauri-apps/api/core";
import type { ApiResponse, LocalStatus, Me, PacksStatus, Prefs } from "./types";

export type HttpMethod = "GET" | "POST" | "PUT" | "PATCH" | "DELETE";

export const bridge = {
  prefsGet: () => invoke<Prefs>("prefs_get"),
  prefsSetLanguage: (language: string | null) => invoke<null>("prefs_set_language", { language }),
  prefsSetDisplay: (textSize: string, highContrast: boolean) =>
    invoke<null>("prefs_set_display", { textSize, highContrast }),
  prefsSetServer: (url: string) => invoke<null>("prefs_set_server", { url }),
  login: (email: string, password: string) =>
    invoke<ApiResponse<Me & { expires_at: string }>>("auth_login", { email, password }),
  /** Trial server only: enters a listed trial account without a password. */
  demoLogin: (email: string) => invoke<ApiResponse<Me & { expires_at: string }>>("auth_demo", { email }),
  logout: () => invoke<null>("auth_logout"),
  /** Admin console: password check; the next step token stays in the native layer. */
  adminLogin: (email: string, password: string) =>
    invoke<ApiResponse<{ step: "mfa" | "enroll" }>>("auth_login", { email, password }),
  /** Admin console, first sign-in: a new authenticator secret to type into the app. */
  adminEnroll: () => invoke<ApiResponse<{ secret: string; otpauth_uri: string }>>("auth_enroll"),
  /** Admin console: the 6-digit authenticator code; starts the admin session. */
  adminCode: (code: string) => invoke<ApiResponse<{ expires_at: string }>>("auth_code", { code }),
  api: <T>(method: HttpMethod, path: string, body?: unknown) =>
    invoke<ApiResponse<T>>("api_request", { method, path, body: body ?? null }),
  /** Saves an attachment into the user's Downloads folder; `body.saved_to` on success. */
  download: (orgId: string, attachmentId: string, fileName: string) =>
    invoke<ApiResponse<{ saved_to: string }>>("api_download", { orgId, attachmentId, fileName }),
  /** Saves an issued invoice / credit note PDF as `<number>.pdf` in Downloads. */
  invoicePdf: (orgId: string, invoiceId: string, number: string) =>
    invoke<ApiResponse<{ saved_to: string }>>("invoice_download", { orgId, invoiceId, number }),
  /** Saves a kept supplier file (email, PDF, spreadsheet, FAX scan) into Downloads. */
  documentDownload: (orgId: string, documentId: string, fileName: string) =>
    invoke<ApiResponse<{ saved_to: string }>>("document_download", { orgId, documentId, fileName }),
  /** Saves the orders or savings-ledger CSV (optionally one month, `YYYY-MM`) into Downloads. */
  csvDownload: (orgId: string, kind: "orders" | "savings", month: string | null) =>
    invoke<ApiResponse<{ saved_to: string }>>("csv_download", { orgId, kind, month }),
  /** A listing photo as a `data:` URL (fetched and checked by the native layer). */
  image: (path: string) => invoke<ApiResponse<{ data_url: string }>>("api_image", { path }),
  /** Standalone mode: the local database, server and AI on this PC. */
  localStatus: () => invoke<LocalStatus>("local_status"),
  localChooseMode: (mode: "server" | "local") => invoke<null>("local_choose_mode", { mode }),
  localStart: () => invoke<null>("local_start"),
  localSetup: (factoryName: string, adminName: string, email: string, password: string, locale: string) =>
    invoke<null>("local_setup", { factoryName, adminName, email, password, locale }),
  /** Pack manager: packs are named by id only; URLs and paths come from the installed manifest. */
  packsStatus: () => invoke<PacksStatus>("packs_status"),
  packsInstall: (packId: string, variant: string | null) =>
    invoke<null>("packs_install", { packId, variant }),
  packsCancel: (packId: string) => invoke<null>("packs_cancel", { packId }),
  log: (level: "info" | "warn" | "error", message: string, requestId?: string) =>
    invoke<null>("client_log", { level, message, requestId: requestId ?? null }).catch(() => undefined),
};

export type Bridge = typeof bridge;

/**
 * Stable error code from an API response (translated via the `errors` namespace).
 * When the server adds a detail (e.g. `items_not_verified`), the result is
 * `code/detail` so the UI can show the specific message and fall back to the general one.
 */
export function errorCode(res: ApiResponse): string {
  if (res.status === 0) return "network";
  const body = res.body as { error?: { code?: string; detail?: string } } | null;
  const code = body?.error?.code ?? "unknown";
  const detail = body?.error?.detail;
  return detail ? `${code}/${detail}` : code;
}

export function ok<T>(res: ApiResponse<T>): res is ApiResponse<T> & { body: T } {
  return res.status >= 200 && res.status < 300;
}
