import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { ListingPhoto } from "../src/catalogue/ListingView";
import { toInput } from "../src/pages/Listings";
import { initI18n } from "../src/i18n";
import { navFor } from "../src/lib/nav";
import { BridgeProvider } from "../src/lib/BridgeContext";
import type { Bridge } from "../src/lib/bridge";
import type { MembershipView } from "../src/lib/types";

const form = (over: Record<string, unknown> = {}) => ({
  manufacturer: "NSK",
  model_number: "6204-2RS",
  serial_number: "",
  condition: "new",
  title: "軸受",
  title_en: "",
  description: "説明",
  description_en: "",
  specs: [
    { key: "内径", value: "20 mm" },
    { key: "", value: "" },
  ],
  business: "480",
  consumer: "",
  tax_rate_bps: 1000,
  stock: "",
  ...over,
});

function member(kind: string, permissions: string[]): MembershipView {
  return {
    org_id: "o",
    org_name: "x",
    org_kind: kind,
    default_locale: "ja",
    timezone: "Asia/Tokyo",
    role: "",
    permissions,
    access: "member",
  };
}

beforeAll(async () => {
  await initI18n("en");
});
afterEach(cleanup);

describe("catalogue and listings", () => {
  it("reads listing prices and stock exactly, drops empty specs", () => {
    expect(toInput(form())).toMatchObject({
      business_price_minor: 480,
      consumer_price_minor: null,
      stock: null,
      serial_number: null,
      specs: [{ key: "内径", value: "20 mm" }],
    });
    expect(toInput(form({ business: "480.5" }))).toBeNull(); // yen has no decimals
    expect(toInput(form({ stock: "1.5" }))).toBeNull();
    expect(toInput(form({ stock: "-1" }))).toBeNull();
    expect(toInput(form({ stock: "12", serial_number: " SN-1 " }))).toMatchObject({
      stock: 12,
      serial_number: "SN-1",
    });
  });

  it("suppliers see their listings; customers the catalogue; nobody else", () => {
    const ids = (m: MembershipView) => navFor(m).map((n) => n.id);
    expect(ids(member("supplier", ["manage_listings", "manage_org_users", "view_audit"]))).toEqual([
      "dashboard",
      "listings",
      "members",
      "audit",
      "settings",
    ]);
    expect(ids(member("customer", ["submit_request", "view_own_requests", "view_catalogue"]))).toContain(
      "catalogue",
    );
    expect(ids(member("customer", ["submit_request"]))).not.toContain("catalogue");
    expect(ids(member("customer", ["view_catalogue"]))).not.toContain("listings");
  });

  it("shows photos only through the native layer", async () => {
    const image = vi.fn(async () => ({
      status: 200,
      body: { data_url: "data:image/png;base64,iVBORw0KGgo=" },
      request_id: "r",
    }));
    render(
      <BridgeProvider value={{ image } as unknown as Bridge}>
        <ListingPhoto path="/api/v1/orgs/o/catalogue/l/media/m/content" alt="bearing" />
      </BridgeProvider>,
    );
    const img = await screen.findByRole("img", { name: "bearing" });
    expect(img.getAttribute("src")).toBe("data:image/png;base64,iVBORw0KGgo=");
    expect(image).toHaveBeenCalledWith("/api/v1/orgs/o/catalogue/l/media/m/content");
  });

  it("a failed photo says so instead of breaking the page", async () => {
    const image = vi.fn(async () => ({ status: 404, body: null, request_id: "r" }));
    render(
      <BridgeProvider value={{ image } as unknown as Bridge}>
        <ListingPhoto path="/p" alt="x" />
      </BridgeProvider>,
    );
    expect(await screen.findByText("No photo")).toBeTruthy();
  });
});
