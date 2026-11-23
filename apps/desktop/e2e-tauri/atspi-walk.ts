// The Linux screen-reader check: the real app on the real engine, read through the accessibility
// tree WebKitGTK hands to Orca (AT-SPI), not the DOM. At every state a user reaches without funds
// or a network, scripts/atspi-unnamed.py walks the app's AT-SPI tree and lists each interactive
// node (button, link, field, tab, check box…) that has no accessible name. The run fails if any
// screen has one, with a table of screen, role, path and nearby text; every result goes to
// $DW_E2E_OUT/atspi-unnamed.json. There is no allowlist: a decorative node exposed as
// interactive is a renderer defect.
//
// Flow: a fresh data root ▸ onboarding (Welcome, passphrase and its mismatch, the phrase and its
// check, Done) ▸ Settings ▸ Add Wallet (Welcome with Cancel, Restore, restore options, back).
// Then a dwcli regtest wallet, as smoke.ts makes it ▸ the lock screen (and a wrong passphrase)
// ▸ Forgot passphrase ▸ unlock for mixing only ▸ the unlock sheet ▸ every sidebar screen, Pay's
// tabs, Add contact (the sheet and a dashpay: link), Notifications, My Profile, Join DashPay ▸
// every Settings tab and the sheets it opens without funds ▸ every Tools tab ▸ Masternodes'
// tabs and Create proposal ▸ Open URI ▸ lock. Without funds there is no transaction to open.
// Screens that show the recovery phrase are walked with names and text dropped from the output,
// and at the end no file of OUT holds the phrases or the passphrase.
//
// The AT-SPI bus (at-spi-bus-launcher) is started on the private session bus that
// scripts/e2e-tauri.sh runs this under, before the app, so its accessibility bridge finds it.
// Settings, Tools and Open URI are opened with their menu shortcuts through xdotool.
// scripts/atspi-walk-docker.sh builds and runs it all in Docker under Xvfb:
//   DW_E2E_SCRIPT=e2e-tauri/atspi-walk.ts scripts/e2e-tauri.sh

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { APP, CHAIN, driverGone, dwcli, env, leaks, poll, startDriver, stopDriver, tid, TIMEOUT_MS, WebDriver } from "./driver.ts";

const OUT = resolve(env("DW_E2E_OUT", "test-results/tauri-atspi"));
const PORT = Number(env("DW_E2E_PORT", "4794"));
const WALKER = resolve("scripts/atspi-unnamed.py");
const BUS_LAUNCHER = env("DW_ATSPI_BUS_LAUNCHER", "/usr/libexec/at-spi-bus-launcher");
const PASSPHRASE = "atspi walk passphrase 3!";
const ONBOARDING_PASSPHRASE = "atspi onboarding passphrase 5?";
/** A Base58 identity id (32 bytes of 7) for the `dashpay://user` link, as smoke.ts. */
const IDENTITY = "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx";

// WebDriver key codes (W3C "keyboard actions").
const CONTROL = "";
const BACKSPACE = "";
const ESCAPE = "";

interface Unnamed {
  role: string;
  states: string[];
  path: { role: string; name: string }[];
  text: string;
  parent: string;
  siblings: { role: string; name: string }[];
  attributes: Record<string, string>;
}
interface ScreenResult {
  screen: string;
  interactive: number;
  walked: number;
  settled: boolean;
  unnamed: Unnamed[];
}

let driver: ChildProcess | undefined;
let bus: ChildProcess | undefined;
let work: string | undefined;
const results: ScreenResult[] = [];
const skipped: { screen: string; why: string }[] = [];

function run(cmd: string, args: string[]): string {
  const r = spawnSync(cmd, args, { encoding: "utf8" });
  if (r.status !== 0) throw new Error(`${cmd} ${args[0]} failed (${r.status}): ${(r.stderr || r.error?.message || "").slice(-600)}`);
  return r.stdout;
}

/** Presses a menu shortcut through the X server, as onboarding.ts does; the keys are released
 * whatever happens. */
function shortcut(keys: string) {
  try {
    run("xdotool", ["search", "--sync", "--onlyvisible", "--name", "^Dash Wallet", "windowfocus", "key", "--clearmodifiers", keys]);
  } finally {
    run("xdotool", ["keyup", keys]);
  }
}

/** Starts the AT-SPI bus on this session bus and waits until it answers. */
async function startAtspiBus() {
  bus = spawn(BUS_LAUNCHER, ["--launch-immediately"], { stdio: "ignore", detached: true });
  await poll(30_000, "the AT-SPI bus", async () => {
    const r = spawnSync("dbus-send", ["--session", "--print-reply", "--dest=org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus.GetAddress"], { encoding: "utf8" });
    return r.status === 0 && r.stdout.includes("unix:") ? true : undefined;
  });
}

function stopAtspiBus() {
  try {
    if (bus?.pid) process.kill(-bus.pid, "SIGTERM");
  } catch {
    // ESRCH
  }
}

/** Walks the app's AT-SPI tree for `screen` and records what it found. */
function walk(screen: string, redact = false) {
  const r = spawnSync("python3", ["-I", WALKER, "--wait", "30", ...(redact ? ["--redact"] : [])], { encoding: "utf8", timeout: 120_000 });
  const lines = (r.stdout ?? "").split("\n").filter((l) => l.trim() !== "");
  const parsed = lines.map(
    (l) => JSON.parse(l) as { unnamed?: Unnamed; summary?: { interactive: number; walked: number; settled: boolean; truncated: boolean }; error?: string },
  );
  const error = parsed.find((p) => p.error);
  if (r.status !== 0 || error || !parsed.some((p) => p.summary))
    throw new Error(`atspi-unnamed.py on ${screen}: ${error?.error ?? `status ${r.status}`} ${JSON.stringify(error ?? {})} ${(r.stderr ?? "").slice(-600)}`);
  const summary = parsed.find((p) => p.summary)!.summary!;
  if (summary.truncated) throw new Error(`atspi-unnamed.py on ${screen}: the tree has more nodes than it walks`);
  const unnamed = parsed.flatMap((p) => (p.unnamed ? [p.unnamed] : []));
  results.push({ screen, interactive: summary.interactive, walked: summary.walked, settled: summary.settled, unnamed });
  console.log(`atspi: ${screen}: ${summary.interactive} interactive, ${unnamed.length} unnamed${summary.settled ? "" : " (tree still changing)"}`);
}

/** The window's own ancestors (the frame, the web view, the document), left out of a path. */
const WINDOW_ROLES = ["application", "frame", "scroll pane", "document web"];
/** Unnamed boxes of these roles are layout only, also left out. */
const LAYOUT_ROLES = ["filler", "panel", "section"];
const pathText = (u: Unnamed) =>
  u.path
    .filter((p) => !WINDOW_ROLES.includes(p.role) && (p.name !== "" || !LAYOUT_ROLES.includes(p.role)))
    .slice(-4)
    .map((p) => (p.name ? `${p.role} "${p.name}"` : p.role))
    .join(" › ");

const nearText = (u: Unnamed) => {
  const near = [u.text && `text "${u.text}"`, u.parent && `parent "${u.parent}"`, ...u.siblings.filter((s) => s.name).map((s) => `${s.role} "${s.name}"`)];
  const tag = u.attributes.tag ? `<${u.attributes.tag}${u.attributes.id ? ` id="${u.attributes.id}"` : ""}>` : "";
  return [tag, ...near].filter(Boolean).join("; ").slice(0, 220);
};

async function main() {
  mkdirSync(OUT, { recursive: true });
  work = mkdtempSync(join(tmpdir(), "dw-atspi-")); // mode 0700
  const freshRoot = join(work, "fresh");
  const walletRoot = join(work, "wallet");
  const passFile = join(work, "passphrase");
  writeFileSync(passFile, `${PASSPHRASE}\n`, { mode: 0o600 });
  // A phrase to restore under Add Wallet, from a wallet of its own root (never funded).
  const phraseRoot = join(work, "phrase");
  dwcli(phraseRoot, passFile, "init-vault");
  const restorePhrase = /^mnemonic (.+)$/m.exec(dwcli(phraseRoot, passFile, "create"))?.[1]?.trim() ?? "";
  if (restorePhrase.split(" ").length !== 12) throw new Error("dwcli create printed no 12-word phrase");

  dwcli(walletRoot, passFile, "init-vault");
  const walletId = /wallet_id (\S+)/.exec(dwcli(walletRoot, passFile, "create"))?.[1];
  if (!walletId) throw new Error("dwcli create printed no wallet id");

  await startAtspiBus();
  const started = await startDriver(PORT, OUT);
  driver = started.driver;
  const wd = new WebDriver(started.base, OUT);
  const launch = (root: string) =>
    wd.start({ alwaysMatch: { "tauri:options": { application: APP, args: ["--datadir", root, ...CHAIN, "--theme", "light"] } } });

  const exists = async (css: string) => (await wd.find(css)) !== null;
  const texts = (css: string) => wd.script<string[]>(`return [...document.querySelectorAll(arguments[0])].map((e) => e.textContent.trim())`, [css]);
  const ids = (css: string) =>
    wd.script<string[]>(`return [...document.querySelectorAll(arguments[0])].filter((e) => e.offsetParent !== null).map((e) => e.dataset.testid)`, [css]);
  const clickText = async (css: string, text: string) => {
    for (const el of await wd.findAll(css)) if ((await wd.text(el)).trim() === text) return void (await wd.click(el));
    throw new Error(`no ${css} reads the expected text`);
  };
  const waitText = (css: string, want: RegExp, what: string) =>
    poll(TIMEOUT_MS, what, async () => {
      const el = await wd.find(css);
      const t = el ? await wd.text(el) : "";
      return want.test(t) ? t : undefined;
    });
  /** Types `text` into a field, checking by length that every key arrived (as onboarding.ts). */
  const enter = async (id: string, text: string) => {
    for (let i = 0; i < 3; i++) {
      const el = await wd.waitFor(tid(id));
      await wd.type(el, i === 0 ? text : `${CONTROL}a${CONTROL}${BACKSPACE}${text}`);
      if ((await wd.script<number>(`return document.querySelector(arguments[0])?.value.length ?? -1`, [tid(id)])) === text.length) return;
    }
    throw new Error(`${id} did not take the text`);
  };
  const dialogs = () => wd.script<number>(`return document.querySelectorAll('[role="dialog"][aria-modal="true"]').length`);
  /** Closes the topmost sheet with its Close button, else Escape. A WebDriver click, not one from a
   * script: with the AT-SPI bridge on, a click inside `execute/sync` that closes a sheet can leave
   * WebKitWebDriver without the script's result. */
  const closeSheet = async () => {
    const before = await dialogs();
    if (before === 0) return;
    const close = (await wd.findAll('[role="dialog"][aria-modal="true"] .sheet-close:not(:disabled)')).pop();
    if (close) await wd.click(close);
    else await wd.call("POST", wd.s("/actions"), { actions: [{ type: "key", id: "kb", actions: [{ type: "keyDown", value: ESCAPE }, { type: "keyUp", value: ESCAPE }] }] });
    await poll(TIMEOUT_MS, "the sheet to close", async () => ((await dialogs()) < before ? true : undefined));
  };
  /** Opens a sheet with `opener`, waits for `ready` (default: one more dialog), walks it, closes it. */
  const sheet = async (screen: string, opener: string, ready?: string) => {
    try {
      const before = await dialogs();
      await wd.press(opener);
      if (ready) await wd.waitFor(ready, screen);
      else await poll(TIMEOUT_MS, `${screen} to open`, async () => ((await dialogs()) > before ? true : undefined));
      walk(screen);
      await closeSheet();
    } catch (e) {
      throw new Error(`${screen}: ${e instanceof Error ? e.message : String(e)}`);
    }
  };
  /** Like `sheet`, but a state that is not on this wallet is recorded as skipped. */
  const maybeSheet = async (screen: string, opener: string, ready?: string) => {
    if (!(await exists(opener))) return void skipped.push({ screen, why: `${opener} is not shown` });
    await sheet(screen, opener, ready);
  };
  /** Each tab of the tablist `prefix` (data-testid `${prefix}-<id>`), walked as `${label}: <title>`. */
  const eachTab = async (prefix: string, label: string, ready?: (id: string) => Promise<void>, after?: (id: string) => Promise<void>) => {
    const tabs = await ids(`[role="tab"][data-testid^="${prefix}-"]`);
    if (tabs.length === 0) throw new Error(`no tabs in ${prefix}`);
    for (const id of tabs) {
      await wd.press(tid(id));
      await poll(TIMEOUT_MS, `${id} selected`, async () =>
        (await wd.script<string | null>(`return document.querySelector(arguments[0])?.getAttribute("aria-selected")`, [tid(id)])) === "true" ? true : undefined,
      );
      await ready?.(id);
      walk(`${label}: ${(await texts(tid(id)))[0] ?? id}`);
      await after?.(id);
    }
  };

  let created: string[] = [];
  let failure: unknown = null;
  try {
    // ---------------------------------------------------------------- onboarding, fresh root
    await launch(freshRoot);
    await wd.waitFor(tid("onboarding-welcome"), "Welcome on a fresh data root");
    if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");
    walk("onboarding: welcome");
    await wd.press(tid("onboarding-create"));
    await wd.waitFor(tid("onboarding-passphrase-new"), "Choose passphrase");
    walk("onboarding: choose passphrase");
    await enter("onboarding-passphrase-new", ONBOARDING_PASSPHRASE);
    await enter("onboarding-passphrase-repeat", `${ONBOARDING_PASSPHRASE}x`);
    await wd.press(tid("onboarding-passphrase-submit"));
    await waitText(tid("onboarding-passphrase-error"), /./, "the mismatch message");
    walk("onboarding: passphrase mismatch");
    await enter("onboarding-passphrase-new", ONBOARDING_PASSPHRASE);
    await enter("onboarding-passphrase-repeat", ONBOARDING_PASSPHRASE);
    await wd.press(tid("onboarding-passphrase-submit"));
    await wd.waitFor(tid("onboarding-phrase-words"), "the recovery phrase");
    created = await texts(`${tid("onboarding-phrase-words")} .phrase-word`);
    if (created.length !== 12) throw new Error(`the phrase step shows ${created.length} words`);
    walk("onboarding: recovery phrase", true);
    await wd.press(tid("onboarding-written-down"));
    await wd.press(tid("onboarding-phrase-continue"));
    await wd.waitFor(tid("onboarding-verifyPhrase"), "the verify step");
    walk("onboarding: verify phrase", true);
    const positions = (await texts(`${tid("verify-slot")} .verify-slot-label`)).map((l) => Number(l.replace(/\D+/g, "")) - 1);
    for (const p of positions) await clickText(tid("verify-chip"), created[p]!);
    await wd.waitFor(tid("onboarding-done"), "Done");
    walk("onboarding: done");
    await wd.press(tid("onboarding-open"));
    await wd.waitFor(tid("hero"), "the Overview of the created wallet");

    // Add Wallet: Welcome with Cancel, Restore, its options, then back out.
    shortcut("ctrl+comma");
    await wd.press(tid("settings-tabs-wallets"));
    await wd.press(tid("settings-add-wallet"));
    await wd.waitFor(tid("onboarding-cancel"), "Add Wallet");
    walk("add wallet: welcome");
    await wd.press(tid("onboarding-restore"));
    await wd.waitFor(tid("restore-input"), "the restore phrase field");
    walk("add wallet: restore phrase");
    const input = await wd.waitFor(tid("restore-input"));
    await wd.type(input, restorePhrase);
    await wd.waitFor(tid("restore-valid"), "the valid-phrase note");
    walk("add wallet: restore phrase, valid", true);
    await wd.press(tid("restore-continue"));
    await wd.waitFor(tid("onboarding-restoreOptions"), "the restore options");
    walk("add wallet: restore options");
    while (!(await exists(tid("onboarding-cancel")))) await wd.press(tid("onboarding-back"));
    await wd.press(tid("onboarding-cancel"));
    await wd.waitGone(tid("onboarding-cancel"), "Add Wallet");
    await wd.end();

    // ---------------------------------------------------------------- the dwcli wallet
    await launch(walletRoot);
    await wd.waitFor(tid("lock-field"), "the lock screen");
    walk("lock screen");
    await enter("lock-field", "not the passphrase");
    await wd.press(tid("lock-submit"));
    await waitText(tid("lock-error"), /./, "the wrong-passphrase message");
    walk("lock screen: wrong passphrase");

    await wd.press(tid("lock-forgot"));
    // One wallet and nothing else to lose: the sheet opens on the phrase.
    const forgotSteps = [tid("forgot-wallet"), tid("forgot-consequences"), tid("forgot-phrase-input")].join(", ");
    await wd.waitFor(forgotSteps, "Forgot passphrase");
    if (await exists(tid("forgot-wallet"))) {
      walk("forgot passphrase: choose wallet");
      await wd.click((await wd.findAll(tid("forgot-wallet")))[0]!);
      await wd.waitFor(`${tid("forgot-consequences")}, ${tid("forgot-phrase-input")}`, "the next step");
    }
    if (await exists(tid("forgot-consequences"))) {
      walk("forgot passphrase: what happens");
      await wd.press(tid("forgot-accept"));
    }
    await wd.waitFor(tid("forgot-phrase-input"), "the phrase field");
    walk("forgot passphrase: recovery phrase");
    await closeSheet();
    await wd.waitFor(tid("lock-field"), "the lock screen");

    await wd.press(tid("lock-mixing-only"));
    await enter("lock-field", PASSPHRASE);
    await wd.press(tid("lock-submit"));
    await wd.waitGone(tid("lock-screen"), "the lock screen");
    await wd.waitFor(tid("hero"), "the Overview");
    await wd.waitFor(tid("status-unlock"), "the mixing-only status");
    walk("home: unlocked for mixing only");
    await wd.press(tid("status-unlock"));
    await wd.waitFor(tid("unlock-sheet-field"), "the unlock sheet");
    walk("unlock sheet");
    await enter("unlock-sheet-field", PASSPHRASE);
    await wd.press(tid("unlock-sheet-submit"));
    await wd.waitGone(tid("unlock-sheet"), "the unlock sheet");

    // Every sidebar screen; Pay's tabs; Add contact.
    for (const id of await ids(`${tid("sidebar")} button[data-testid^="sidebar-"]`)) {
      const route = id.slice("sidebar-".length);
      await wd.press(tid(id));
      await wd.waitFor(tid(`page-${route}`), route);
      if (route === "home") await wd.waitFor(`${tid("history-empty")}, ${tid("history-syncing")}`, "the empty history");
      if (route === "activity") await wd.waitFor(`${tid("tx-empty")}, ${tid("tx-syncing")}`, "the empty Activity");
      walk(`${(await texts(`${tid(id)} .sidebar-title`))[0] || route}`);
      if (route === "pay") await eachTab("pay-tabs", "Pay");
      if (route === "contacts") await maybeSheet("contacts: add contact", tid("contacts-add"), tid("add-contact"));
      if (route === "joinDashPay") {
        await maybeSheet("join DashPay: questions", tid("join-faq"));
        await maybeSheet("join DashPay: username voting", tid("join-voting-info"));
        await maybeSheet("join DashPay: invitation", tid("join-invitation"));
      }
      if (route === "activity" && (await ids('[data-testid^="tx-row"]')).length === 0) skipped.push({ screen: "transaction detail", why: "no transaction without funds" });
    }
    // Contacts (Ctrl+4): without an identity the sidebar offers Join DashPay in its place.
    if (!(await exists(tid("page-contacts")))) {
      shortcut("ctrl+4");
      await wd.waitFor(tid("page-contacts"), "Contacts");
      walk("Contacts");
      await maybeSheet("contacts: add contact", tid("contacts-add"), tid("add-contact"));
    }
    // Add contact from a dashpay: link, forwarded by a second launch (smoke.ts).
    const forward = spawnSync(APP, ["--datadir", walletRoot, `dashpay://user?id=${IDENTITY}&username=bob`], { encoding: "utf8", timeout: 60_000, killSignal: "SIGKILL" });
    if (forward.status !== 0) throw new Error(`the second launch did not hand over its link (status ${forward.status})`);
    await wd.waitFor(tid("add-contact-link-error"), "Add contact to give up on the absent Platform");
    walk("contacts: add contact from a link");
    await closeSheet();

    // The toolbar: notifications, the identity chip (My Profile; without an identity, Join DashPay,
    // which the sidebar opened above).
    await wd.press(tid("toolbar-bell"));
    await poll(TIMEOUT_MS, "the notifications", async () => ((await exists(tid("bell-popover"))) || (await exists(tid("page-notifications"))) ? true : undefined));
    walk(`notifications${(await exists(tid("bell-popover"))) ? " (bell)" : ""}`);
    if (await exists(tid("bell-see-all"))) {
      await wd.press(tid("bell-see-all"));
      await wd.waitFor(tid("page-notifications"), "Notifications");
      walk("notifications");
    }
    if (await exists(tid("toolbar-chip"))) {
      await wd.press(tid("toolbar-chip"));
      await poll(TIMEOUT_MS, "the identity page", async () => ((await exists(tid("page-joinDashPay"))) || (await exists(tid("page-profile"))) ? true : undefined));
      walk((await exists(tid("page-profile"))) ? "my profile" : "join DashPay (toolbar)");
    }

    // Settings: every tab, and the sheets each opens without funds.
    shortcut("ctrl+comma");
    await wd.waitFor(tid("page-settings"), "Settings");
    const settingsSheets: Record<string, [string, string, string?][]> = {
      general: [["settings: reset settings", tid("settings-reset")]],
      security: [
        ["settings: change passphrase", tid("settings-change-passphrase")],
        ["settings: payment authentication", tid("settings-payment-auth"), tid("settings-payment-auth-sheet")],
        ["settings: show recovery phrase", tid("settings-security-recovery-phrase")],
        ["settings: wipe", tid("settings-wipe")],
      ],
      wallets: [
        ["settings: rename wallet", '[data-testid^="settings-wallet-"][data-testid$="-rename"]'],
        ["settings: extended public key", '[data-testid^="settings-wallet-"][data-testid$="-xpub"]', tid("settings-xpub-qr")],
        ["settings: add watch-only wallet", tid("settings-add-watch-only")],
      ],
      backups: [
        ["settings: back up wallet", tid("settings-backup")],
        ["settings: export keys", tid("settings-export")],
        ["settings: import keys", tid("settings-import-keys")],
        ["settings: import file", tid("settings-import-file")],
      ],
    };
    await eachTab("settings-tabs", "settings", undefined, async (id) => {
      for (const [screen, opener, ready] of settingsSheets[id.slice("settings-tabs-".length)] ?? []) await maybeSheet(screen, opener, ready);
    });

    // Tools: every tab.
    shortcut("ctrl+shift+i");
    await wd.waitFor(tid("page-tools"), "Tools");
    await eachTab("tools-tabs", "tools");

    // Masternodes, once Settings ▸ General shows them: each tab and Create proposal.
    shortcut("ctrl+comma");
    await wd.press(tid("settings-tabs-general"));
    await wd.press(tid("settings-show-masternodes"));
    // Governance data is off on regtest: the setting asks whether to load it; no, stay offline.
    await wd.waitFor(tid("settings-governance-ask"), "Load governance data?");
    walk("settings: load governance data?");
    await closeSheet();
    await wd.press(tid("sidebar-masternodes"));
    await wd.waitFor(tid("page-masternodes"), "Masternodes");
    await eachTab("masternodes-tabs", "masternodes", undefined, async (id) => {
      if (id === "masternodes-tabs-governance") await maybeSheet("masternodes: create proposal", tid("governance-create"));
    });

    // Shell dialogs: Open URI (Ctrl+O).
    shortcut("ctrl+o");
    await poll(TIMEOUT_MS, "Open URI", async () => ((await dialogs()) > 0 ? true : undefined));
    walk("open URI");
    await closeSheet();

    await wd.press(tid("toolbar-lock"));
    await wd.waitFor(tid("lock-field"), "the lock screen after locking");
  } catch (e) {
    failure = e;
    const onPhrase = () => wd.script<boolean>(`return !!document.querySelector('.phrase-grid, .verify-chips, .phrase-input')`).catch(() => true);
    if (!(await onPhrase())) {
      const shown = await ids("[data-testid]").catch(() => []);
      writeFileSync(join(OUT, "failure-testids.json"), `${JSON.stringify(shown)}\n`);
      await wd.screenshot("failure").catch(() => undefined);
      if (await onPhrase()) rmSync(join(OUT, "failure.png"), { force: true });
    }
  } finally {
    await wd.end().catch(() => undefined);
  }

  stopDriver(driver);
  driver = undefined;
  await driverGone(started.driver);
  stopAtspiBus();

  const unnamed = results.flatMap((r) => r.unnamed.map((u) => ({ screen: r.screen, ...u })));
  const report = {
    ok: failure === null && unnamed.length === 0,
    screens: results.length,
    interactive: results.reduce((n, r) => n + r.interactive, 0),
    unnamed: unnamed.length,
    skipped,
    error: failure ? (failure instanceof Error ? failure.message : String(failure)) : null,
    results,
  };
  writeFileSync(join(OUT, "atspi-unnamed.json"), `${JSON.stringify(report, null, 2)}\n`);
  const secrets = [restorePhrase, created.join(" "), PASSPHRASE, ONBOARDING_PASSPHRASE].filter((s) => s !== "");
  const found = leaks(OUT, secrets);
  if (found.length > 0) {
    for (const f of found) rmSync(f.replace(/ \(\d+\)$/, ""), { force: true });
    throw new Error(`secrets in plain text: ${found.join(", ")}`);
  }

  console.log(`atspi: ${results.length} screens, ${report.interactive} interactive nodes checked, ${unnamed.length} unnamed`);
  for (const s of skipped) console.log(`atspi: skipped ${s.screen}: ${s.why}`);
  if (unnamed.length > 0) {
    const rows = unnamed.map((u) => [u.screen, u.role, pathText(u), nearText(u)]);
    const head = ["screen", "role", "path", "nearby"];
    const width = head.map((h, i) => Math.min(70, Math.max(h.length, ...rows.map((r) => r[i]!.length))));
    const line = (cells: string[]) => cells.map((c, i) => (i < 3 ? c.slice(0, width[i]).padEnd(width[i]!) : c)).join(" │ ");
    console.error(line(head));
    for (const r of rows) console.error(line(r));
  }
  if (failure) throw failure;
  if (unnamed.length > 0) throw new Error(`${unnamed.length} interactive nodes without an accessible name (${join(OUT, "atspi-unnamed.json")})`);
  console.log(`atspi: PASS; results in ${join(OUT, "atspi-unnamed.json")}`);
}

main()
  .catch((e) => {
    console.error(`atspi: FAIL: ${e instanceof Error ? e.message : String(e)}`);
    process.exitCode = 1;
  })
  .finally(() => {
    stopDriver(driver);
    stopAtspiBus();
    try {
      if (work) rmSync(work, { recursive: true, force: true, maxRetries: 3 });
    } catch (e) {
      console.warn(`atspi: could not remove ${work}: ${e instanceof Error ? e.message : String(e)}`);
    }
  });
