// What RESTORED waits for (B addendum 1, MG C3 v2), under the restored banner: a line for each wait,
// Try again, Finish without Dash Platform only while Platform's first check is the wait, a history that
// failed to load named with Report a problem only, and a clear that couldn't be written with Try again
// and Report a problem. Read again on sync, never on a timer.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { clearMocks } from "@tauri-apps/api/mocks";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../api/backend";
import { LOCKED_COMMANDS } from "../api/lockGate";
import { BackendContext, keys } from "../api/queries";
import type { RestoreStatus } from "../api/restoreHolds";
import { isRestored } from "../api/storeOpen";
import { IpcBackend } from "../api/tauri";
import { DEFAULT_PREFS, ui } from "../app/ui";
import { t } from "../i18n";
import { installFixture } from "../fixture/index";
import { FIXTURE_WALLET_ID as W } from "../fixture/scenarios";
import { RestoredStatePanel, RestoredStateActions, RestoredStateView, offersFinishWithoutPlatform, offersRetry, restoredStateActions } from "./RestoredStatePanel";

// Server rendering needs a snapshot for every external store the app reads; the client's stands in.
vi.mock("react", async (importOriginal) => {
  const real = await importOriginal<typeof import("react")>();
  const useSyncExternalStore: typeof real.useSyncExternalStore = (subscribe, get, server) => real.useSyncExternalStore(subscribe, get, server ?? get);
  return { ...real, default: { ...real, useSyncExternalStore }, useSyncExternalStore };
});

const COPY = {
  core: "Waiting for the wallet to finish syncing with the Dash network.",
  platform: "Waiting for Dash Wallet's first check with Dash Platform.",
  history: "Waiting for a wallet's history to load.",
  retry: "Try again",
  finish: "Finish without Dash Platform",
  meaning: "Dash Wallet reopens without Dash Platform. Dash Platform and Shielded come back the next time you start Dash Wallet.",
  historyFailed: (wallet: string) =>
    `Dash Wallet couldn't load the history of ${wallet}, so it can't finish checking the wallet data it restored. Anything that moves your funds stays paused.`,
  clearFailed:
    "Dash Wallet finished checking the wallet data it restored, but couldn't save that the check is done, so anything that moves your funds stays paused. The disk may be full, or Dash Wallet may not be able to write to it. Free up some space, then try again. If this keeps happening, report a problem.",
  report: "Report a problem",
};
const THEMES = ["light", "dark"] as const;
const STYLESHEET = readFileSync(resolve(import.meta.dirname, "../styles/app.css"), "utf8");
const SOURCE = readFileSync(resolve(import.meta.dirname, "RestoredStatePanel.tsx"), "utf8");
const escaped = (text: string) => text.replace(/&/g, "&amp;").replace(/'/g, "&#x27;").replace(/"/g, "&quot;");
const noop = () => {};

const status = (s: Partial<RestoreStatus>): RestoreStatus => ({ waiting: [], historyWallets: [], clearFailed: false, ...s });
type ViewActions = Parameters<typeof RestoredStateView>[0]["actions"];
const idle: ViewActions = { running: false, problem: null, failedLink: null, retry: async () => {}, finishWithoutPlatform: async () => {}, report: async () => {} };
const view = (s: RestoreStatus, actions: Partial<ViewActions> = {}, nameOf = (id: string) => `name-of-${id}`) =>
  renderToStaticMarkup(createElement(RestoredStateView, { status: s, nameOf, actions: { ...idle, ...actions } }));

/** The `<button …>label</button>` with `testId`: its opening tag and its label; null when absent. */
function button(html: string, testId: string): { tag: string; label: string } | null {
  const m = new RegExp(`(<button[^>]*data-testid="${testId}"[^>]*>)([^<]*)</button>`).exec(html);
  return m ? { tag: m[1], label: m[2] } : null;
}
const lines = (html: string) => [...html.matchAll(/<p class="protocol-advisory-text" data-testid="([^"]+)">([^<]*)<\/p>/g)].map((m) => [m[1], m[2]]);

function expectStyledClasses(html: string) {
  const classes = new Set([...html.matchAll(/class="([^"]+)"/g)].flatMap((m) => m[1].split(/\s+/)).filter(Boolean));
  for (const c of classes) expect(STYLESHEET, c).toMatch(new RegExp(`\\.${c.replace(/-/g, "\\-")}[\\s,{:.>]`));
}

/** The panel as Home renders it, with RESTORED, `restore_status` and the wallet list as read. */
function panel(restored: boolean | undefined, s: RestoreStatus | null | undefined, wallets = [{ walletId: W, name: "Main" }]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  if (restored !== undefined) client.setQueryData(keys.restored, restored);
  if (s !== undefined) client.setQueryData(keys.restoreStatus, s);
  client.setQueryData(keys.wallets, wallets);
  const backend = { kind: "fixture", activeNetwork: () => new Promise(noop), restoreStatus: () => new Promise(noop), listWallets: () => new Promise(noop) } as unknown as Backend;
  return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(BackendContext.Provider, { value: backend }, createElement(RestoredStatePanel))));
}

afterEach(() => ui.set({ theme: null, prefs: DEFAULT_PREFS }));

describe("the restored state panel", () => {
  for (const theme of THEMES)
    it(`in ${theme}, says each wait on its own line, with MG's copy`, () => {
      ui.set({ theme });
      expect(lines(view(status({ waiting: ["coreSync"] })))).toEqual([["restored-waiting-coreSync", COPY.core]]);
      expect(lines(view(status({ waiting: ["platformPass"] })))).toEqual([["restored-waiting-platformPass", escaped(COPY.platform)]]);
      expect(lines(view(status({ waiting: ["historyLoad"] })))).toEqual([["restored-waiting-historyLoad", escaped(COPY.history)]]);
      const all = view(status({ waiting: ["coreSync", "platformPass", "historyLoad"] }));
      expect(lines(all).map(([id]) => id)).toEqual(["restored-waiting-coreSync", "restored-waiting-platformPass", "restored-waiting-historyLoad"]);
      expectStyledClasses(all);
    });

  it("renders the same markup in light and dark", () => {
    const s = status({ waiting: ["coreSync", "platformPass"], historyWallets: [W], clearFailed: true });
    const [light, dark] = THEMES.map((theme) => {
      ui.set({ theme });
      return view(s);
    });
    expect(dark).toBe(light);
  });

  it("offers Try again for every wait, and Report a problem with the About link's label", () => {
    expect(t("settings.about.links.issues")).toBe(COPY.report);
    for (const wait of ["coreSync", "platformPass", "historyLoad"] as const) {
      const html = view(status({ waiting: [wait] }));
      expect(button(html, "restored-retry")?.label).toBe(COPY.retry);
      expect(button(html, "restored-report")?.label).toBe(COPY.report);
    }
  });

  it("offers Finish without Dash Platform, with its meaning beside it, only while Platform's check is the wait", () => {
    const html = view(status({ waiting: ["coreSync", "platformPass"] }));
    const finish = button(html, "restored-finish-without-platform");
    expect(finish?.label).toBe(COPY.finish);
    const meaning = /<p id="([^"]+)" class="field-hint" data-testid="restored-finish-meaning">([^<]*)<\/p>/.exec(html);
    expect(meaning?.[2]).toBe(COPY.meaning);
    expect(finish?.tag).toContain(`aria-describedby="${meaning?.[1]}"`);
    for (const s of [status({ waiting: ["coreSync"] }), status({ waiting: ["historyLoad"] }), status({ historyWallets: [W] }), status({ clearFailed: true })]) {
      const other = view(s);
      expect(button(other, "restored-finish-without-platform")).toBeNull();
      expect(other).not.toContain("restored-finish-meaning");
      expect(offersFinishWithoutPlatform(s)).toBe(false);
    }
  });

  it("names a wallet whose history failed to load, and offers Report a problem only", () => {
    const html = view(status({ historyWallets: [W] }), {}, (id) => (id === W ? "Main" : id));
    expect(lines(html)).toEqual([["restored-history-failed", escaped(COPY.historyFailed("Main"))]]);
    expect(button(html, "restored-report")?.label).toBe(COPY.report);
    expect(button(html, "restored-retry")).toBeNull();
    expect(button(html, "restored-finish-without-platform")).toBeNull();
    expect(offersRetry(status({ historyWallets: [W] }))).toBe(false);
  });

  it("offers no Finish without Dash Platform beside a failed history, Platform's check waiting or not", () => {
    const s = status({ waiting: ["platformPass", "historyLoad"], historyWallets: [W] });
    const html = view(s);
    expect(offersFinishWithoutPlatform(s)).toBe(false);
    expect(button(html, "restored-finish-without-platform")).toBeNull();
    expect(button(html, "restored-retry")?.label).toBe(COPY.retry);
    expect(button(html, "restored-report")?.label).toBe(COPY.report);
  });

  it("is passed by the lock gate while closing and opening without Platform, as openNetwork is", () => {
    for (const command of ["openNetwork", "closeNetwork", "openNetworkWithoutPlatform"]) expect(LOCKED_COMMANDS.has(command), command).toBe(true);
  });

  it("names the wallet from the wallet list, and a wallet it doesn't list by its id's start", () => {
    const other = "ab".repeat(32);
    const html = panel(true, status({ historyWallets: [W, other] }));
    expect(lines(html)).toEqual([
      ["restored-history-failed", escaped(COPY.historyFailed("Main"))],
      ["restored-history-failed", escaped(COPY.historyFailed("abababab…"))],
    ]);
  });

  it("says a clear that couldn't be written, with Try again and Report a problem", () => {
    const html = view(status({ clearFailed: true }));
    expect(lines(html)).toEqual([["restored-clear-failed", escaped(COPY.clearFailed)]]);
    expect(button(html, "restored-retry")?.label).toBe(COPY.retry);
    expect(button(html, "restored-report")?.label).toBe(COPY.report);
    expect(button(html, "restored-finish-without-platform")).toBeNull();
  });

  it("shows nothing until RESTORED is set and restore_status says something", () => {
    const waiting = status({ waiting: ["coreSync"] });
    expect(panel(true, waiting)).toContain('data-testid="restored-state"');
    expect(panel(false, waiting)).toBe("");
    expect(panel(undefined, waiting)).toBe("");
    expect(panel(true, null)).toBe("");
    expect(panel(true, undefined)).toBe("");
    expect(panel(true, status({}))).toBe("");
  });

  it("disables its buttons while one runs, and shows the failure and a link that didn't open", () => {
    const html = view(status({ waiting: ["platformPass"] }), { running: true, problem: "the network is busy", failedLink: "https://example.invalid/issues" });
    for (const id of ["restored-retry", "restored-finish-without-platform", "restored-report"]) expect(button(html, id)?.tag).toContain('disabled=""');
    expect(html).toContain('data-testid="restored-state-problem">the network is busy</p>');
    expect(html).toContain('data-testid="restored-state-link-failed"');
    expect(html).toContain("https://example.invalid/issues");
  });

  it("is read again on sync only: no timer and no polling", () => {
    expect(SOURCE).not.toMatch(/setTimeout|setInterval|refetchInterval|requestAnimationFrame/);
    const queries = readFileSync(resolve(import.meta.dirname, "../api/queries.ts"), "utf8");
    const hook = /export function useRestoreStatus[\s\S]*?\n}\n/.exec(queries)?.[0] ?? "";
    expect(hook).toContain("keys.restoreStatus");
    expect(hook).not.toMatch(/refetchInterval|setTimeout|setInterval/);
  });
});

describe("the panel's actions", () => {
  type Api = ConstructorParameters<typeof RestoredStateActions>[0];
  const LOCK = { state: "locked", encrypted: true, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSecs: null, walletsWithSecrets: [] };
  const OPENED = { network: { kind: "testnet", dirName: "testnet", unitName: "tDASH", restored: true }, lock: LOCK };
  function fake(overrides: Partial<Record<keyof Api, () => Promise<unknown>>> = {}) {
    const calls: string[] = [];
    const answers: Record<keyof Api, unknown> = { restoreRetry: null, closeNetwork: true, openNetworkWithoutPlatform: OPENED, openNetwork: OPENED, openProjectLink: null };
    const api = Object.fromEntries(
      (Object.keys(answers) as (keyof Api)[]).map((name) => [
        name,
        (...args: unknown[]) => {
          calls.push(args.length ? `${name}(${args.join(",")})` : name);
          return overrides[name]?.() ?? Promise.resolve(answers[name]);
        },
      ]),
    ) as unknown as Api;
    return { api, calls };
  }
  /** A client with the reads Home has: none of them stale yet. */
  function client() {
    const c = new QueryClient();
    c.setQueryData(keys.restored, true);
    c.setQueryData(keys.restoreStatus, status({ waiting: ["platformPass"] }));
    c.setQueryData(keys.wallets, []);
    return c;
  }
  const stale = (c: QueryClient, key: readonly unknown[]) => c.getQueryState(key)?.isInvalidated;

  it("Try again calls restore_retry and reads the status and RESTORED again", async () => {
    const { api, calls } = fake();
    const c = client();
    const actions = new RestoredStateActions(api, c, noop);
    await actions.retry();
    expect(calls).toEqual(["restoreRetry"]);
    expect(stale(c, keys.restoreStatus)).toBe(true);
    expect(stale(c, keys.restored)).toBe(true);
    expect(stale(c, keys.wallets)).toBe(false);
    expect(actions.running).toBe(false);
    expect(actions.problem).toBeNull();
  });

  it("Try again that fails says why and reads nothing", async () => {
    const { api } = fake({ restoreRetry: () => Promise.reject(new Error("disk full")) });
    const c = client();
    const actions = new RestoredStateActions(api, c, noop);
    await actions.retry();
    expect(stale(c, keys.restoreStatus)).toBe(false);
    expect(actions.problem).toBe("disk full");
  });

  it("Finish without Dash Platform closes the network, then opens it without Platform, then boots the page again", async () => {
    const { api, calls } = fake();
    const boots: (string | null)[] = [];
    const actions = new RestoredStateActions(api, client(), (note) => {
      calls.push("boot");
      boots.push(note);
    });
    await actions.finishWithoutPlatform();
    expect(calls).toEqual(["closeNetwork", "openNetworkWithoutPlatform", "boot"]);
    expect(boots).toEqual([null]);
    expect(actions.running).toBe(true);
  });

  it("a refused close changes nothing: no open, no boot, the refusal said in place", async () => {
    const { api, calls } = fake({ closeNetwork: () => Promise.reject(new Error("a payment is being sent")) });
    const boot = vi.fn();
    const actions = new RestoredStateActions(api, client(), boot);
    await actions.finishWithoutPlatform();
    expect(calls).toEqual(["closeNetwork"]);
    expect(boot).not.toHaveBeenCalled();
    expect(actions.problem).toBe("a payment is being sent");
    expect(actions.running).toBe(false);
  });

  it("an open without Platform that fails opens the network as usual, and the panel stays with MG's line", async () => {
    const { api, calls } = fake({ openNetworkWithoutPlatform: () => Promise.reject(new Error("storage in use")) });
    const c = client();
    const boot = vi.fn();
    const actions = new RestoredStateActions(api, c, boot);
    await actions.finishWithoutPlatform();
    expect(calls).toEqual(["closeNetwork", "openNetworkWithoutPlatform", "openNetwork"]);
    expect(boot).not.toHaveBeenCalled();
    expect(actions.problem).toBe("Dash Wallet couldn't reopen without Dash Platform, so it opened as usual. You can try again from here. Details: storage in use");
    expect(actions.running).toBe(false);
    // The new session's reads: RESTORED from its open, the vault's lock, everything else read again.
    expect(c.getQueryData(keys.restored)).toBe(true);
    expect(c.getQueryData(keys.lockStatus)).toEqual(LOCK);
    expect(stale(c, keys.restoreStatus)).toBe(true);
    expect(stale(c, keys.wallets)).toBe(true);
    // The panel stays, its buttons offered, with the line under them.
    const html = view(status({ waiting: ["platformPass"] }), actions);
    for (const id of ["restored-retry", "restored-finish-without-platform", "restored-report"]) expect(button(html, id)?.tag).not.toContain("disabled");
    expect(html).toContain(`data-testid="restored-state-problem">${escaped(actions.problem!)}</p>`);
  });

  it("when the usual open fails too, the page boots again and its own open says why", async () => {
    const fail = () => Promise.reject(new Error("storage in use"));
    const { api, calls } = fake({ openNetworkWithoutPlatform: fail, openNetwork: fail });
    const boots: (string | null)[] = [];
    const actions = new RestoredStateActions(api, client(), (note) => void boots.push(note));
    await actions.finishWithoutPlatform();
    expect(calls).toEqual(["closeNetwork", "openNetworkWithoutPlatform", "openNetwork"]);
    expect(boots).toEqual([null]);
  });

  it("is one per app, so the line outlives the panel (a reopen may bring the lock screen)", () => {
    const c = client();
    const backend = fake().api as unknown as Backend;
    expect(restoredStateActions(backend, c)).toBe(restoredStateActions(backend, c));
    expect(restoredStateActions(backend, client())).not.toBe(restoredStateActions(backend, c));
  });

  it("Report a problem opens the issues link, and keeps a link that didn't open to copy", async () => {
    const { api, calls } = fake();
    const actions = new RestoredStateActions(api, client(), noop);
    await actions.report();
    expect(calls).toEqual(["openProjectLink(issues)"]);
    expect(actions.failedLink).toBeNull();
    const failing = fake({ openProjectLink: () => Promise.reject({ code: "app.browser_open", message: "no browser", data: { kind: "link", url: "https://example.invalid/issues" } }) });
    const again = new RestoredStateActions(failing.api, client(), noop);
    await again.report();
    expect(again.failedLink).toBe("https://example.invalid/issues");
    expect(again.problem).toBeNull();
  });
});

describe("the fixture's restore status", () => {
  let backend: IpcBackend;
  beforeEach(() => {
    vi.stubGlobal("window", globalThis);
    const stored = new Map<string, string>();
    vi.stubGlobal("location", { reload: vi.fn() });
    vi.stubGlobal("sessionStorage", { getItem: (k: string) => stored.get(k) ?? null, setItem: (k: string, v: string) => stored.set(k, v), removeItem: (k: string) => stored.delete(k) });
    backend = new IpcBackend("fixture");
  });
  afterEach(() => {
    clearMocks();
    vi.unstubAllGlobals();
  });

  it("answers each wait state, the failed history and the failed clear, and null once RESTORED is clear", async () => {
    const cases = [
      ["restored-waiting-core", status({ waiting: ["coreSync"] })],
      ["restored-waiting-platform", status({ waiting: ["platformPass"] })],
      ["restored-waiting-history", status({ waiting: ["historyLoad"] })],
      ["restored-history-failed", status({ historyWallets: [W] })],
      ["restored-clear-failed", status({ clearFailed: true })],
    ] as const;
    for (const [scenario, expected] of cases) {
      installFixture({ scenario });
      expect(isRestored(await backend.activeNetwork())).toBe(true);
      expect(await backend.restoreStatus()).toEqual(expected);
      clearMocks();
    }
    installFixture({ scenario: "funded" });
    expect(await backend.restoreStatus()).toBeNull();
  });

  it("Finish without Dash Platform: close, then an open with noPlatform, and RESTORED clears", async () => {
    const fixture = installFixture({ scenario: "restored-waiting-platform" });
    const boot = vi.fn();
    const actions = new RestoredStateActions(backend, new QueryClient(), boot);
    await actions.finishWithoutPlatform();
    expect(fixture.calls.slice(-2)).toEqual(["close_network", "open_network"]);
    expect(fixture.world.noPlatform).toBe(true);
    expect(boot).toHaveBeenCalledWith(null);
    expect(isRestored(await backend.activeNetwork())).toBe(false);
    expect(await backend.restoreStatus()).toBeNull();
    // The page boots again on the same session: the world it carries has RESTORED clear.
    expect(JSON.parse(sessionStorage.getItem("dw-fixture-relaunch")!).restored).toBe(false);
  });

  it("an open without Platform that fails: the network opens as usual, RESTORED still set and Platform's check still the wait", async () => {
    const fixture = installFixture({ scenario: "restored-finish-fails" });
    const boot = vi.fn();
    const actions = new RestoredStateActions(backend, new QueryClient(), boot);
    await actions.finishWithoutPlatform();
    expect(fixture.calls.slice(-3)).toEqual(["close_network", "open_network", "open_network"]);
    expect(boot).not.toHaveBeenCalled();
    expect(actions.problem).toContain("Details: fixture: opening testnet without Dash Platform failed");
    expect(fixture.world.noPlatform).toBeUndefined();
    expect(isRestored(await backend.activeNetwork())).toBe(true);
    expect(await backend.restoreStatus()).toEqual(status({ waiting: ["platformPass"] }));
  });

  it("hides Finish without Dash Platform when Platform is already off: its check no longer counts", async () => {
    const fixture = installFixture({ scenario: "restored-waiting-core" });
    fixture.world.restoreStatus = status({ waiting: ["coreSync", "platformPass"] });
    expect(offersFinishWithoutPlatform((await backend.restoreStatus())!)).toBe(true);
    fixture.world.noPlatform = true;
    const off = (await backend.restoreStatus())!;
    expect(off).toEqual(status({ waiting: ["coreSync"] }));
    expect(button(view(off), "restored-finish-without-platform")).toBeNull();
  });

  it("Try again writes the clear a failed one couldn't, and RESTORED clears; a wait stays", async () => {
    const fixture = installFixture({ scenario: "restored-clear-failed" });
    await backend.restoreRetry();
    expect(fixture.world.restoreCalls).toContain("restoreRetry");
    expect(isRestored(await backend.activeNetwork())).toBe(false);
    clearMocks();
    installFixture({ scenario: "restored-waiting-core" });
    await backend.restoreRetry();
    expect(await backend.restoreStatus()).toEqual(status({ waiting: ["coreSync"] }));
  });
});
