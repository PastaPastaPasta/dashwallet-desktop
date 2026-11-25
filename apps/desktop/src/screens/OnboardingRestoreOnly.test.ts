// Q107 C1's restore-only Welcome: while the network's data is a restore not yet synced (RESTORED), a
// wallet comes only from its recovery phrase. MG's copy, first run and Add a wallet; once the mark is gone
// the other ways to add a wallet show again.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../api/backend";
import { BackendContext, keys } from "../api/queries";
import { DEFAULT_PREFS, ui } from "../app/ui";
import { buildWorld } from "../fixture/scenarios";
import { HANDLERS, type Ctx } from "../fixture/handlers";
import { en } from "../i18n/en";
import { failureText } from "../onboarding/model";
import { OnboardingModel } from "../onboarding/onboarding";
import { Welcome } from "./Onboarding";

vi.mock("react", async (importOriginal) => {
  const real = await importOriginal<typeof import("react")>();
  const useSyncExternalStore: typeof real.useSyncExternalStore = (subscribe, get, server) => real.useSyncExternalStore(subscribe, get, server ?? get);
  return { ...real, default: { ...real, useSyncExternalStore }, useSyncExternalStore };
});

afterEach(() => ui.set({ theme: null, prefs: DEFAULT_PREFS }));

const W = en.onboarding.welcome;
const RESTORE_ONLY = "Your earlier wallet data couldn't be read, so Dash Wallet started with a new data folder. Enter a recovery phrase to set up a wallet again.";
const RESTORE_ONLY_ADD = "Until Dash Wallet has finished its first sync, you can only add a wallet from its recovery phrase. The other ways to add a wallet come back after that.";

const escaped = (text: string) => text.replace(/&/g, "&amp;").replace(/'/g, "&#x27;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

function renderWelcome(restored: boolean, add: boolean): string {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  client.setQueryData(keys.restored, restored);
  const backend = { kind: "fixture" } as unknown as Backend;
  const model = new OnboardingModel(backend, "testnet", { files: { api: backend, network: "testnet" } });
  const welcome = createElement(Welcome, { model, onCancel: add ? () => {} : null });
  return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(BackendContext.Provider, { value: backend }, welcome)));
}

const text = (html: string) => /data-testid="onboarding-welcome-text">([^<]*)<\/p>/.exec(html)?.[1];
const OTHER_WAYS = ["onboarding-create", "onboarding-restore-file", "onboarding-24-words"];

describe("the restore-only Welcome (Q107 C1)", () => {
  it("carries MG's copy, word for word", () => {
    expect(W.restoreOnly).toBe(RESTORE_ONLY);
    expect(W.restoreOnlyAdd).toBe(RESTORE_ONLY_ADD);
    expect(W.restoreOnly).not.toContain("{folder}");
  });

  for (const theme of ["light", "dark"] as const)
    for (const add of [false, true])
      it(`in ${theme}, ${add ? "Add a wallet" : "the first run"}: only the recovery phrase while RESTORED is set`, () => {
        ui.set({ theme });
        const html = renderWelcome(true, add);
        expect(text(html)).toBe(escaped(add ? RESTORE_ONLY_ADD : RESTORE_ONLY));
        expect(html).toContain('data-testid="onboarding-restore"');
        for (const id of OTHER_WAYS) expect(html, id).not.toContain(`data-testid="${id}"`);
      });

  for (const add of [false, true])
    it(`${add ? "Add a wallet" : "the first run"}: every other way to add a wallet is back once the mark is gone`, () => {
      const html = renderWelcome(false, add);
      expect(text(html)).toBe(escaped(W.text));
      for (const id of ["onboarding-restore", ...OTHER_WAYS]) expect(html, id).toContain(`data-testid="${id}"`);
      expect(html).not.toContain(escaped(RESTORE_ONLY));
      expect(html).not.toContain(escaped(RESTORE_ONLY_ADD));
    });

  it("a refused way in says nothing was added and why, in MG's words", () => {
    expect(failureText({ code: "restore_only", message: "x", data: null })).toBe(
      "Nothing was added. Until Dash Wallet has finished its first sync, you can only add a wallet from its recovery phrase.",
    );
  });

  it("the fixture refuses every way to add a wallet but the phrase while RESTORED is set, and none after", () => {
    const world = buildWorld("funded", null);
    const ctx: Ctx = { world, emit: () => {} };
    type Way = "generate_mnemonic" | "restore_backup" | "import_dump_wallet" | "import_wallet_dat" | "import_key_material" | "import_watch_only";
    const call = (name: Way, args: object) => (HANDLERS[name] as unknown as (a: object, c: typeof ctx) => unknown)(args, ctx);
    const codeOf = (f: () => unknown) => {
      try {
        f();
        return null;
      } catch (e) {
        return (e as { code?: string }).code ?? null;
      }
    };
    const options = { name: null, birthHeight: null, coreCompat: false, lookahead: null };
    const ways: [Way, object][] = [
      ["generate_mnemonic", { wordCount: 12, language: "english" }],
      ["restore_backup", { path: "/nowhere", passphrase: null }],
      ["import_dump_wallet", { path: "/nowhere", options }],
      ["import_wallet_dat", { path: "/nowhere", walletPassphrase: null, options }],
      ["import_key_material", { material: { kind: "hdSeed", seedHex: "01" }, options }],
      ["import_watch_only", { xpub: "xpub", options: { name: null, birthHeight: null, lookahead: null } }],
    ];
    world.restored = true;
    for (const [name, args] of ways) expect(codeOf(() => call(name, args)), name).toBe("restore_only");
    world.restored = false;
    for (const [name, args] of ways) expect(codeOf(() => call(name, args)), name).not.toBe("restore_only");
  });
});
