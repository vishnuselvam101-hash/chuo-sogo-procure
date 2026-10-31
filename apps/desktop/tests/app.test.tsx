import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { App, resolveLanguage } from "../src/App";
import { initI18n } from "../src/i18n";
import { BridgeProvider } from "../src/lib/BridgeContext";
import type { Bridge } from "../src/lib/bridge";
import type { Me, Prefs } from "../src/lib/types";

function membership(role: string, permissions: string[], kind: "customer" | "platform" = "customer") {
  return {
    org_id: "0192f0aa-1111-7222-8333-444455556666",
    org_name: "株式会社エー製作所",
    org_kind: kind,
    default_locale: "ja",
    timezone: "Asia/Tokyo",
    role,
    permissions,
  };
}

function meWith(m: ReturnType<typeof membership>): Me {
  return {
    user: { id: "u1", email: "u@a.test", display_name: "山田", locale: null, account_kind: "customer" },
    memberships: [m],
    invitations: [],
  };
}

/** In-memory stand-in for the Rust layer; `stored` survives "restarts". */
function fakeBridge(me: Me, stored: { prefs: Prefs }) {
  const calls: string[] = [];
  const fake = {
    prefsGet: vi.fn(async () => ({ ...stored.prefs })),
    prefsSetLanguage: vi.fn(async (language: string | null) => {
      stored.prefs = { ...stored.prefs, language };
    }),
    prefsSetServer: vi.fn(async () => undefined),
    login: vi.fn(async () => ({ status: 200, body: { ...me, expires_at: "" }, request_id: "r" })),
    logout: vi.fn(async () => undefined),
    api: vi.fn(async (method: string, path: string) => {
      calls.push(`${method} ${path}`);
      return { status: 200, body: method === "GET" && path.endsWith("/me") ? me : [], request_id: "r" };
    }),
    log: vi.fn(async () => undefined),
  };
  return { bridge: fake as unknown as Bridge, fake, calls };
}

/** Language-independent: the login screen may already be in any language. */
async function signIn() {
  const email = await waitFor(() => {
    const el = document.querySelector<HTMLInputElement>('input[type="email"]');
    if (!el) throw new Error("login form not rendered");
    return el;
  });
  const password = document.querySelector<HTMLInputElement>('input[type="password"]');
  const form = email.closest("form");
  if (!password || !form) throw new Error("login form incomplete");
  fireEvent.change(email, { target: { value: "u@a.test" } });
  fireEvent.change(password, { target: { value: "pw-pw-pw-pw-pw" } });
  await act(async () => {
    fireEvent.submit(form);
  });
}

beforeAll(async () => {
  await initI18n("ja");
});
afterEach(cleanup);

describe("language resolution", () => {
  it("prefers the user's choice, then the organisation default, then Japanese", () => {
    expect(resolveLanguage("ta", "en")).toBe("ta");
    expect(resolveLanguage(null, "en")).toBe("en");
    expect(resolveLanguage(null, undefined)).toBe("ja");
    expect(resolveLanguage("xx", "zz")).toBe("ja");
  });
});

describe("desktop shell", () => {
  it("renders Japanese by default and shows only what the role allows", async () => {
    const stored = { prefs: { language: null, server_url: "http://127.0.0.1:8088" } };
    const { bridge } = fakeBridge(
      meWith(membership("requester", ["submit_request", "view_own_requests"])),
      stored,
    );
    render(
      <BridgeProvider value={bridge}>
        <App />
      </BridgeProvider>,
    );
    await signIn();
    expect(await screen.findByRole("button", { name: "ダッシュボード" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "調達依頼" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "メンバー管理" })).toBeNull();
    expect(screen.queryByRole("button", { name: "監査ログ" })).toBeNull();
    expect(screen.queryByRole("button", { name: "承認" })).toBeNull();
    expect(screen.getByTestId("current-language").textContent).toBe("日本語");
  });

  it("switches language instantly, persists it across restart, and never writes data", async () => {
    const stored = { prefs: { language: null as string | null, server_url: "http://127.0.0.1:8088" } };
    const me = meWith(membership("customer_admin", ["manage_org_users", "view_audit", "submit_request"]));
    const first = fakeBridge(me, stored);
    const view = render(
      <BridgeProvider value={first.bridge}>
        <App />
      </BridgeProvider>,
    );
    await signIn();
    fireEvent.click(await screen.findByRole("button", { name: /言語/ }));
    await act(async () => {
      fireEvent.click(screen.getByRole("option", { name: "தமிழ்" }));
    });
    expect(await screen.findByRole("button", { name: "முகப்புப் பலகை" })).toBeTruthy();
    expect(document.documentElement.lang).toBe("ta");
    expect(first.fake.prefsSetLanguage).toHaveBeenCalledWith("ta");
    // Language change must not touch procurement data: no write calls at all.
    expect(first.calls.filter((c) => !c.startsWith("GET"))).toEqual([]);
    view.unmount();

    // "Restart": a fresh App reading the persisted preference.
    const second = fakeBridge(me, stored);
    render(
      <BridgeProvider value={second.bridge}>
        <App />
      </BridgeProvider>,
    );
    await signIn();
    expect(await screen.findByRole("button", { name: "முகப்புப் பலகை" })).toBeTruthy();
    expect(screen.getByTestId("current-language").textContent).toBe("தமிழ்");
  });

  it("renders Hindi and English menus", async () => {
    for (const [lang, label] of [
      ["hi", "डैशबोर्ड"],
      ["en", "Dashboard"],
    ] as const) {
      const stored = { prefs: { language: lang, server_url: "http://127.0.0.1:8088" } };
      const { bridge } = fakeBridge(meWith(membership("requester", ["submit_request"])), stored);
      render(
        <BridgeProvider value={bridge}>
          <App />
        </BridgeProvider>,
      );
      await signIn();
      await waitFor(() => expect(screen.getByRole("button", { name: label })).toBeTruthy());
      cleanup();
    }
  });

  it("shows the operations workspace to internal staff", async () => {
    const stored = { prefs: { language: "en", server_url: "http://127.0.0.1:8088" } };
    const m = membership(
      "platform_super_admin",
      ["manage_organisations", "manage_internal_users", "view_audit"],
      "platform",
    );
    const { bridge } = fakeBridge(meWith(m), stored);
    render(
      <BridgeProvider value={bridge}>
        <App />
      </BridgeProvider>,
    );
    await signIn();
    expect(await screen.findByText("Procurement operations")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Organisations" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Approvals" })).toBeNull();
  });
});
