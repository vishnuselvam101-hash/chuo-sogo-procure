// The only way the UI talks to the outside world: typed calls into the Rust layer.
// The session token never enters JavaScript.
import { invoke } from "@tauri-apps/api/core";
import type { ApiResponse, Me, Prefs } from "./types";

export const bridge = {
  prefsGet: () => invoke<Prefs>("prefs_get"),
  prefsSetLanguage: (language: string | null) => invoke<null>("prefs_set_language", { language }),
  prefsSetServer: (url: string) => invoke<null>("prefs_set_server", { url }),
  login: (email: string, password: string) =>
    invoke<ApiResponse<Me & { expires_at: string }>>("auth_login", { email, password }),
  logout: () => invoke<null>("auth_logout"),
  api: <T>(method: "GET" | "POST" | "PATCH" | "DELETE", path: string, body?: unknown) =>
    invoke<ApiResponse<T>>("api_request", { method, path, body: body ?? null }),
  log: (level: "info" | "warn" | "error", message: string, requestId?: string) =>
    invoke<null>("client_log", { level, message, requestId: requestId ?? null }).catch(() => undefined),
};

export type Bridge = typeof bridge;

/** Stable error code from an API response (translated via the `errors` namespace). */
export function errorCode(res: ApiResponse): string {
  if (res.status === 0) return "network";
  const body = res.body as { error?: { code?: string } } | null;
  return body?.error?.code ?? "unknown";
}

export function ok<T>(res: ApiResponse<T>): res is ApiResponse<T> & { body: T } {
  return res.status >= 200 && res.status < 300;
}
