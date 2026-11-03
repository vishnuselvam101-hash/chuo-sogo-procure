import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { App } from "../src/App";
import { initI18n } from "../src/i18n";
import { BridgeProvider } from "../src/lib/BridgeContext";
import type { Bridge } from "../src/lib/bridge";
import type { LocalStatus, PacksStatus, Prefs } from "../src/lib/types";

const GB = 1024 ** 3;

function status(over: Partial<LocalStatus>): LocalStatus {
  return {
    available: true,
    phase: "running",
    error: null,
    server_url: "http://127.0.0.1:50123",
    needs_setup: false,
    ai: "off",
    voice: "no_model",
    ocr: "no_tesseract",
    ...over,
  };
}

const PACKS: PacksStatus = {
  platform: "windows-x64",
  ram_bytes: 32 * GB,
  free_bytes: 200 * GB,
  recommended_variant: "standard",
  packs: [
    {
      id: "ai-model",
      kind: "large",
      version: "1",
      title_en: "AI model",
      title_ja: "AIモデル",
      licence: "Apache-2.0",
      licence_url: null,
      size: 9 * GB,
      variants: [
        {
          variant: "standard",
          title_en: "Standard",
          title_ja: "標準",
          min_ram_gb: 16,
          size: 9 * GB,
          installed: false,
        },
        {
          variant: "light",
          title_en: "Light",
          title_ja: "軽量版",
          min_ram_gb: 8,
          size: 5 * GB,
          installed: false,
        },
      ],
      status: "missing",
      done: 0,
      total: 0,
      error: null,
    },
  ],
};

function fake(prefs: Prefs, local: LocalStatus) {
  const f = {
    prefsGet: vi.fn(async () => prefs),
    localChooseMode: vi.fn(async () => null),
    localStatus: vi.fn(async () => local),
    localStart: vi.fn(async () => null),
    localSetup: vi.fn(async () => null),
    packsStatus: vi.fn(async () => PACKS),
    packsInstall: vi.fn(async () => null),
    packsCancel: vi.fn(async () => null),
    api: vi.fn(async () => ({ status: 200, body: [], request_id: "r" })),
    log: vi.fn(async () => undefined),
  };
  return { bridge: f as unknown as Bridge, f };
}

beforeAll(async () => {
  await initI18n("ja");
});
afterEach(cleanup);

describe("standalone mode", () => {
  it("asks once on a fresh installation and saves the choice", async () => {
    const { bridge, f } = fake({ language: null, server_url: "", mode: null }, status({ phase: "starting" }));
    render(
      <BridgeProvider value={bridge}>
        <App />
      </BridgeProvider>,
    );
    const local = await screen.findByRole("button", { name: /この PC だけで使う/ });
    expect(screen.getByRole("button", { name: /サーバーに接続する/ })).toBeTruthy();
    await act(async () => {
      fireEvent.click(local);
    });
    expect(f.localChooseMode).toHaveBeenCalledWith("local");
    expect(await screen.findByText("この PC で起動しています…")).toBeTruthy();
  });

  it("existing installations (no mode) go straight to sign-in", async () => {
    const { bridge, f } = fake({ language: null, server_url: "https://x.test" }, status({}));
    render(
      <BridgeProvider value={bridge}>
        <App />
      </BridgeProvider>,
    );
    expect(await screen.findByRole("button", { name: "サインイン" })).toBeTruthy();
    expect(f.localStatus).not.toHaveBeenCalled();
  });

  it("creates the factory, then offers the recommended AI model by pack id only", async () => {
    const { bridge, f } = fake(
      { language: null, server_url: "", mode: "local" },
      status({ needs_setup: true }),
    );
    render(
      <BridgeProvider value={bridge}>
        <App />
      </BridgeProvider>,
    );
    fireEvent.change(await screen.findByLabelText("工場名・会社名"), {
      target: { value: "株式会社テスト工業" },
    });
    fireEvent.change(screen.getByLabelText("お名前"), { target: { value: "山田" } });
    fireEvent.change(document.querySelector('input[type="email"]') as Element, {
      target: { value: "yamada@test.example" },
    });
    const [pw, repeat] = Array.from(document.querySelectorAll('input[type="password"]'));
    fireEvent.change(pw as Element, { target: { value: "correct-horse-battery" } });
    fireEvent.change(repeat as Element, { target: { value: "correct-horse-battery" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "作成して次へ" }));
    });
    expect(f.localSetup).toHaveBeenCalledWith(
      "株式会社テスト工業",
      "山田",
      "yamada@test.example",
      "correct-horse-battery",
      "ja",
    );
    expect(await screen.findByText("この PC におすすめ（32 GB）")).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /ダウンロード（9 GB）/ }));
    });
    expect(f.packsInstall).toHaveBeenCalledWith("ai-model", "standard");
  });
});
