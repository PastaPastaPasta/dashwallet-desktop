// While RESTORED is set a wallet comes only from its recovery phrase: Settings offers no other way to
// add one (Backups' imports, Add watch-only), and shows them again once the mark is gone.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement, type FunctionComponent } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { Backend } from "../../api/backend";
import { BackendContext, keys } from "../../api/queries";
import { BackupsTab } from "./Backups";
import { WalletsTab } from "./Wallets";

vi.mock("react", async (importOriginal) => {
  const real = await importOriginal<typeof import("react")>();
  const useSyncExternalStore: typeof real.useSyncExternalStore = (subscribe, get, server) => real.useSyncExternalStore(subscribe, get, server ?? get);
  return { ...real, default: { ...real, useSyncExternalStore }, useSyncExternalStore };
});

function render(tab: FunctionComponent, restored: boolean | undefined): string {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  if (restored !== undefined) client.setQueryData(keys.restored, restored);
  const backend = { kind: "fixture", locale: "en-US" } as unknown as Backend;
  return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(BackendContext.Provider, { value: backend }, createElement(tab))));
}

const ENTRY_POINTS: [string, FunctionComponent, string][] = [
  ["Backups ▸ Import a wallet file", BackupsTab, "settings-import-file"],
  ["Backups ▸ Import keys", BackupsTab, "settings-import-keys"],
  ["Wallets ▸ Add watch-only", WalletsTab, "settings-add-watch-only"],
];

describe("Settings while RESTORED is set", () => {
  for (const [name, tab, id] of ENTRY_POINTS) {
    it(`${name} is hidden under the mark`, () => {
      expect(render(tab, true)).not.toContain(`data-testid="${id}"`);
    });

    it(`${name} shows again once the mark is gone, and while it isn't known`, () => {
      expect(render(tab, false)).toContain(`data-testid="${id}"`);
      expect(render(tab, undefined)).toContain(`data-testid="${id}"`);
    });
  }

  it("keeps the phrase way in and everything that adds no wallet", () => {
    const wallets = render(WalletsTab, true);
    for (const id of ["settings-add-wallet", "settings-recovery-phrase", "settings-wallets"]) expect(wallets, id).toContain(`data-testid="${id}"`);
    const backups = render(BackupsTab, true);
    expect(backups).not.toContain('data-testid="settings-import-card"');
    expect(backups).toContain('data-testid="settings-automatic-backups"');
  });
});
