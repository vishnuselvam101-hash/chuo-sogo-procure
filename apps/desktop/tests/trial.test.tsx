import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { Login } from "../src/pages/Login";
import { initI18n } from "../src/i18n";
import { BridgeProvider } from "../src/lib/BridgeContext";
import type { Bridge } from "../src/lib/bridge";

const account = (email: string, role: string, org_kind: string) => ({
  email,
  display_name: email,
  role,
  org_name: org_kind === "customer" ? "サンプル精機株式会社" : "Other",
  org_kind,
});

function fakeBridge(accounts: unknown, status = 200) {
  const demoLogin = vi.fn(async () => ({
    status: 200,
    body: { user: {}, memberships: [], invitations: [] },
  }));
  const api = vi.fn(async () => ({ status, body: accounts }));
  return { bridge: { api, demoLogin } as unknown as Bridge, demoLogin };
}

beforeAll(async () => {
  await initI18n("en");
});
afterEach(cleanup);

describe("trial server sign-in", () => {
  it("offers business roles in walkthrough order and enters one without a password", async () => {
    const { bridge, demoLogin } = fakeBridge([
      account("sales@s.test", "supplier_admin", "supplier"),
      account("req@c.test", "requester", "customer"),
      account("hanako@x.test", "consumer", "consumer"),
      account("ceo@c.test", "customer_admin", "customer"),
    ]);
    const onLoggedIn = vi.fn();
    render(
      <BridgeProvider value={bridge}>
        <Login serverUrl="https://trial.test" onLoggedIn={onLoggedIn} />
      </BridgeProvider>,
    );
    await screen.findByTestId("trial-start");
    const roles = screen.getAllByRole("button").filter((b) => b.className === "trial-role");
    // The consumer belongs in the shop app; customer roles come first.
    expect(roles.map((b) => b.textContent)).toEqual([
      "Organisation administratorサンプル精機株式会社",
      "Requesterサンプル精機株式会社",
      "Supplier administratorOther",
    ]);
    const requester = roles[1];
    expect(requester).toBeDefined();
    if (requester) fireEvent.click(requester);
    await waitFor(() => expect(onLoggedIn).toHaveBeenCalled());
    expect(demoLogin).toHaveBeenCalledWith("req@c.test");
  });

  it("shows the normal sign-in form when the server is not a trial server", async () => {
    const { bridge } = fakeBridge({ error: { code: "not_found" } }, 404);
    render(
      <BridgeProvider value={bridge}>
        <Login serverUrl="https://real.test" onLoggedIn={() => {}} />
      </BridgeProvider>,
    );
    await screen.findByLabelText("Password");
    expect(screen.queryByTestId("trial-start")).toBeNull();
  });
});
