// The screens shown instead of the wallet when its data can't be opened: the engine's store refusal
// screens as `screen_for` gives them today (the foreign lineage's included), a restore that stopped the
// open in each state and cause, and StorageInUse in both variants, in light and dark. None ever falls
// through to onboarding, an empty wallet list or a zero balance. Restore earlier data goes only through
// the confirm, and the restored banner shows while RESTORED is set.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../api/backend";
import { BackendContext, keys } from "../api/queries";
import { RESTORE_CAUSES, type RestoreState, type StoreRefusedData, type StoreRestoreData, type StoreScreen } from "../api/storeOpen";
import { App } from "../app/App";
import { DEFAULT_PREFS, ui } from "../app/ui";
import { RESTORE_FAILURE_NAMES, restoreFailure, STORE_COPY, STORE_SCREEN_NAMES, STORE_SCREENS, storageInUseError, storeRefusedError, storeRestoreError } from "../fixture/errors";
import { HANDLERS } from "../fixture/handlers";
import { PENDING_HANDLERS } from "../fixture/pending";
import { buildWorld } from "../fixture/scenarios";
import { t } from "../i18n";
import { flatten } from "../i18n/core";
import { en } from "../i18n/en";
import { buttonAction, OpenFailureActions, RESTORE_BUTTONS, restoreButtonKey, RestoreConfirmView, screenText, StorageInUseView, StoreRefusalView, StoreRestoreView } from "./StoreRefusal";
import { RestoredBanner } from "./RestoredBanner";

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
const renderRefusal = (refusal: StoreRefusedData, problem: string | null = null, restoreAvailable = true) =>
  renderToStaticMarkup(createElement(StoreRefusalView, { refusal, problem, restoreAvailable, onAction: noop }));
const renderRestore = (restore: StoreRestoreData, problem: string | null = null) => renderToStaticMarkup(createElement(StoreRestoreView, { restore, problem, onAction: noop }));
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
  // The header's empty side cells, as the existing notices and sheets have them, take the grid's columns alone.
  const classes = new Set([...html.matchAll(/class="([^"]+)"/g)].flatMap((m) => m[1].split(/\s+/)).filter((c) => c && c !== "onboarding-header-side" && c !== "sheet-header-side"));
  for (const c of classes) expect(STYLESHEET, c).toMatch(new RegExp(`\\.${c.replace(/-/g, "\\-")}[\\s,{:.>]`));
}

/** Every catalog key and English text a screen carries. */
const texts = (s: StoreScreen): [string, string][] => [[s.titleKey, s.title], ...s.lines.map((l): [string, string] => [l.key, l.text]), ...s.buttons.map((b): [string, string] => [b.key, b.label]), ...(s.hint ? [[s.hint.key, s.hint.text] as [string, string]] : [])];

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

  it("covers today's screens: the refusal family with the way out to the version before, to support and to Restore earlier data", () => {
    expect(FIXTURES.map(([, r]) => r.screen.textKey)).toEqual(["storeUpgrade.refused.wayOut.previous", "storeUpgrade.refused.wayOut.support", "storeUpgrade.refused.wayOut.restore"]);
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
    const pattern = (key: string) => new RegExp(`(?:line|button)\\(\\s*"${key.replace(/\./g, "\\.")}",\\s*"((?:[^"\\\\]|\\\\.)*)",?\\s*\\)`, "g");
    for (const [key, english] of Object.entries(STORE_COPY)) {
      const found = [...rust.matchAll(pattern(key))].map((m) => m[1]);
      expect(found, key).toEqual([english]);
    }
    expect(rust).toContain('button("settings.about.links.issues", "Report a problem")');
    expect(rust).not.toContain("storeUpgrade.cantOpen.");
  });

  it("has every refusal line and button copy.rs names in its catalog, verbatim", () => {
    const rust = readFileSync(resolve(import.meta.dirname, "../../../../rust/crates/dw-store-upgrade/src/copy.rs"), "utf8");
    const named = new Map([...rust.matchAll(/(?:line|button)\(\s*"(storeUpgrade\.refused\.[\w.]+)",\s*"((?:[^"\\]|\\.)*)",?\s*\)/g)].map((m) => [m[1], m[2]]));
    const catalog = [...ENGLISH.keys()].filter((k) => k.startsWith("storeUpgrade.refused."));
    expect(named.size).toBeGreaterThan(20);
    expect(catalog.sort()).toEqual([...named.keys()].sort());
    for (const [key, english] of named) expect(ENGLISH.get(key), key).toBe(english);
  });

  it("marks the keys copy.rs lists as pending MG as pending in its catalog", () => {
    const rust = readFileSync(resolve(import.meta.dirname, "../../../../rust/crates/dw-store-upgrade/src/copy.rs"), "utf8");
    const catalog = readFileSync(resolve(import.meta.dirname, "../i18n/catalog/storeUpgrade.ts"), "utf8");
    const pending = [.../pub const PENDING_MG: &\[&str\] = &\[([^\]]*)\]/.exec(rust)![1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
    const marked = [...catalog.matchAll(/\/\/ Pending MG: [^\n]*\n\s*(\w+):/g)].map((m) => m[1]);
    expect(pending.map((k) => k.split(".").pop()).sort()).toEqual(marked.sort());
    for (const key of pending) expect(ENGLISH.has(key), key).toBe(true);
  });

  it("has the engine's restore copy in its catalog verbatim, the keys copy.rs names included", () => {
    const rust = readFileSync(resolve(import.meta.dirname, "../../../../rust/crates/dw-store-upgrade/src/copy.rs"), "utf8");
    const constant = (name: string) => new RegExp(`const ${name}: &str = "((?:[^"\\\\]|\\\\.)*)";`).exec(rust)?.[1]?.replace(/\\n/g, "\n");
    const R = en.storeUpgrade.restore;
    expect([R.confirm.title, R.confirm.text, R.refused.title, R.refused.text, R.incomplete.title, R.incomplete.text, R.noSpace.title, R.noSpace.text]).toEqual(
      ["CONFIRM_TITLE", "CONFIRM_TEXT", "RESTORE_REFUSED_TITLE", "RESTORE_REFUSED_TEXT", "INCOMPLETE_TITLE", "INCOMPLETE_TEXT", "NO_SPACE_TITLE", "NO_SPACE_TEXT"].map(constant),
    );
    for (const key of ["title", "text", "keep", "keepMeaning", "quit"] as const) expect(R.foreign[key]).toBe(STORE_COPY[`storeUpgrade.restore.foreign.${key}`]);
    expect(rust).not.toContain("keepCurrent");
    expect([...ENGLISH.keys()].filter((k) => k.includes("keepCurrent"))).toEqual([]);
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
        text: `A later intro: a later text about ${FOLDER}.`,
        lines: [
          { key: "storeUpgrade.later.intro", text: "A later intro" },
          { key: "storeUpgrade.later.text", text: `a later text about ${FOLDER}.` },
        ],
        buttons: [
          { key: "storeUpgrade.later.report", label: "A later button", destructive: false, defaultFocus: true },
          { key: "storeUpgrade.later.quit", label: "Leave", destructive: true, defaultFocus: false },
        ],
        hint: { key: "settings.about.links.issuesHint", text: "A later hint." },
      },
    };
    const html = renderRefusal(refusal);
    expect(html).toContain(">A later title</h1>");
    expect(html).toContain(`data-testid="store-refused-text">A later intro: a later text about ${FOLDER}.</p>`);
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
    const refusal = STORE_SCREENS.refusedPrevious(FOLDER);
    const html = renderRefusal({
      ...refusal,
      screen: { ...refusal.screen, titleKey: "storeUpgrade.restore.foreign.title", title: en.storeUpgrade.restore.foreign.title, hint: { key: "storeUpgrade.later.hint", text: "A later hint." } },
    });
    expect(html).not.toContain(escaped(refusal.screen.title));
    for (const line of refusal.screen.lines) expect(html).not.toContain(escaped(line.text));
    expect(html).toContain(FOLDER);
    expect(html).toContain('data-testid="store-refused-hint">A later hint.</p>');
  });

  it("joins its lines as the engine does, each in the language", () => {
    const screen: StoreScreen = {
      ...STORE_SCREENS.refusedSupport(FOLDER).screen,
      text: "An intro: it was written by a newer version of Dash Wallet. Choose Report a problem.",
      lines: [
        { key: "storeUpgrade.later.intro", text: "An intro" },
        { key: "storeUpgrade.refused.detail.tooNew", text: "it was written by a newer version of Dash Wallet." },
        { key: "storeUpgrade.refused.wayOut.support", text: "Choose Report a problem." },
      ],
    };
    expect(screenText(t, screen, FOLDER, false)).toBe("An intro: it was written by a newer version of Dash Wallet. Choose Report a problem.");
    expect(screenText(t, screen, FOLDER, true)).toBe("\u2066An intro\u2069: it was written by a newer version of Dash Wallet. Choose Report a problem.");
  });

  it("shows the engine's text unchanged for no lines, one line, or lines that don't join to it", () => {
    ui.set({ prefs: { ...DEFAULT_PREFS, language: "en-XA" } });
    const whole = STORE_SCREENS.refusedPrevious(FOLDER);
    const screens: StoreScreen[] = [
      { ...whole.screen, lines: [] },
      { ...whole.screen, text: "A later text.", lines: [{ key: "storeUpgrade.refused.intro", text: "A later text." }] },
      { ...whole.screen, lines: whole.screen.lines.slice(1) },
    ];
    for (const screen of screens) {
      expect(screenText(t, screen, FOLDER, false)).toBe(screen.text);
      expect(renderRefusal({ ...whole, screen })).toContain(`data-testid="store-refused-text">${escaped(screen.text)}</p>`);
    }
  });

  for (const language of ["en-XA", "ar-XB"] as const)
    it(`translates every line the catalog has under ${language}, and keeps the engine's English for the one it lacks`, () => {
      ui.set({ prefs: { ...DEFAULT_PREFS, language } });
      const whole = STORE_SCREENS.refusedPrevious(FOLDER);
      const lines = whole.screen.lines.map((l, i) => (i === 1 ? { ...l, key: "storeUpgrade.later.detail" } : l));
      const html = renderRefusal({ ...whole, screen: { ...whole.screen, lines } });
      const kept = escaped(lines[1].text);
      expect(html).toContain(language === "ar-XB" ? `\u2066${kept}\u2069` : kept);
      if (language === "en-XA") expect(html).not.toContain("\u2066");
      const translated = [lines[0], lines[2], lines[3]];
      if (language === "en-XA") for (const line of translated) expect(html).not.toContain(escaped(line.text));
      else {
        // ar-XB draws the catalog's English right to left (U+202E … U+202C); only the kept line is isolated left to right.
        expect(html).toContain(`\u202e${escaped(lines[3].text)}\u202c`);
        for (const line of translated) expect(html).not.toContain(`\u2066${escaped(line.text)}`);
        expect(html.split("\u2066")).toHaveLength(2);
      }
    });

  it("never depends on the cause", () => {
    const refusal = STORE_SCREENS.refusedPrevious(FOLDER);
    expect(renderRefusal({ ...refusal, cause: "P9aBroken" })).toBe(renderRefusal({ ...refusal, cause: "AnythingElse" }));
  });

  it("shows a quit that failed", () => {
    expect(renderRefusal(STORE_SCREENS.refusedPrevious(FOLDER), "This is not available in this build.")).toContain('data-testid="store-refused-problem">This is not available in this build.</p>');
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
      ...api([]),
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
    world.openRefusal = STORE_SCREENS.refusedSupport(FOLDER);
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

/** Every action's API, each recording its call by name and answering `null`; `fail` names those that throw. */
function api(calls: string[], fail: Record<string, unknown> = {}) {
  const call = (name: string) => async () => {
    calls.push(name);
    if (name in fail) throw fail[name];
    return null;
  };
  return {
    quitApp: call("quit"),
    restartApp: call("restart"),
    keepCurrentFiles: call("keep"),
    cancelRestore: call("cancelRestore"),
    requestRestore: call("requestRestore"),
    openProjectLink: async (link: string) => {
      calls.push(`link:${link}`);
      if ("link" in fail) throw fail.link;
      return null;
    },
  };
}

const RESTORE_STATES = Object.keys(RESTORE_CAUSES) as RestoreState[];
const RESTORE_FIXTURES = RESTORE_FAILURE_NAMES.map((name) => [name, restoreFailure(name, FOLDER)] as const);
const R = en.storeUpgrade.restore;

describe("the restore screens", () => {
  for (const theme of THEMES)
    describe(`in ${theme}`, () => {
      it.each(RESTORE_FIXTURES)("shows %s by its state, from the catalog", (_, restore) => {
        ui.set({ theme });
        const copy = R[restore.state];
        const html = renderRestore(restore);
        expect(html).toContain('role="alertdialog"');
        expect(html).toContain(`>${escaped(copy.title)}</h1>`);
        expect(html).toContain(`data-testid="store-restore-text">${escaped(copy.text.replaceAll("{folder}", FOLDER))}</p>`);
        expect(html).not.toContain("{folder}");
        const keys = RESTORE_BUTTONS[restore.state];
        expect(html.match(/<button/g)).toHaveLength(keys.length);
        const foreign = restore.state === "foreign";
        keys.forEach((k, i) => {
          const key = restoreButtonKey(restore.state, k);
          const b = button(html, `store-restore-button-${key}`);
          expect(b?.label, k).toBe(escaped(ENGLISH.get(key) as string));
          expect(b?.tag.includes("dash-button-filledBlue"), k).toBe(!foreign && i === 0);
        });
        expect(html).not.toContain("disabled");
        expect(keys.at(-1)).toBe("quit");
        expect(html).not.toContain(escaped(en.app.tryAgain));
        const reports = keys.includes("report");
        expect(html.includes(`>${escaped(en.settings.about.links.issuesHint)}</p>`)).toBe(reports);
        expectNoWalletFallthrough(html);
        expectStyledClasses(html);
      });
    });

  it("renders the same markup in light and dark, and the same for every cause of a state", () => {
    for (const [, restore] of RESTORE_FIXTURES) sameInBothThemes(() => renderRestore(restore));
    for (const state of RESTORE_STATES) {
      const [first, ...rest] = RESTORE_CAUSES[state].map((cause) => renderRestore({ ...restoreFailure(`${state}:${RESTORE_CAUSES[state][0]}`, FOLDER), cause }));
      for (const html of rest) expect(html).toBe(first);
    }
  });

  it("covers every state and each cause the engine names", () => {
    expect(RESTORE_STATES).toEqual(["refused", "noSpace", "notStarted", "incomplete", "foreign"]);
    expect(RESTORE_FAILURE_NAMES).toHaveLength(3 + 1 + 4 + 4 + 2);
    expect([...new Set(RESTORE_FIXTURES.map(([, r]) => r.state))]).toEqual(RESTORE_STATES);
  });

  it("gives every button an action: OK opens the wallet data, Restart restarts, Cancel restore drops the request", () => {
    expect(RESTORE_STATES.flatMap((state) => RESTORE_BUTTONS[state].map((k) => [`${state}.${k}`, buttonAction(restoreButtonKey(state, k))]))).toEqual([
      ["refused.ok", "reopen"],
      ["refused.quit", "quit"],
      ["noSpace.ok", "reopen"],
      ["noSpace.quit", "quit"],
      ["notStarted.restart", "restart"],
      ["notStarted.cancel", "cancelRestore"],
      ["notStarted.report", "report"],
      ["notStarted.quit", "quit"],
      ["incomplete.restart", "restart"],
      ["incomplete.report", "report"],
      ["incomplete.quit", "quit"],
      ["foreign.keep", "keep"],
      ["foreign.report", "report"],
      ["foreign.quit", "quit"],
    ]);
  });

  it("labels Report a problem as Settings does, and never offers to clean up", () => {
    expect(R.notStarted.report).toBe(en.settings.about.links.issues);
    expect(R.incomplete.report).toBe(en.settings.about.links.issues);
    for (const text of Object.values(en.storeUpgrade.restore).flatMap((v) => (typeof v === "string" ? [v] : Object.values(v)))) expect(text).not.toMatch(/held on the network/);
  });

  it("follows the language", () => {
    ui.set({ prefs: { ...DEFAULT_PREFS, language: "en-XA" } });
    const html = renderRestore(restoreFailure("incomplete:diskFull", FOLDER));
    expect(html).not.toContain(escaped(R.incomplete.title));
    expect(html).toContain(FOLDER);
  });

  it("holds every button but Quit while an action or the open again runs", () => {
    const html = renderToStaticMarkup(createElement(StoreRestoreView, { restore: restoreFailure("notStarted:other", FOLDER), problem: null, busy: true, onAction: noop }));
    for (const k of RESTORE_BUTTONS.notStarted) expect(button(html, `store-restore-button-storeUpgrade.restore.notStarted.${k}`)?.tag.includes('disabled=""'), k).toBe(k !== "quit");
    const foreign = renderToStaticMarkup(createElement(StoreRestoreView, { restore: restoreFailure("foreign:otherLineage", FOLDER), problem: null, busy: true, onAction: noop }));
    expect(button(foreign, "store-restore-button-storeUpgrade.restore.foreign.keep")?.tag).toContain('disabled=""');
    expect(button(foreign, "store-restore-button-storeUpgrade.restore.foreign.quit")?.tag).not.toContain("disabled");
  });

  it("adds no second issues warning to a screen whose hint is the warning", () => {
    expect(renderRefusal(STORE_SCREENS.refusedRestore(FOLDER))).not.toContain("store-refused-issues-hint");
    expect(renderRefusal(STORE_SCREENS.refusedPrevious(FOLDER))).not.toContain("store-refused-issues-hint");
  });

  it("shows an action that failed", () => {
    expect(renderRestore(restoreFailure("notStarted:other", FOLDER), "This is not available in this build.")).toContain('data-testid="store-restore-problem">This is not available in this build.</p>');
  });
});

describe("the foreign lineage's screen", () => {
  const foreign = restoreFailure("foreign:otherLineage", FOLDER);

  for (const theme of THEMES)
    it(`in ${theme}, shows the engine's copy with Keep the current files, not focused, and what Keep means under it`, () => {
      ui.set({ theme });
      const html = renderRestore(foreign);
      const F = en.storeUpgrade.restore.foreign;
      expect(html).toContain(`>${escaped(F.title)}</h1>`);
      expect(html).toContain(`data-testid="store-restore-text">${escaped(F.text.replaceAll("{folder}", FOLDER))}</p>`);
      const keep = button(html, "store-restore-button-storeUpgrade.restore.foreign.keep");
      expect(keep?.label).toBe(F.keep);
      expect(keep?.tag).not.toContain("dash-button-filledBlue");
      expect(html).not.toContain("autofocus");
      expect(button(html, "store-restore-button-settings.about.links.issues")?.label).toBe(en.settings.about.links.issues);
      expect(button(html, "store-restore-button-storeUpgrade.restore.foreign.quit")?.label).toBe("Quit");
      expect(html).toContain(`data-testid="store-restore-hint">${escaped(F.keepMeaning.replaceAll("{folder}", FOLDER))}</p>`);
      expect(html).toContain(`data-testid="store-restore-issues-hint">${escaped(en.settings.about.links.issuesHint)}</p>`);
      expectNoWalletFallthrough(html);
      expectStyledClasses(html);
    });

  it("wires Keep to keep_current_files, Report a problem to the issues page and Quit to quit, in copy.rs's order and keys", () => {
    const keys = RESTORE_BUTTONS.foreign.map((k) => restoreButtonKey("foreign", k));
    expect(keys).toEqual(["storeUpgrade.restore.foreign.keep", "settings.about.links.issues", "storeUpgrade.restore.foreign.quit"]);
    expect(keys.map(buttonAction)).toEqual(["keep", "report", "quit"]);
    const rust = readFileSync(resolve(import.meta.dirname, "../../../../rust/crates/dw-store-upgrade/src/copy.rs"), "utf8");
    const screen = rust.slice(rust.indexOf("pub mod restore_foreign"));
    expect(screen).toContain('buttons: vec![\n                b(KEEP),\n                button("settings.about.links.issues", "Report a problem"),\n                b(QUIT),\n            ]');
    expect(en.storeUpgrade.restore.foreign.quit).toBe("Quit");
  });
});

describe("Restore earlier data", () => {
  const offer = STORE_SCREENS.refusedRestore(FOLDER);
  const restoreButton = "store-refused-button-storeUpgrade.refused.restore";

  it("shows only while a verified snapshot is there", () => {
    expect(button(renderRefusal(offer, null, true), restoreButton)?.label).toBe("Restore earlier data");
    const without = renderRefusal(offer, null, false);
    expect(button(without, restoreButton)).toBeNull();
    expect(without.match(/<button/g)).toHaveLength(offer.screen.buttons.length - 1);
    expect(buttonAction("storeUpgrade.refused.restore")).toBe("restore");
  });

  const renderConfirm = (busy = false, problem: string | null = null) => renderToStaticMarkup(createElement(RestoreConfirmView, { folder: FOLDER, busy, problem, onConfirm: noop, onClose: noop }));

  for (const theme of THEMES)
    it(`in ${theme}, asks first: MG's confirm, Cancel focused, Restart and restore destructive`, () => {
      ui.set({ theme });
      const html = renderConfirm();
      const C = R.confirm;
      expect(html).toContain('role="dialog"');
      expect(html).toContain(`>${escaped(C.title)}</h2>`);
      const paragraphs = [...html.matchAll(/data-testid="restore-confirm-text">([^<]*)<\/p>/g)].map((m) => m[1]);
      expect(paragraphs).toEqual(C.text.replaceAll("{folder}", FOLDER).split("\n\n").map(escaped));
      expect(paragraphs[0]).toContain(`upgrade-backups folder inside ${FOLDER}`);
      const cancel = button(html, "restore-confirm-cancel");
      expect(cancel?.label).toBe(C.cancel);
      expect(cancel?.tag).toContain("data-autofocus");
      const confirm = button(html, "restore-confirm-confirm");
      expect(confirm?.label).toBe(C.confirm);
      expect(confirm?.tag).toContain("dash-button-plainRed");
      expect(confirm?.tag).not.toContain("data-autofocus");
      expectNoWalletFallthrough(html);
      expectStyledClasses(html);
    });

  it("renders the confirm the same in light and dark, holds both buttons while it runs, and shows a failure", () => {
    sameInBothThemes(() => renderConfirm());
    const busy = renderConfirm(true);
    expect(button(busy, "restore-confirm-confirm")?.tag).toContain('disabled=""');
    expect(button(busy, "restore-confirm-cancel")?.tag).toContain('disabled=""');
    expect(renderConfirm(false, "Something went wrong.")).toContain('data-testid="restore-confirm-problem">Something went wrong.</p>');
  });
});

describe("the restore actions", () => {
  it("confirm: request_restore, then restart_app", async () => {
    const calls: string[] = [];
    const actions = new OpenFailureActions(api(calls));
    await actions.restore();
    expect(calls).toEqual(["requestRestore", "restart"]);
    expect(actions.problem).toBeNull();
    expect(actions.running).toBe(false);
  });

  it("never restarts when the request was refused, and says so", async () => {
    const calls: string[] = [];
    const actions = new OpenFailureActions(api(calls, { requestRestore: storeRestoreError(restoreFailure("refused:missingBackup", FOLDER)) }));
    await actions.restore();
    expect(calls).toEqual(["requestRestore"]);
    expect(actions.problem).toBe(R.refused.text);
    expect(actions.requested).toBe(false);
    await actions.dismissRestore();
    expect(calls).toEqual(["requestRestore"]);
  });

  it("after a failed restart, tries only the restart again, and drops the request when the confirm closes", async () => {
    const calls: string[] = [];
    const fail: Record<string, unknown> = { restart: new Error("The restart failed.") };
    const actions = new OpenFailureActions(api(calls, fail));
    await actions.restore();
    expect(actions.problem).toBe("The restart failed.");
    expect(actions.requested).toBe(true);
    await actions.restore();
    expect(calls).toEqual(["requestRestore", "restart", "restart"]);
    await actions.dismissRestore();
    expect(calls.at(-1)).toBe("cancelRestore");
    expect(actions.requested).toBe(false);
  });

  it("clears the last failure as the confirm opens", async () => {
    const actions = new OpenFailureActions(api([], { link: new Error("no browser") }));
    await actions.report();
    expect(actions.problem).toBe("no browser");
    actions.clear();
    expect([actions.problem, actions.failedLink]).toEqual([null, null]);
  });

  it("Keep: keep_current_files, then the open again", async () => {
    const calls: string[] = [];
    const actions = new OpenFailureActions(api(calls));
    await actions.keep(() => calls.push("reopen"));
    expect(calls).toEqual(["keep", "reopen"]);
  });

  it("does not open again when Keep failed", async () => {
    const calls: string[] = [];
    const actions = new OpenFailureActions(api(calls, { keep: new Error("This is not available in this build.") }));
    await actions.keep(() => calls.push("reopen"));
    expect(calls).toEqual(["keep"]);
    expect(actions.problem).toBe("This is not available in this build.");
  });

  it("Cancel restore: cancel_restore, then the open again", async () => {
    const calls: string[] = [];
    await new OpenFailureActions(api(calls)).cancelRestore(() => calls.push("reopen"));
    expect(calls).toEqual(["cancelRestore", "reopen"]);
  });

  it("Report a problem opens the issues page, and names it to copy when no browser opens", async () => {
    const calls: string[] = [];
    const ok = new OpenFailureActions(api(calls));
    await ok.report();
    expect(calls).toEqual(["link:issues"]);
    const failing = new OpenFailureActions(api([], { link: { code: "app.browser_open", message: "no browser", data: { kind: "link", url: "https://example.invalid/issues" } } }));
    await failing.report();
    expect(failing.failedLink).toBe("https://example.invalid/issues");
    expect(failing.problem).toBeNull();
  });
});

describe("the app while a restore stopped the open", () => {
  it.each(RESTORE_FIXTURES)("shows the %s restore screen, not the shell or onboarding", (_, restore) => {
    const html = renderAppFailed(storeRestoreError(restore));
    expect(html).toContain('data-testid="store-restore"');
    expect(html).not.toContain('data-testid="store-refused"');
    expect(html).not.toContain('data-testid="splash"');
    expectNoWalletFallthrough(html);
  });

  it("shows a malformed storage_restore as the splash's message, with no retry", () => {
    const html = renderAppFailed({ code: "storage_restore", message: "restore stopped", data: { kind: "restore", state: "later", cause: "x", neededBytes: null, folder: FOLDER } });
    expect(html).toContain('data-testid="splash"');
    expect(html).toContain("Dash Wallet could not open the wallet: restore stopped");
    expect(html).not.toContain(escaped(en.app.tryAgain));
  });
});

describe("the restored banner", () => {
  function renderBanner(restored: boolean | undefined) {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
    if (restored !== undefined) client.setQueryData(keys.restored, restored);
    const backend = { kind: "fixture", activeNetwork: () => new Promise(noop) } as unknown as Backend;
    return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(BackendContext.Provider, { value: backend }, createElement(RestoredBanner))));
  }

  for (const theme of THEMES)
    it(`in ${theme}, shows while restored is true: anything that moves funds is paused`, () => {
      ui.set({ theme });
      const html = renderBanner(true);
      expect(html).toContain('data-testid="restored-banner"');
      expect(html).toContain(`data-testid="restored-banner-paused">${escaped(R.banner)}</p>`);
      expect(html).toContain(`data-testid="restored-banner-text">${escaped(en.storeUpgrade.restored.text)}</p>`);
      expectStyledClasses(html);
    });

  it("is gone when restored is false or not known yet, and the same in light and dark", () => {
    expect(renderBanner(false)).toBe("");
    expect(renderBanner(undefined)).toBe("");
    sameInBothThemes(() => renderBanner(true));
  });
});

describe("the fixture's restore", () => {
  type Ctx = Parameters<(typeof PENDING_HANDLERS)["restart_app"]>[1];
  const call = (handler: unknown) => handler as (args: object, ctx: Ctx) => unknown;
  const [open, active, restart] = [call(HANDLERS.open_network), call(HANDLERS.active_network), call(PENDING_HANDLERS.restart_app)];
  const [available, request, keep, cancel] = ["restore_available", "request_restore", "keep_current_files", "cancel_restore"].map((c) => call(PENDING_HANDLERS[c]));
  const ctxOf = (world: ReturnType<typeof buildWorld>): Ctx => ({ world, emit: noop, relaunch: noop });
  const opened = (ctx: Ctx) => open({ network: null }, ctx) as { network: { restored: boolean } };

  it("refuses a refused or no-space restore once, then opens the data as it was", () => {
    for (const name of ["refused:mismatchedBackup", "noSpace:diskFull"]) {
      const world = buildWorld("funded", null);
      world.openRestore = restoreFailure(name, FOLDER);
      const ctx = ctxOf(world);
      expect(() => open({}, ctx)).toThrow(expect.objectContaining({ code: "storage_restore", data: restoreFailure(name, FOLDER) }));
      expect(opened(ctx).network.restored).toBe(false);
    }
  });

  it("keeps a restore that couldn't start until Cancel restore, and one that stopped partway across a restart", () => {
    const world = buildWorld("funded", null);
    world.openRestore = restoreFailure("notStarted:permissionDenied", FOLDER);
    const ctx = ctxOf(world);
    restart({}, ctx);
    expect(() => open({}, ctx)).toThrow(expect.objectContaining({ code: "storage_restore" }));
    cancel({ network: null }, ctx);
    expect(opened(ctx).network.restored).toBe(false);

    world.openRestore = restoreFailure("incomplete:fileVanished", FOLDER);
    cancel({ network: null }, ctx);
    restart({}, ctx);
    expect(() => open({}, ctx)).toThrow(expect.objectContaining({ code: "storage_restore", data: expect.objectContaining({ state: "incomplete" }) }));
  });

  it("Keep keeps the foreign files: the next open opens them, restored", () => {
    const world = buildWorld("funded", null);
    world.openRestore = restoreFailure("foreign:otherLineage", FOLDER);
    const ctx = ctxOf(world);
    expect(() => open({}, ctx)).toThrow(expect.objectContaining({ code: "storage_restore", data: expect.objectContaining({ state: "foreign" }) }));
    keep({ network: null }, ctx);
    expect(opened(ctx).network.restored).toBe(true);
    expect(world.restoreCalls).toEqual(["keepCurrentFiles"]);
  });

  it("asks for a restore only with a verified snapshot, and runs it at the restart", () => {
    const world = buildWorld("funded", null);
    world.openRefusal = STORE_SCREENS.refusedRestore(FOLDER);
    const ctx = ctxOf(world);
    expect(available({ network: null }, ctx)).toBe(false);
    expect(() => request({ network: null }, ctx)).toThrow(expect.objectContaining({ code: "storage_restore", data: expect.objectContaining({ state: "refused", cause: "missingBackup" }) }));
    world.restoreAvailable = true;
    expect(available({ network: null }, ctx)).toBe(true);
    request({ network: null }, ctx);
    expect(() => open({}, ctx)).toThrow(expect.objectContaining({ code: "storage_refused" }));
    restart({}, ctx);
    expect(opened(ctx).network.restored).toBe(true);
    expect(active({}, ctx)).toMatchObject({ restored: true });
    expect(world.restoreCalls).toEqual(["restoreAvailable", "requestRestore", "restoreAvailable", "requestRestore"]);
    expect(world.appExits).toEqual(["restart"]);
  });
});
