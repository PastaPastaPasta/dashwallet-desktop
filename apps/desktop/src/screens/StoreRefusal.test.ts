// The screens shown instead of the wallet when its data can't be opened: the engine's store refusal
// screens as `screen_for` gives them today, and StorageInUse in both variants, in light and dark. Neither
// ever falls through to onboarding, an empty wallet list or a zero balance.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../api/backend";
import { BackendContext, keys } from "../api/queries";
import type { StoreRefusedData, StoreScreen } from "../api/storeOpen";
import { App } from "../app/App";
import { DEFAULT_PREFS, ui } from "../app/ui";
import { STORE_COPY, STORE_SCREEN_NAMES, STORE_SCREENS, storageInUseError, storeRefusedError } from "../fixture/errors";
import { HANDLERS } from "../fixture/handlers";
import { PENDING_HANDLERS } from "../fixture/pending";
import { buildWorld } from "../fixture/scenarios";
import { t } from "../i18n";
import { flatten } from "../i18n/core";
import { en } from "../i18n/en";
import { buttonAction, OpenFailureActions, StorageInUseView, StoreRefusalView } from "./StoreRefusal";

// Server rendering needs a snapshot for every external store the app reads; the client's stands in.
vi.mock("react", async (importOriginal) => {
  const real = await importOriginal<typeof import("react")>();
  const useSyncExternalStore: typeof real.useSyncExternalStore = (subscribe, get, server) => real.useSyncExternalStore(subscribe, get, server ?? get);
  return { ...real, default: { ...real, useSyncExternalStore }, useSyncExternalStore };
});

const FOLDER = "/home/user/.local/share/dashwallet/testnet";
const THEMES = ["light", "dark"] as const;
const S = en.app.storageInUse;
const ENGLISH = flatten(en);

afterEach(() => ui.set({ theme: null, prefs: DEFAULT_PREFS }));

/** React's escaping of text in markup. */
const escaped = (text: string) => text.replace(/&/g, "&amp;").replace(/'/g, "&#x27;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

const noop = () => {};

/** The `<button …>label</button>` with `testId`: its opening tag and its label; null when absent. */
function button(html: string, testId: string): { tag: string; label: string } | null {
  const m = new RegExp(`(<button[^>]*data-testid="${testId.replace(/\./g, "\\.")}"[^>]*>)([^<]*)</button>`).exec(html);
  return m ? { tag: m[1], label: m[2] } : null;
}
const renderRefusal = (refusal: StoreRefusedData, problem: string | null = null) => renderToStaticMarkup(createElement(StoreRefusalView, { refusal, problem, onQuit: noop }));
const renderInUse = (retryable: boolean, problem: string | null = null, retrying = false) =>
  renderToStaticMarkup(createElement(StorageInUseView, { retryable, retrying, problem, onRetry: noop, onRestart: noop }));

/** The markup `render` gives in dark is the one it gives in light: the theme is the stylesheet's alone. */
function sameInBothThemes(render: () => string) {
  const [light, dark] = THEMES.map((theme) => {
    ui.set({ theme });
    return render();
  });
  expect(dark).toBe(light);
}

/** What only the shell, onboarding or a wallet's figures would show. */
function expectNoWalletFallthrough(html: string) {
  for (const id of ["onboarding-welcome", "onboarding-create", "onboarding-restore", "wallets-error", "wallets-loading", "hero", "history-empty", "sidebar-activity", "toolbar-lock"])
    expect(html).not.toContain(`data-testid="${id}"`);
  for (const text of [en.onboarding.welcome.title, en.onboarding.welcome.create, en.onboarding.welcome.restore, en.onboarding.welcome.addTitle]) expect(html).not.toContain(escaped(text));
  expect(html).not.toMatch(/\b0(\.0+)? t?DASH\b/);
}

/** Every class the markup uses is one app.css styles from the generated tokens, in both themes. */
const STYLESHEET = readFileSync(resolve(import.meta.dirname, "../styles/app.css"), "utf8");
function expectStyledClasses(html: string) {
  // The header's empty side cells, as the existing notices have them, take the grid's columns alone.
  const classes = new Set([...html.matchAll(/class="([^"]+)"/g)].flatMap((m) => m[1].split(/\s+/)).filter((c) => c && c !== "onboarding-header-side"));
  for (const c of classes) expect(STYLESHEET, c).toMatch(new RegExp(`\\.${c.replace(/-/g, "\\-")}[\\s,{:.>]`));
}

/** Every catalog key and English text a screen carries. */
const texts = (s: StoreScreen): [string, string][] => [[s.titleKey, s.title], [s.textKey, s.text], ...s.buttons.map((b): [string, string] => [b.key, b.label]), ...(s.hint ? [[s.hint.key, s.hint.text] as [string, string]] : [])];

const FIXTURES = STORE_SCREEN_NAMES.map((name) => [name, STORE_SCREENS[name](FOLDER)] as const);

describe("the store refusal screen", () => {
  for (const theme of THEMES)
    describe(`in ${theme}`, () => {
      it.each(FIXTURES)("shows %s as the engine gives it", (_, refusal) => {
        ui.set({ theme });
        const { screen } = refusal;
        const html = renderRefusal(refusal);
        expect(html).toContain('role="alertdialog"');
        expect(html).toContain(`>${escaped(screen.title)}</h1>`);
        expect(html).toContain(`data-testid="store-refused-text">${escaped(screen.text)}</p>`);
        expect(html).toContain(FOLDER);
        expect(html).not.toContain("{folder}");
        for (const b of screen.buttons) expect(button(html, `store-refused-button-${b.key}`)?.label).toBe(escaped(b.label));
        expect(html.match(/<button/g)).toHaveLength(screen.buttons.length);
        expect(html).not.toContain("disabled");
        expect(html.includes('data-testid="store-refused-hint"')).toBe(screen.hint !== null);
        expectNoWalletFallthrough(html);
        expectStyledClasses(html);
      });
    });

  it("renders the same markup in light and dark: the theme is the stylesheet's alone", () => {
    for (const [, refusal] of FIXTURES) sameInBothThemes(() => renderRefusal(refusal));
  });

  it("covers today's screens: cantOpen with and without a backup", () => {
    expect(FIXTURES.map(([, r]) => r.screen.textKey)).toEqual(["storeUpgrade.cantOpen.text", "storeUpgrade.cantOpen.textWithoutBackup"]);
  });

  it("has an action for every button key the engine sends", () => {
    for (const [, { screen }] of FIXTURES) for (const b of screen.buttons) expect(buttonAction(b.key), b.key).not.toBeNull();
  });

  it("carries the engine's English for each key, so the catalog's text is the DTO's", () => {
    for (const [, { screen, folder }] of FIXTURES)
      for (const [key, english] of texts(screen)) {
        expect(ENGLISH.has(key), key).toBe(true);
        expect((t as unknown as (k: string, v: object) => string)(key, { folder }), key).toBe(english);
      }
  });

  it("keeps the fixture's copy dw-store-upgrade's, verbatim", () => {
    const rust = readFileSync(resolve(import.meta.dirname, "../../../../rust/crates/dw-store-upgrade/src/copy.rs"), "utf8");
    const constant = (name: string) => new RegExp(`const ${name}: &str = "((?:[^"\\\\]|\\\\.)*)";`).exec(rust)?.[1];
    expect(constant("CANT_OPEN_TITLE")).toBe(STORE_COPY.cantOpenTitle);
    expect(constant("CANT_OPEN_TEXT")).toBe(STORE_COPY.cantOpenText);
    expect(constant("CANT_OPEN_TEXT_WITHOUT_BACKUP")).toBe(STORE_COPY.cantOpenTextWithoutBackup);
    expect(rust).toContain('button("storeUpgrade.cantOpen.quit", "Quit")');
  });

  it("shows the engine's English for a key the catalog lacks, never other copy, and disables a button it can't run", () => {
    const refusal: StoreRefusedData = {
      kind: "storeRefused",
      cause: "SomeNewVariant",
      folder: FOLDER,
      screen: {
        titleKey: "storeUpgrade.later.title",
        title: "A later title",
        textKey: "storeUpgrade.later.text",
        text: `A later text about ${FOLDER}.`,
        buttons: [
          { key: "storeUpgrade.later.report", label: "A later button", destructive: false, defaultFocus: true },
          { key: "storeUpgrade.later.quit", label: "Leave", destructive: true, defaultFocus: false },
        ],
        hint: { key: "settings.about.links.issuesHint", text: "A later hint." },
      },
    };
    const html = renderRefusal(refusal);
    expect(html).toContain(">A later title</h1>");
    expect(html).toContain(`data-testid="store-refused-text">A later text about ${FOLDER}.</p>`);
    const later = button(html, "store-refused-button-storeUpgrade.later.report");
    expect(later?.label).toBe("A later button");
    expect(later?.tag).toContain("dash-button-filledBlue");
    expect(later?.tag).toContain('disabled=""');
    const leave = button(html, "store-refused-button-storeUpgrade.later.quit");
    expect(leave?.label).toBe("Leave");
    expect(leave?.tag).toContain("dash-button-plainRed");
    expect(leave?.tag).not.toContain("disabled");
    expect(html).toContain('data-testid="store-refused-hint">A later hint.</p>');
    expect(ENGLISH.has("settings.about.links.issuesHint")).toBe(true);
    expect(html).not.toContain(escaped(en.settings.about.links.issuesHint));
    expect(buttonAction("storeUpgrade.later.report")).toBeNull();
  });

  it("follows the language for a key the catalog has, and keeps the engine's English for one it lacks", () => {
    ui.set({ prefs: { ...DEFAULT_PREFS, language: "en-XA" } });
    const refusal = STORE_SCREENS.cantOpen(FOLDER);
    const html = renderRefusal({ ...refusal, screen: { ...refusal.screen, hint: { key: "storeUpgrade.later.hint", text: "A later hint." } } });
    expect(html).not.toContain(escaped(refusal.screen.title));
    expect(html).toContain(FOLDER);
    expect(html).toContain('data-testid="store-refused-hint">A later hint.</p>');
  });

  it("never depends on the cause", () => {
    const refusal = STORE_SCREENS.cantOpen(FOLDER);
    expect(renderRefusal({ ...refusal, cause: "P9aBroken" })).toBe(renderRefusal({ ...refusal, cause: "AnythingElse" }));
  });

  it("shows a quit that failed", () => {
    expect(renderRefusal(STORE_SCREENS.cantOpen(FOLDER), "This is not available in this build.")).toContain('data-testid="store-refused-problem">This is not available in this build.</p>');
  });
});

describe("the StorageInUse screen", () => {
  for (const theme of THEMES) {
    it(`in ${theme}, offers Try again and Restart when trying again can help`, () => {
      ui.set({ theme });
      const html = renderInUse(true);
      expect(html).toContain('role="alertdialog"');
      expect(html).toContain(`>${escaped(S.title)}</h1>`);
      expect(html).toContain(`data-testid="storage-in-use-text">${escaped(S.text)}</p>`);
      expect(button(html, "storage-in-use-retry")?.label).toBe(S.tryAgain);
      expect(button(html, "storage-in-use-restart")?.label).toBe(S.restart);
      expectNoWalletFallthrough(html);
      expectStyledClasses(html);
    });

    it(`in ${theme}, offers only Restart, and no Try again in its text, when only a restart frees the data`, () => {
      ui.set({ theme });
      const html = renderInUse(false);
      expect(html).toContain(`>${escaped(S.title)}</h1>`);
      expect(html).toContain(`data-testid="storage-in-use-text">${escaped(S.textRestartOnly)}</p>`);
      expect(html).not.toContain("Try again in a moment, or");
      expect(html).not.toContain(S.tryAgain);
      expect(button(html, "storage-in-use-retry")).toBeNull();
      expect(button(html, "storage-in-use-restart")?.label).toBe(S.restart);
      expectNoWalletFallthrough(html);
      expectStyledClasses(html);
    });
  }

  it("keeps MG's text: the restart-only text is the full text without its Try again, the same in both themes", () => {
    expect(S.textRestartOnly).toBe(S.text.replace("Try again in a moment, or restart", "Restart"));
    for (const retryable of [true, false]) sameInBothThemes(() => renderInUse(retryable));
  });

  it("holds Try again while its open runs", () => {
    const idle = button(renderInUse(true), "storage-in-use-retry")?.tag;
    expect(idle).not.toContain("disabled");
    expect(idle).toContain('aria-busy="false"');
    const busy = button(renderInUse(true, null, true), "storage-in-use-retry")?.tag;
    expect(busy).toContain('disabled=""');
    expect(busy).toContain('aria-busy="true"');
    expect(button(renderInUse(true, null, true), "storage-in-use-restart")?.tag).not.toContain("disabled");
  });

  it("shows a restart that failed", () => {
    expect(renderInUse(false, "This is not available in this build.")).toContain('data-testid="storage-in-use-problem">This is not available in this build.</p>');
  });
});

describe("the actions", () => {
  it("quits and restarts through dw-app, and shows what failed", async () => {
    const calls: string[] = [];
    const actions = new OpenFailureActions({
      quitApp: async () => {
        calls.push("quit");
        return null;
      },
      restartApp: async () => {
        calls.push("restart");
        throw new Error("This is not available in this build.");
      },
    });
    await actions.quit();
    expect(actions.problem).toBeNull();
    await actions.restart();
    expect(calls).toEqual(["quit", "restart"]);
    expect(actions.problem).toBe("This is not available in this build.");
    await actions.quit();
    expect(actions.problem).toBeNull();
  });
});

/** The whole app with the network's open failed with `error`, as its first paint shows it. */
function renderAppFailed(error: unknown): string {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false } } });
  const query = client.getQueryCache().build(client, { queryKey: keys.network });
  query.setState({ status: "error", error: error as Error, fetchStatus: "idle", errorUpdatedAt: 1, errorUpdateCount: 1, fetchFailureCount: 1 });
  const backend = { kind: "fixture" } as unknown as Backend;
  return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(BackendContext.Provider, { value: backend }, createElement(App))));
}

describe("the app while the open is refused or held", () => {
  it.each(FIXTURES)("shows the %s refusal screen, not the shell or onboarding", (_, refusal) => {
    const html = renderAppFailed(storeRefusedError(refusal));
    expect(html).toContain('data-testid="store-refused"');
    expect(html).not.toContain('data-testid="splash"');
    expectNoWalletFallthrough(html);
  });

  it.each([true, false])("shows the StorageInUse screen (retryable: %s), not the shell or onboarding", (retryable) => {
    const html = renderAppFailed(storageInUseError(retryable));
    expect(html).toContain('data-testid="storage-in-use"');
    expect(html.includes('data-testid="storage-in-use-retry"')).toBe(retryable);
    expect(html).not.toContain('data-testid="splash"');
    expectNoWalletFallthrough(html);
  });

  it("keeps the splash's own error for any other failure", () => {
    const html = renderAppFailed({ code: "storage", message: "disk full", data: null });
    expect(html).toContain('data-testid="splash"');
    expect(html).toContain("Dash Wallet could not open the wallet: disk full");
  });
});

describe("the fixture", () => {
  type Ctx = Parameters<(typeof PENDING_HANDLERS)["restart_app"]>[1];
  const call = (handler: unknown) => handler as (args: object, ctx: Ctx) => unknown;
  const [open, restart, quit] = [call(HANDLERS.open_network), call(PENDING_HANDLERS.restart_app), call(PENDING_HANDLERS.quit_app)];
  const ctxOf = (world: ReturnType<typeof buildWorld>, relaunch?: () => void): Ctx => ({ world, emit: noop, relaunch });

  it("refuses every open, a restart's included, with the screen it holds", () => {
    const world = buildWorld("funded", null);
    world.openRefusal = STORE_SCREENS.cantOpenWithoutBackup(FOLDER);
    const ctx = ctxOf(world);
    expect(() => open({}, ctx)).toThrow(expect.objectContaining({ code: "storage_refused", data: world.openRefusal }));
    quit({}, ctx);
    restart({}, ctx);
    expect(() => open({}, ctx)).toThrow(expect.objectContaining({ code: "storage_refused" }));
    expect(world.appExits).toEqual(["quit", "restart"]);
  });

  it("frees a retryable held store on the next open, and a restart-only one only on restart", () => {
    const world = buildWorld("funded", null);
    world.storageHeld = { retryable: true };
    const ctx = ctxOf(world);
    expect(() => open({}, ctx)).toThrow(expect.objectContaining({ code: "storage_in_use", data: { kind: "storageInUse", retryable: true } }));
    expect(open({}, ctx)).toMatchObject({ network: world.launch.network });

    world.storageHeld = { retryable: false };
    for (let i = 0; i < 3; i++) expect(() => open({}, ctx)).toThrow(expect.objectContaining({ code: "storage_in_use", data: { kind: "storageInUse", retryable: false } }));
    let relaunched = 0;
    restart({}, ctxOf(world, () => relaunched++));
    expect(relaunched).toBe(1);
    expect(open({}, ctx)).toMatchObject({ network: world.launch.network });
  });
});
