import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { AdminApp, adminNav } from "../src/admin/AdminApp";
import { groupSecret } from "../src/admin/AdminLogin";
import { initI18n } from "../src/i18n";
import { BridgeProvider } from "../src/lib/BridgeContext";
import type { Bridge } from "../src/lib/bridge";
import type { AdminMe } from "../src/admin/types";

const SUPER = [
  "manage_organisations",
  "manage_internal_users",
  "manage_service_assignments",
  "view_system_health",
  "view_audit",
];

function adminMe(permissions: string[]): AdminMe {
  return {
    user: {
      id: "a1",
      email: "admin@chuo-sogo.test",
      display_name: "管理者",
      locale: null,
      account_kind: "internal",
    },
    platform_org_id: "p1",
    role: "platform_super_admin",
    permissions,
  };
}

function reply<T>(status: number, body: T) {
  return { status, body, request_id: "r" };
}

/** In-memory stand-in for the admin build's Rust layer. */
function fakeBridge(step: "mfa" | "enroll", me: AdminMe) {
  const calls: string[] = [];
  let signedIn = false;
  const fake = {
    prefsGet: vi.fn(async () => ({ language: "en", server_url: "http://127.0.0.1:8088" })),
    prefsSetLanguage: vi.fn(async () => undefined),
    prefsSetServer: vi.fn(async () => undefined),
    adminLogin: vi.fn(async () => reply(200, { step })),
    adminEnroll: vi.fn(async () =>
      reply(200, { secret: "ABCDEFGHIJKLMNOP", otpauth_uri: "otpauth://totp/x" }),
    ),
    adminCode: vi.fn(async (code: string) => {
      if (code !== "123456") return reply(401, { error: { code: "invalid_credentials" } });
      signedIn = true;
      return reply(200, { expires_at: "" });
    }),
    logout: vi.fn(async () => undefined),
    api: vi.fn(async (method: string, path: string) => {
      calls.push(`${method} ${path}`);
      if (!signedIn) return reply(401, { error: { code: "unauthenticated" } });
      if (path.endsWith("/me")) return reply(200, me);
      if (path.endsWith("/health"))
        return reply(200, {
          server_version: "0.1.0",
          schema_version: 8,
          database_bytes: 1048576,
          ai_provider: "mock",
          ai_model: "",
          active_sessions: 1,
          accounts_active: 3,
          accounts_locked: 0,
          orgs: [],
        });
      return reply(200, []);
    }),
    log: vi.fn(async () => undefined),
  };
  return { bridge: fake as unknown as Bridge, fake, calls };
}

async function enterPassword() {
  const email = await waitFor(() => {
    const el = document.querySelector<HTMLInputElement>('input[type="email"]');
    if (!el) throw new Error("login form not rendered");
    return el;
  });
  const password = document.querySelector<HTMLInputElement>('input[type="password"]');
  if (!password) throw new Error("no password field");
  fireEvent.change(email, { target: { value: "admin@chuo-sogo.test" } });
  fireEvent.change(password, { target: { value: "correct horse battery" } });
  fireEvent.submit(email.closest("form") as HTMLFormElement);
}

async function enterCode(code: string) {
  const input = await screen.findByTestId("totp-code");
  fireEvent.change(input, { target: { value: code } });
  fireEvent.submit(input.closest("form") as HTMLFormElement);
}

beforeAll(async () => {
  await initI18n("en");
});
afterEach(cleanup);

describe("admin console", () => {
  it("first sign-in shows the new authenticator key, then confirms it with a code", async () => {
    const { bridge, fake } = fakeBridge("enroll", adminMe(SUPER));
    render(
      <BridgeProvider value={bridge}>
        <AdminApp />
      </BridgeProvider>,
    );
    await enterPassword();
    expect(await screen.findByTestId("totp-secret")).toHaveProperty("textContent", "ABCD EFGH IJKL MNOP");
    expect(fake.adminEnroll).toHaveBeenCalledOnce();
    await enterCode("123456");
    expect(await screen.findByTestId("health")).toBeTruthy();
    expect(fake.adminCode).toHaveBeenCalledWith("123456");
  });

  it("a wrong code says so (not 'wrong password') and stays on the code step", async () => {
    const { bridge } = fakeBridge("mfa", adminMe(SUPER));
    render(
      <BridgeProvider value={bridge}>
        <AdminApp />
      </BridgeProvider>,
    );
    await enterPassword();
    await enterCode("000000");
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Enter the 6-digit code from your authenticator app.",
    );
    expect(screen.getByTestId("totp-code")).toBeTruthy();
  });

  it("only uses the admin API namespace", async () => {
    const { bridge, calls } = fakeBridge("mfa", adminMe(SUPER));
    render(
      <BridgeProvider value={bridge}>
        <AdminApp />
      </BridgeProvider>,
    );
    await enterPassword();
    await enterCode("123456");
    await screen.findByTestId("health");
    fireEvent.click(screen.getByRole("button", { name: "Organisations" }));
    await screen.findByTestId("org-list");
    expect(calls.length).toBeGreaterThan(1);
    for (const c of calls) expect(c).toMatch(/^(GET|POST|PUT|PATCH|DELETE) \/api\/admin\/v1\//);
  });

  it("an expired sign-in step goes back to the password", async () => {
    const { bridge, fake } = fakeBridge("mfa", adminMe(SUPER));
    fake.adminCode.mockResolvedValueOnce(reply(401, { error: { code: "unauthenticated" } }));
    render(
      <BridgeProvider value={bridge}>
        <AdminApp />
      </BridgeProvider>,
    );
    await enterPassword();
    await enterCode("123456");
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Your session has ended. Please sign in again.",
    );
    expect(document.querySelector('input[type="password"]')).toBeTruthy();
  });

  it("if the console cannot load after the code, the new session is ended and the reason shown", async () => {
    const { bridge, fake } = fakeBridge("mfa", adminMe(SUPER));
    fake.api.mockResolvedValueOnce(reply(403, { error: { code: "forbidden" } }));
    render(
      <BridgeProvider value={bridge}>
        <AdminApp />
      </BridgeProvider>,
    );
    await enterPassword();
    await enterCode("123456");
    expect((await screen.findByRole("alert")).textContent).toBe("You do not have permission to do this.");
    expect(fake.logout).toHaveBeenCalled();
    expect(document.querySelector('input[type="password"]')).toBeTruthy();
  });

  it("shows sections by permission", () => {
    expect(adminNav([...SUPER, "manage_billing", "manage_shop"])).toEqual([
      "overview",
      "orgs",
      "accounts",
      "shop",
      "billing",
      "settings",
    ]);
    expect(adminNav(["manage_service_assignments", "view_audit"])).toEqual(["settings"]);
    expect(adminNav(["view_system_health"])).toEqual(["overview", "settings"]);
  });

  it("groups the authenticator key for typing", () => {
    expect(groupSecret("ABCDEFGHIJ")).toBe("ABCD EFGH IJ");
    expect(groupSecret("ABCD")).toBe("ABCD");
  });
});
