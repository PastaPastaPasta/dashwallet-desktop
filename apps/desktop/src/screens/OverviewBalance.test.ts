import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { clearMocks } from "@tauri-apps/api/mocks";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, expect, it, vi } from "vitest";
import { BackendContext, keys } from "../api/queries";
import { IpcBackend } from "../api/tauri";
import { installFixture } from "../fixture";
import { FIXTURE_WALLET_ID as walletId } from "../fixture/scenarios";
import { Overview } from "./Overview";

vi.mock("react", async (original) => {
  const react = await original<typeof import("react")>();
  const useSyncExternalStore: typeof react.useSyncExternalStore = (subscribe, get, server) => react.useSyncExternalStore(subscribe, get, server ?? get);
  return { ...react, default: { ...react, useSyncExternalStore }, useSyncExternalStore };
});

afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

it("renders the same three cells with pending on each reporting pool and when it clears", async () => {
  vi.stubGlobal("window", globalThis);
  const { world } = installFixture({ scenario: "funded" });
  const backend = new IpcBackend("fixture");
  const client = new QueryClient();
  const balances = (await backend.balance(walletId))!;
  const shielded = (await backend.shieldedStatus(walletId))!;
  const platform = (await backend.platformAddresses(walletId))!;
  client.setQueryData(keys.platformAddresses(walletId), { ...platform, credits: "0", movableCredits: "0", duffs: 0, addresses: [] });
  const home = createElement(Overview, { walletId, network: world.launch.network, onOpen: () => {} });
  const render = (pending: boolean) => {
    client.setQueryData(keys.balance(walletId), { ...balances, unconfirmed: pending ? 10_000_000 : 0 });
    client.setQueryData(keys.shieldedStatus(walletId), { ...shielded, balance: "0", pending: pending ? "49609190000" : "0" });
    return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(BackendContext.Provider, { value: backend }, home)));
  };
  const arriving = render(true);
  const settled = render(false);
  for (const html of [arriving, settled]) {
    expect(html).toContain('data-testid="hero-breakdown"');
    expect(html.match(/data-testid="breakdown-(core|shielded|platform)"/g)).toHaveLength(3);
  }
  const cell = (pool: string) => new RegExp('data-testid="breakdown-' + pool + '"[^>]*>(.*?)</div>').exec(arriving)?.[1];
  expect(cell("core")).toContain("0.10 pending");
  expect(cell("shielded")).toContain("0.4960919 pending");
  expect(cell("platform")).not.toContain("pending");
  expect(settled).not.toContain("0.4960919 pending");
});
