// What RESTORED waits for (B addendum 1, MG C3 v2), under the restored banner: a line for each wait,
// Try again, Finish without Dash Platform only while Platform's first check is the wait, a history that
// failed to load named with Report a problem only, and a clear that couldn't be written with Try again
// and Report a problem. Read again on sync, never on a timer.

import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { clearMocks } from "@tauri-apps/api/mocks";
import { QueryClient, QueryClientProvider, QueryObserver } from "@tanstack/react-query";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../api/backend";
import { LOCKED_COMMANDS } from "../api/lockGate";
import { setRestoreFinishing, useRestoredGate } from "../api/restoreGate";
import { BackendContext, keys } from "../api/queries";
import { switchOutcome } from "../network/switch";
import type { RestoreStatus } from "../api/generated/RestoreStatus";
import type { RestoreWait } from "../api/generated/RestoreWait";
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
  platform: "Waiting for DashPay Desktop's first check with Dash Platform.",
  history: "Waiting for a wallet's history to load.",
  retry: "Try again",
  finish: "Finish without Dash Platform",
  meaning: "DashPay Desktop reopens without Dash Platform, so your Dash wallet funds can be used again. Dash Platform features and Shielded syncing are off until you next start DashPay Desktop, and your Platform and Shielded funds stay paused until DashPay Desktop has checked them with Dash Platform.",
  historyFailed: (wallet: string) =>
    `DashPay Desktop couldn't load the history of ${wallet}, so it can't finish checking the wallet data it restored. Anything that moves your funds stays paused.`,
  clearFailed:
    "DashPay Desktop finished checking the wallet data it restored, but couldn't save that the check is done, so anything that moves your funds stays paused. The disk may be full, or DashPay Desktop may not be able to write to it. Free up some space, then try again. If this keeps happening, report a problem.",
  report: "Report a problem",
  platformOnly:
    "Your Dash wallet funds can be used again. Your Platform and Shielded funds stay paused until DashPay Desktop has finished checking this wallet with Dash Platform. That check runs while Dash Platform is on.",
};
const THEMES = ["light", "dark"] as const;
const STYLESHEET = readFileSync(resolve(import.meta.dirname, "../styles/app.css"), "utf8");
const SOURCE = readFileSync(resolve(import.meta.dirname, "RestoredStatePanel.tsx"), "utf8");
const escaped = (text: string) => text.replace(/&/g, "&amp;").replace(/'/g, "&#x27;").replace(/"/g, "&quot;");
const noop = () => {};

const status = (s: Partial<RestoreStatus>): RestoreStatus => ({ waiting: [], historyWallets: [], clearFailed: false, platformOnly: false, platformOff: false, ...s });
const PLATFORM_ONLY = status({ waiting: ["platformPass"], platformOnly: true });
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

/** Home's panel on `client`'s reads, with the app's actions for that client. */
function panelOn(client: QueryClient) {
  const backend = { kind: "fixture", activeNetwork: () => new Promise(noop), restoreStatus: () => new Promise(noop), listWallets: () => new Promise(noop) } as unknown as Backend;
  return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(BackendContext.Provider, { value: backend }, createElement(RestoredStatePanel))));
}

afterEach(() => {
  ui.set({ theme: null, prefs: DEFAULT_PREFS });
  setRestoreFinishing(false);
});

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

  it("is passed by the lock gate, as openNetwork is: closing and opening without Platform is one call", () => {
    for (const command of ["openNetwork", "finishWithoutPlatform"]) expect(LOCKED_COMMANDS.has(command), command).toBe(true);
    for (const command of ["closeNetwork", "openNetworkWithoutPlatform"]) expect(LOCKED_COMMANDS.has(command), command).toBe(false);
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

  it("Core funds free, Platform and Shielded funds held: MG's line, Platform's wait, Try again and Report a problem, never Finish", () => {
    for (const theme of THEMES) {
      ui.set({ theme });
      const html = view(PLATFORM_ONLY);
      expect(lines(html), theme).toEqual([
        ["restored-platform-only", escaped(COPY.platformOnly)],
        ["restored-waiting-platformPass", escaped(COPY.platform)],
      ]);
      expect(button(html, "restored-retry")?.label).toBe(COPY.retry);
      expect(button(html, "restored-report")?.label).toBe(COPY.report);
      expect(button(html, "restored-finish-without-platform")).toBeNull();
      expect(html).not.toContain("restored-finish-meaning");
      expectStyledClasses(html);
    }
    expect(offersFinishWithoutPlatform(PLATFORM_ONLY)).toBe(false);
    expect(offersRetry(PLATFORM_ONLY)).toBe(true);
  });

  it("has no line, Try again or Finish for waiting on a wallet: the panel can't be reached without one", () => {
    const wallet: RestoreWait = "wallet";
    const alone = status({ waiting: [wallet] });
    const html = view(alone);
    expect(html).not.toContain("restored-waiting-");
    expect(button(html, "restored-finish-without-platform")).toBeNull();
    expect(button(html, "restored-retry")).toBeNull();
    expect(offersFinishWithoutPlatform(alone)).toBe(false);
    expect(offersFinishWithoutPlatform(status({ waiting: [wallet, "platformPass"] }))).toBe(false);
    expect(panel(true, alone)).toBe("");
    expect(lines(view(status({ waiting: [wallet, "coreSync"] })))).toEqual([["restored-waiting-coreSync", COPY.core]]);
  });

  it("without Platform this session, Platform's check left: MG's line, its wait and Report a problem, no Try again; other waits keep it", () => {
    const off = status({ waiting: ["platformPass"], platformOnly: true, platformOff: true });
    const html = view(off);
    expect(lines(html)).toEqual([
      ["restored-platform-only", escaped(COPY.platformOnly)],
      ["restored-waiting-platformPass", escaped(COPY.platform)],
    ]);
    expect(button(html, "restored-retry")).toBeNull();
    expect(button(html, "restored-finish-without-platform")).toBeNull();
    expect(button(html, "restored-report")?.label).toBe(COPY.report);
    expect(offersRetry(PLATFORM_ONLY)).toBe(true);
    // A clear that failed is tried again without Platform too: Try again cuts its wait short.
    expect(offersRetry(status({ clearFailed: true, platformOff: true }))).toBe(true);
    expect(offersRetry(status({ waiting: ["coreSync"], platformOff: true }))).toBe(true);
  });

  it("shows while RESTORED is set and restore_status says something, or while it holds only Platform and Shielded funds", () => {
    const waiting = status({ waiting: ["coreSync"] });
    expect(panel(true, waiting)).toContain('data-testid="restored-state"');
    expect(panel(false, PLATFORM_ONLY)).toContain('data-testid="restored-platform-only"');
    expect(panel(false, null)).toBe("");
    expect(panel(false, waiting)).toBe("");
    expect(panel(undefined, waiting)).toBe("");
    expect(panel(undefined, PLATFORM_ONLY)).toBe("");
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
  const WAITING_PLATFORM = status({ waiting: ["platformPass"] });
  const busy = { code: "app.network_busy", message: "closing the network would cut what runs", data: { kind: "networkBusy", flows: ["registration"], mixing: false, broadcast: false, stalled: false, draining: false } };
  const switchFailed = (rolledBack: boolean) => ({ code: "app.network_switch_failed", message: "storage in use", data: { kind: "networkSwitch", network: { kind: "testnet", dirName: "testnet", unitName: "tDASH" }, rolledBack } });
  function fake(overrides: Partial<Record<keyof Api, () => Promise<unknown>>> = {}) {
    const calls: string[] = [];
    const api = Object.fromEntries(
      (["restoreRetry", "finishWithoutPlatform", "openProjectLink"] as const).map((name) => [
        name,
        (...args: unknown[]) => {
          calls.push(args.length ? `${name}(${args.join(",")})` : name);
          return overrides[name]?.() ?? Promise.resolve(null);
        },
      ]),
    ) as unknown as Api;
    return { api, calls };
  }
  /** A client with the reads Home has: none of them stale yet. */
  function client() {
    const c = new QueryClient();
    c.setQueryData(keys.restored, true);
    c.setQueryData(keys.restoreStatus, WAITING_PLATFORM);
    c.setQueryData(keys.wallets, []);
    return c;
  }
  const stale = (c: QueryClient, key: readonly unknown[]) => c.getQueryState(key)?.isInvalidated;

  /** The money gate's reading of RESTORED, as TxDetailSheet and Coin Selection take it. */
  function gate(c: QueryClient): string {
    const Probe = () => createElement("i", null, String(useRestoredGate()));
    const backend = { activeNetwork: () => new Promise(noop) } as unknown as Backend;
    return renderToStaticMarkup(createElement(QueryClientProvider, { client: c }, createElement(BackendContext.Provider, { value: backend }, createElement(Probe))));
  }

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

  it("Finish without Dash Platform is one finish_without_platform call, then the page boots again", async () => {
    const { api, calls } = fake();
    const boots: (string | null)[] = [];
    const actions = new RestoredStateActions(api, client(), (note) => {
      calls.push("boot");
      boots.push(note);
    });
    await actions.finishWithoutPlatform(WAITING_PLATFORM);
    expect(calls).toEqual(["finishWithoutPlatform", "boot"]);
    expect(boots).toEqual([null]);
    expect(actions.running).toBe(true);
  });

  it("while it runs, the panel stays busy and the gate reads RESTORED as unknown, even if the closed session reads clear", async () => {
    let answer: (v: unknown) => void = noop;
    const { api } = fake({ finishWithoutPlatform: () => new Promise((_, refuse) => (answer = refuse)) });
    const c = client();
    const actions = restoredStateActions(api as Backend, c);
    expect(gate(c)).toBe("<i>true</i>");
    const finishing = actions.finishWithoutPlatform(WAITING_PLATFORM);
    // The session-closed blip: `restored` reads the closed network as not restored.
    c.setQueryData(keys.restored, false);
    c.setQueryData(keys.restoreStatus, null);
    expect(gate(c)).toBe("<i>undefined</i>");
    expect(actions.running).toBe(true);
    expect(actions.finishing).toEqual(WAITING_PLATFORM);
    const html = panelOn(c);
    expect(html).toContain('aria-busy="true"');
    expect(html).toContain('data-testid="restored-waiting-platformPass"');
    for (const id of ["restored-retry", "restored-finish-without-platform", "restored-report"]) expect(button(html, id)?.tag).toContain('disabled=""');
    answer(busy);
    await finishing;
    expect(gate(c)).toBe("<i>false</i>");
  });

  it("a busy network refuses it: nothing changes, no boot, and the switch's busy refusal is said in place", async () => {
    const { api, calls } = fake({ finishWithoutPlatform: () => Promise.reject(busy) });
    const boot = vi.fn();
    const c = client();
    const actions = new RestoredStateActions(api, c, boot);
    await actions.finishWithoutPlatform(WAITING_PLATFORM);
    expect(calls).toEqual(["finishWithoutPlatform"]);
    expect(boot).not.toHaveBeenCalled();
    expect(actions.problem).toBe("Wait for username registration to finish before DashPay Desktop can reopen without Dash Platform.");
    expect(actions.running).toBe(false);
    expect(actions.finishing).toBeNull();
    expect(gate(c)).toBe("<i>true</i>");
    expect(stale(c, keys.wallets)).toBe(false);
  });

  it("rolled back: it opened as usual, every read is read again, and the panel stays with MG's line", async () => {
    const { api, calls } = fake({ finishWithoutPlatform: () => Promise.reject(switchFailed(true)) });
    const c = client();
    const actions = restoredStateActions(api as Backend, c);
    await actions.finishWithoutPlatform(WAITING_PLATFORM);
    expect(calls).toEqual(["finishWithoutPlatform"]);
    expect(actions.problem).toBe("DashPay Desktop couldn't reopen without Dash Platform, so it opened as usual. You can try again from here. Details: storage in use");
    expect(actions.running).toBe(false);
    expect(actions.finishing).toBeNull();
    expect(stale(c, keys.restored)).toBe(true);
    expect(stale(c, keys.restoreStatus)).toBe(true);
    expect(stale(c, keys.wallets)).toBe(true);
    const html = panelOn(c);
    for (const id of ["restored-retry", "restored-finish-without-platform", "restored-report"]) expect(button(html, id)?.tag).not.toContain("disabled");
    expect(html).toContain(`data-testid="restored-state-problem">${escaped(actions.problem!)}</p>`);
  });

  it("every money gate reads RESTORED through the gate: only the banner, this panel, the gate and the restore-only add-wallet screens read restored itself", () => {
    const src = resolve(import.meta.dirname, "..");
    const readers = readdirSync(src, { recursive: true, encoding: "utf8" })
      .filter((f) => /\.tsx?$/.test(f) && !/\.test\.tsx?$/.test(f))
      .filter((f) => /(?<!function )useRestored\(\)/.test(readFileSync(resolve(src, f), "utf8")));
    expect(readers.sort()).toEqual(["api/restoreGate.ts", "screens/Onboarding.tsx", "screens/RestoredBanner.tsx", "screens/RestoredStatePanel.tsx", "screens/settings/Backups.tsx", "screens/settings/Wallets.tsx"]);
    for (const file of ["screens/TxDetailSheet.tsx", "screens/tools/CoinSelection.tsx"]) expect(readFileSync(resolve(src, file), "utf8"), file).toContain("useRestoredGate()");
  });

  it("a restored not known (never read, or answered without it) offers no panel action and keeps the gate unknown", () => {
    for (const restored of [undefined, null]) {
      const c = client();
      if (restored === null) c.setQueryData(keys.restored, null);
      else c.removeQueries({ queryKey: keys.restored });
      expect(gate(c), String(restored)).toBe("<i>undefined</i>");
      const html = panelOn(c);
      expect(html, String(restored)).not.toContain("<button");
    }
    const known = client();
    expect(gate(known)).toBe("<i>true</i>");
    expect(panelOn(known)).toContain('data-testid="restored-retry"');
  });

  it("Core funds free, Platform and Shielded funds held: the gate reads RESTORED clear, and the panel offers no Finish", () => {
    const c = client();
    c.setQueryData(keys.restored, false);
    c.setQueryData(keys.restoreStatus, PLATFORM_ONLY);
    expect(gate(c)).toBe("<i>false</i>");
    const html = panelOn(c);
    expect(html).toContain('data-testid="restored-platform-only"');
    expect(html).toContain('data-testid="restored-retry"');
    expect(html).not.toContain('data-testid="restored-finish-without-platform"');
  });

  it("any other failure is said in place and the gate reads RESTORED again", async () => {
    const { api } = fake({ finishWithoutPlatform: () => Promise.reject(new Error("This build of DashPay Desktop can't do that yet.")) });
    const c = client();
    const actions = new RestoredStateActions(api, c, vi.fn());
    await actions.finishWithoutPlatform(WAITING_PLATFORM);
    expect(actions.problem).toBe("This build of DashPay Desktop can't do that yet.");
    expect(actions.finishing).toBeNull();
    expect(actions.running).toBe(false);
    expect(gate(c)).toBe("<i>true</i>");
  });

  it("rolled back: the gate stays unknown until RESTORED is read on the new session, and unknown if that read fails", async () => {
    for (const read of ["ok", "fails"] as const) {
      let answer: (v: boolean) => void = noop;
      let refuse: (e: Error) => void = noop;
      const c = client();
      const observer = new QueryObserver(c, { queryKey: keys.restored, queryFn: () => new Promise<boolean>((r, f) => ((answer = r), (refuse = f))), retry: false });
      const stop = observer.subscribe(noop);
      const { api } = fake({ finishWithoutPlatform: () => Promise.reject(switchFailed(true)) });
      const actions = new RestoredStateActions(api, c, vi.fn());
      const finishing = actions.finishWithoutPlatform(WAITING_PLATFORM);
      await vi.waitFor(() => expect(c.getQueryState(keys.restored)?.fetchStatus).toBe("fetching"));
      expect(gate(c)).toBe("<i>undefined</i>");
      if (read === "ok") answer(true);
      else refuse(new Error("no network"));
      await finishing;
      expect(actions.finishing).toBeNull();
      expect(gate(c)).toBe(read === "ok" ? "<i>true</i>" : "<i>undefined</i>");
      stop();
    }
  });

  it("says every refusal in MG's words, a refused close in the engine's, never the network switch's", async () => {
    const refusal = (data: object, code = "app.network_busy") => ({ code, message: "refused", data });
    const busyData = (over: object) => ({ kind: "networkBusy", flows: [], mixing: false, broadcast: false, stalled: false, draining: false, ...over });
    const cases: [object, string][] = [
      [refusal(busyData({ flows: ["registration"] })), "Wait for username registration to finish before DashPay Desktop can reopen without Dash Platform."],
      [refusal(busyData({ flows: ["registration"], mixing: true })), "Wait for username registration to finish, or stop mixing, before DashPay Desktop can reopen without Dash Platform."],
      [refusal(busyData({ broadcast: true })), "Wait for the payment to finish before DashPay Desktop can reopen without Dash Platform."],
      [refusal(busyData({ mixing: true })), "Stop mixing before DashPay Desktop can reopen without Dash Platform."],
      [refusal(busyData({ draining: true })), "DashPay Desktop is still finishing a request. Choose Finish without Dash Platform again once it's done."],
      [refusal({ kind: "networkCloseRefused", reason: "walletOpening" }, "wallet"), "refused"],
      [refusal({ kind: "networkCloseRefused", reason: "shieldedSpendUnresolved" }, "shielded_spend_unresolved"), "refused"],
    ];
    const c = client();
    c.setQueryData(keys.lockStatus, { state: "unlocked", encrypted: true, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSecs: null, walletsWithSecrets: [] });
    const stalled = refusal(busyData({ flows: ["registration"], stalled: true }));
    cases.push([stalled, "Wait for username registration to finish before DashPay Desktop can reopen without Dash Platform. Locking the wallet stops username registration."]);
    for (const [e, line] of cases) {
      const { api } = fake({ finishWithoutPlatform: () => Promise.reject(e) });
      const actions = new RestoredStateActions(api, c, vi.fn());
      await actions.finishWithoutPlatform(WAITING_PLATFORM);
      expect(actions.problem).toBe(line);
      const html = view(WAITING_PLATFORM, actions);
      expect(html).toContain(escaped(line));
      expect(html.toLowerCase()).not.toMatch(/switch(ing)? networks?/);
    }
  });

  it("the network switch keeps its own lines", () => {
    const busy = { code: "app.network_busy", message: "refused", data: { kind: "networkBusy", flows: ["registration"], mixing: false, broadcast: false, stalled: false, draining: false } };
    expect(switchOutcome(busy, "mainnet", "testnet")).toEqual({ problem: "Wait for username registration to finish before switching networks." });
  });

  it("not rolled back: no network is open, so the page boots again and its own open says why", async () => {
    const { api } = fake({ finishWithoutPlatform: () => Promise.reject(switchFailed(false)) });
    const boots: (string | null)[] = [];
    const c = client();
    const actions = new RestoredStateActions(api, c, (note) => void boots.push(note));
    await actions.finishWithoutPlatform(WAITING_PLATFORM);
    expect(boots).toEqual([null]);
    expect(actions.problem).toBeNull();
    expect(gate(c)).toBe("<i>undefined</i>");
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
      ["restored-status-history-failed", status({ historyWallets: [W] })],
      ["restored-status-clear-failed", status({ clearFailed: true })],
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

  it("Core funds free, Platform and Shielded funds held: RESTORED clear, and the status says only Platform's check", async () => {
    installFixture({ scenario: "restored-status-platform-only" });
    expect(isRestored(await backend.activeNetwork())).toBe(false);
    expect(await backend.restoreStatus()).toEqual(PLATFORM_ONLY);
    expect((await backend.finishWithoutPlatform()).network.kind).toBe("testnet");
    expect(await backend.restoreStatus()).toEqual(PLATFORM_ONLY);
  });

  it("Finish without Dash Platform: one finish_without_platform; RESTORED clears on the session it opened, Platform and Shielded funds stay held", async () => {
    const fixture = installFixture({ scenario: "restored-waiting-platform" });
    const boot = vi.fn();
    const actions = new RestoredStateActions(backend, new QueryClient(), boot);
    await actions.finishWithoutPlatform(status({ waiting: ["platformPass"] }));
    expect(fixture.calls.at(-1)).toBe("finish_without_platform");
    expect(fixture.calls).not.toContain("close_network");
    expect(fixture.world.noPlatform).toBe(true);
    expect(boot).toHaveBeenCalledWith(null);
    expect(isRestored(await backend.activeNetwork())).toBe(false);
    expect(await backend.restoreStatus()).toEqual(status({ waiting: ["platformPass"], platformOnly: true, platformOff: true }));
    // The page boots again on the same session: the world it carries has RESTORED clear.
    expect(JSON.parse(sessionStorage.getItem("dw-fixture-switch")!).active.restored).toBe(false);
  });

  it("does nothing unless Platform's first check is the wait: no close, the same session", async () => {
    const fixture = installFixture({ scenario: "restored-waiting-core" });
    fixture.world.finishWithoutPlatform = "rolledBack";
    const opened = await backend.finishWithoutPlatform();
    expect(opened.network.kind).toBe("testnet");
    expect(fixture.world.lock.state).toBe("unlocked");
    expect(fixture.world.noPlatform).toBeUndefined();
    expect(fixture.world.restored).toBe(true);
  });

  it("draining: refused as the engine refuses it, nothing changed", async () => {
    const fixture = installFixture({ scenario: "restored-waiting-platform" });
    fixture.world.finishWithoutPlatform = "draining";
    await expect(backend.finishWithoutPlatform()).rejects.toMatchObject({ code: "app.network_busy", data: { kind: "networkBusy", draining: true, flows: [] } });
    expect(fixture.world.lock.state).toBe("unlocked");
    expect(isRestored(await backend.activeNetwork())).toBe(true);
  });

  it("busy: refused, nothing changed, RESTORED still set and Platform's check still the wait", async () => {
    const fixture = installFixture({ scenario: "restored-finish-busy" });
    const actions = new RestoredStateActions(backend, new QueryClient(), vi.fn());
    await actions.finishWithoutPlatform(status({ waiting: ["platformPass"] }));
    expect(actions.problem).toBe("Wait for username registration to finish before DashPay Desktop can reopen without Dash Platform.");
    expect(fixture.world.lock.state).toBe("unlocked");
    expect(fixture.world.noPlatform).toBeUndefined();
    expect(isRestored(await backend.activeNetwork())).toBe(true);
  });

  it("rolled back: the network opens as usual, RESTORED still set and Platform's check still the wait", async () => {
    const fixture = installFixture({ scenario: "restored-finish-rolled-back" });
    await expect(new IpcBackend("fixture").finishWithoutPlatform()).rejects.toMatchObject({
      code: "app.network_switch_failed",
      data: { kind: "networkSwitch", network: { kind: "testnet" }, rolledBack: true },
    });
    fixture.world.finishWithoutPlatform = "rolledBack";
    const boot = vi.fn();
    const actions = new RestoredStateActions(backend, new QueryClient(), boot);
    await actions.finishWithoutPlatform(status({ waiting: ["platformPass"] }));
    expect(boot).not.toHaveBeenCalled();
    expect(actions.problem).toContain("Details: testnet did not open without Platform: fixture");
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
    expect(off).toEqual(status({ waiting: ["coreSync"], platformOff: true }));
    expect(button(view(off), "restored-finish-without-platform")).toBeNull();
  });

  it("Try again writes the clear a failed one couldn't, and RESTORED clears; a wait stays", async () => {
    const fixture = installFixture({ scenario: "restored-status-clear-failed" });
    await backend.restoreRetry();
    expect(fixture.world.restoreCalls).toContain("restoreRetry");
    expect(isRestored(await backend.activeNetwork())).toBe(false);
    clearMocks();
    installFixture({ scenario: "restored-waiting-core" });
    await backend.restoreRetry();
    expect(await backend.restoreStatus()).toEqual(status({ waiting: ["coreSync"] }));
  });
});
