// The Linux screen-reader check: the real app on the real engine, read through the accessibility
// tree WebKitGTK hands to Orca (AT-SPI), not the DOM. At every state a user reaches without funds
// or a network, scripts/atspi-unnamed.py walks the app's AT-SPI tree and lists each interactive
// node (button, link, field, tab, check box…) that has no accessible name. The run fails if any
// screen has one, with a table of screen, role, path and nearby text. There is no allowlist: a
// decorative node exposed as interactive is a renderer defect.
//
// A state counts only when the tree has caught up with it: each walk names a node of that state
// (a sheet's dialog, the step's heading, the tab's own control), read from the DOM once a
// control only that state has is showing, and the walker waits until that node is in the AT-SPI
// tree, there is at least one interactive node, every read succeeded and two walks agree. A state
// that does not get there fails the run, and so does one it cannot reach, except the few that do
// not exist without funds or an identity (EXPECTED_SKIPS).
//
// Each run writes to a directory of its own, $DW_E2E_OUT/atspi-<time>, which it creates: the
// driver's log, atspi-unnamed.json with every screen's result, and on failure the test ids shown
// and a screenshot. The leak check reads only that directory.
//
// Flow: a fresh data root ▸ onboarding (Welcome, passphrase and its mismatch, the phrase and its
// check, Done) ▸ Settings ▸ Add Wallet (Welcome with Cancel, Restore, restore options, back).
// Then a dwcli regtest wallet, as smoke.ts makes it ▸ the lock screen (and a wrong passphrase)
// ▸ Forgot passphrase ▸ unlock for mixing only ▸ the unlock sheet ▸ every sidebar screen, Pay's
// tabs, a payment request (its sheet and the list), Join DashPay's sheets, Contacts, Add contact
// from a dashpay: link, Notifications ▸
// every Settings tab and the sheets it opens without funds ▸ every Tools tab ▸ Masternodes'
// tabs and Create proposal ▸ Open URI ▸ lock. Without funds there is no transaction to open.
// Screens that show the recovery phrase are walked with names and text dropped from the output,
// and at the end no file the run wrote holds the phrases or the passphrases.
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
import { MARKER_SCRIPT } from "./atspi-marker.ts";
import { APP, CHAIN, driverGone, dwcli, env, leaks, poll, startDriver, stopDriver, tid, TIMEOUT_MS, WebDriver } from "./driver.ts";
import { canonical, removeOwned } from "./scratch.ts";

const OUT_BASE = resolve(env("DW_E2E_OUT", "test-results/tauri-atspi"));
/** This run's own directory, created by `main` (it may not exist yet). */
const OUT = join(OUT_BASE, `atspi-${new Date().toISOString().replace(/[:.]/g, "-")}`);
const PORT = Number(env("DW_E2E_PORT", "4794"));
const WALKER = resolve("scripts/atspi-unnamed.py");
const BUS_LAUNCHER = env("DW_ATSPI_BUS_LAUNCHER", "/usr/libexec/at-spi-bus-launcher");
const PASSPHRASE = "atspi walk passphrase 3!";
const ONBOARDING_PASSPHRASE = "atspi onboarding passphrase 5?";
/** A Base58 identity id (32 bytes of 7) for the `dashpay://user` link, as smoke.ts. */
const IDENTITY = "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx";
/** How long an xdotool call may take before the run gives up on it. */
const XDOTOOL_TIMEOUT_MS = 15_000;

/** States this wallet cannot show, and why. Any other state the walk does not reach fails it. */
const EXPECTED_SKIPS: Record<string, string> = {
  "transaction detail": "no transaction without funds",
  "contacts: add contact (header)": "Contacts' header offers Add contact only with a DashPay identity, which needs Platform",
  "my profile": "My Profile needs a DashPay identity, which needs Platform; docs/qa/H-08-macos-windows.md covers it by hand",
};

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
/** A node the state shows: an Atspi.Role name (HEADING, DIALOG, PUSH_BUTTON…) and its name. */
type Marker = [role: string, name: string];
interface Summary {
  interactive: number;
  walked: number;
  settled: boolean;
  missing: string[];
  truncated: number;
  read_failures: number;
}
interface ScreenResult {
  screen: string;
  expect: Marker[];
  interactive: number;
  walked: number;
  settled: boolean;
  missing: string[];
  truncated: number;
  readFailures: number;
  /** Why the walk does not count: empty when it does. */
  problems: string[];
  unnamed: Unnamed[];
}

let driver: ChildProcess | undefined;
let bus: ChildProcess | undefined;
let work: string | undefined;
const results: ScreenResult[] = [];
const skipped: { screen: string; why: string }[] = [];

function run(cmd: string, args: string[], timeout: number): string {
  const r = spawnSync(cmd, args, { encoding: "utf8", timeout, killSignal: "SIGKILL" });
  if ((r.error as NodeJS.ErrnoException | undefined)?.code === "ETIMEDOUT")
    throw new Error(`${cmd} ${args.join(" ")} did not finish in ${timeout / 1000} s (no window to send keys to?)`);
  if (r.status !== 0) throw new Error(`${cmd} ${args[0]} failed (${r.status ?? r.signal}): ${(r.stderr || r.error?.message || "").slice(-600)}`);
  return r.stdout;
}

/** Presses a menu shortcut through the X server, as onboarding.ts does; the keys are released
 * whatever happens. */
function shortcut(keys: string) {
  try {
    run("xdotool", ["search", "--sync", "--onlyvisible", "--name", "^DashPay Desktop", "windowfocus", "key", "--clearmodifiers", keys], XDOTOOL_TIMEOUT_MS);
  } finally {
    run("xdotool", ["keyup", keys], XDOTOOL_TIMEOUT_MS);
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

/** Walks the app's AT-SPI tree for `screen`, once it shows every node of `expect`, and records
 * what it found; a walk that did not get there is recorded with its problems. */
function walk(screen: string, expect: Marker[], redact = false) {
  const args = ["-I", WALKER, "--wait", "30", "--min-interactive", "1", ...expect.flatMap(([role, name]) => ["--expect", `${role}=${name}`])];
  const r = spawnSync("python3", [...args, ...(redact ? ["--redact"] : [])], { encoding: "utf8", timeout: 120_000 });
  const lines = (r.stdout ?? "").split("\n").filter((l) => l.trim() !== "");
  const parsed = lines.map((l) => JSON.parse(l) as { unnamed?: Unnamed; summary?: Summary; error?: string });
  const error = parsed.find((p) => p.error);
  if (r.status !== 0 || error || !parsed.some((p) => p.summary))
    throw new Error(`atspi-unnamed.py on ${screen}: ${error?.error ?? `status ${r.status}`} ${JSON.stringify(error ?? {})} ${(r.stderr ?? "").slice(-600)}`);
  const summary = parsed.find((p) => p.summary)!.summary!;
  const problems = [
    summary.settled ? "" : "the tree did not settle",
    summary.interactive === 0 ? "no interactive node" : "",
    summary.missing.length > 0 ? `not in the tree: ${summary.missing.join(", ")}` : "",
    summary.truncated > 0 ? `${summary.truncated} parents with more children than it reads` : "",
    summary.read_failures > 0 ? `${summary.read_failures} nodes it could not read` : "",
  ].filter(Boolean);
  const unnamed = parsed.flatMap((p) => (p.unnamed ? [p.unnamed] : []));
  results.push({
    screen,
    expect,
    interactive: summary.interactive,
    walked: summary.walked,
    settled: summary.settled,
    missing: summary.missing,
    truncated: summary.truncated,
    readFailures: summary.read_failures,
    problems,
    unnamed,
  });
  console.log(`atspi: ${screen}: ${summary.interactive} interactive, ${unnamed.length} unnamed${problems.length ? `; ${problems.join("; ")}` : ""}`);
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
  mkdirSync(OUT_BASE, { recursive: true });
  mkdirSync(OUT, { mode: 0o700 }); // not recursive: fails if it exists, so every file in it is this run's
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
  const started = await startDriver(PORT, OUT, (child) => (driver = child));
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
  /** The node the tree must show for the element `css`, read from the DOM: a dialog's name; a
   * button's, link's, tab's or field's own name; else the first visible heading in it, else its
   * first visible button or link. Null when it has none. */
  const markerOf = (css: string) => wd.script<Marker | null>(MARKER_SCRIPT, [css]);
  /** Walks `screen` once every element of `css` is showing; the first of them is only on this
   * state, and the tree must show its marker (`markerOf`). On a redacted screen the marker is a
   * heading or a dialog's name, never a word of the phrase. */
  const walkAt = async (screen: string, css: string | string[], redact = false) => {
    const all = typeof css === "string" ? [css] : css;
    for (const c of all) await wd.waitFor(c, `${screen}: ${c}`);
    const marker = await markerOf(all[0]!);
    if (!marker) throw new Error(`${screen}: ${all[0]} has no dialog name, heading or control to look for in the tree`);
    if (redact && !["HEADING", "DIALOG"].includes(marker[0])) throw new Error(`${screen}: a redacted screen's marker must be a heading or a dialog`);
    walk(screen, [marker], redact);
  };
  /** Opens the sheet `dialog` (its data-testid) with `opener`, walks it, closes it. */
  const sheet = async (screen: string, opener: string, dialog: string, more: string[] = []) => {
    try {
      await wd.press(opener);
      await walkAt(screen, [tid(dialog), ...more]);
      await closeSheet();
    } catch (e) {
      throw new Error(`${screen}: ${e instanceof Error ? e.message : String(e)}`);
    }
  };
  /** Records `screen` as skipped: only a state of EXPECTED_SKIPS may be. */
  const skip = (screen: string) => {
    const why = EXPECTED_SKIPS[screen];
    if (!why) throw new Error(`${screen}: not reached, and not a state this wallet is expected to lack`);
    skipped.push({ screen, why });
  };
  /** Each tab of the tablist `prefix` (data-testid `${prefix}-<id>`), walked as `${label}: <title>`
   * once the panel shows the element `dest[<id>]` (a test id, or the test ids of the states it may
   * settle in), which only that tab has. A tab missing from `dest` fails the run. */
  const eachTab = async (prefix: string, label: string, dest: Record<string, string | string[]>, after?: (id: string) => Promise<void>) => {
    const tabs = await ids(`[role="tab"][data-testid^="${prefix}-"]`);
    if (tabs.length === 0) throw new Error(`no tabs in ${prefix}`);
    for (const id of tabs) {
      const key = id.slice(prefix.length + 1);
      if (!dest[key]) throw new Error(`${id}: no destination control listed for this tab`);
      await wd.press(tid(id));
      await poll(TIMEOUT_MS, `${id} selected`, async () =>
        (await wd.script<string | null>(`return document.querySelector(arguments[0])?.getAttribute("aria-selected")`, [tid(id)])) === "true" ? true : undefined,
      );
      const panel = [dest[key]].flat().map((d) => `[role="tabpanel"] ${tid(d)}`).join(", ");
      await walkAt(`${label}: ${(await texts(tid(id)))[0] ?? id}`, panel);
      await after?.(key);
    }
  };

  let created: string[] = [];
  let failure: unknown = null;
  try {
    // ---------------------------------------------------------------- onboarding, fresh root
    // Each step's marker is its heading, read once a control only that step has is showing.
    const step = (screen: string, control: string, redact = false) => walkAt(screen, [".onboarding-body", tid(control)], redact);
    await launch(freshRoot);
    await wd.waitFor(tid("onboarding-welcome"), "Welcome on a fresh data root");
    if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");
    await step("onboarding: welcome", "onboarding-create");
    await wd.press(tid("onboarding-create"));
    await step("onboarding: choose passphrase", "onboarding-passphrase-new");
    await enter("onboarding-passphrase-new", ONBOARDING_PASSPHRASE);
    await enter("onboarding-passphrase-repeat", `${ONBOARDING_PASSPHRASE}x`);
    await wd.press(tid("onboarding-passphrase-submit"));
    await waitText(tid("onboarding-passphrase-error"), /./, "the mismatch message");
    await step("onboarding: passphrase mismatch", "onboarding-passphrase-error");
    await enter("onboarding-passphrase-new", ONBOARDING_PASSPHRASE);
    await enter("onboarding-passphrase-repeat", ONBOARDING_PASSPHRASE);
    await wd.press(tid("onboarding-passphrase-submit"));
    await wd.waitFor(tid("onboarding-phrase-words"), "the recovery phrase");
    created = await texts(`${tid("onboarding-phrase-words")} .phrase-word`);
    if (created.length !== 12) throw new Error(`the phrase step shows ${created.length} words`);
    await step("onboarding: recovery phrase", "onboarding-phrase-words", true);
    await wd.press(tid("onboarding-written-down"));
    await wd.press(tid("onboarding-phrase-continue"));
    await step("onboarding: verify phrase", "onboarding-verifyPhrase", true);
    const positions = (await texts(`${tid("verify-slot")} .verify-slot-label`)).map((l) => Number(l.replace(/\D+/g, "")) - 1);
    for (const p of positions) await clickText(tid("verify-chip"), created[p]!);
    await step("onboarding: done", "onboarding-done");
    await wd.press(tid("onboarding-open"));
    await wd.waitFor(tid("hero"), "the Overview of the created wallet");

    // Add Wallet: Welcome with Cancel, Restore, its options, then back out.
    shortcut("ctrl+comma");
    await wd.press(tid("settings-tabs-wallets"));
    await wd.press(tid("settings-add-wallet"));
    await step("add wallet: welcome", "onboarding-cancel");
    await wd.press(tid("onboarding-restore"));
    await step("add wallet: restore phrase", "restore-input");
    const input = await wd.waitFor(tid("restore-input"));
    await wd.type(input, restorePhrase);
    await step("add wallet: restore phrase, valid", "restore-valid", true);
    await wd.press(tid("restore-continue"));
    await step("add wallet: restore options", "onboarding-restoreOptions");
    while (!(await exists(tid("onboarding-cancel")))) await wd.press(tid("onboarding-back"));
    await wd.press(tid("onboarding-cancel"));
    await wd.waitGone(tid("onboarding-cancel"), "Add Wallet");
    await wd.end();

    // ---------------------------------------------------------------- the dwcli wallet
    await launch(walletRoot);
    await walkAt("lock screen", [tid("lock-screen"), tid("lock-field")]);
    await enter("lock-field", "not the passphrase");
    await wd.press(tid("lock-submit"));
    await waitText(tid("lock-error"), /./, "the wrong-passphrase message");
    await walkAt("lock screen: wrong passphrase", [tid("lock-screen"), tid("lock-error")]);

    await wd.press(tid("lock-forgot"));
    // One wallet and nothing else to lose: the sheet opens on the phrase.
    const forgotSteps = [tid("forgot-wallet"), tid("forgot-consequences"), tid("forgot-phrase-input")].join(", ");
    await wd.waitFor(forgotSteps, "Forgot passphrase");
    if (await exists(tid("forgot-wallet"))) {
      await walkAt("forgot passphrase: choose wallet", [tid("forgot-passphrase"), tid("forgot-wallet")]);
      await wd.click((await wd.findAll(tid("forgot-wallet")))[0]!);
      await wd.waitFor(`${tid("forgot-consequences")}, ${tid("forgot-phrase-input")}`, "the next step");
    }
    if (await exists(tid("forgot-consequences"))) {
      await walkAt("forgot passphrase: what happens", [tid("forgot-accept"), tid("forgot-consequences")]);
      await wd.press(tid("forgot-accept"));
    }
    await walkAt("forgot passphrase: recovery phrase", [tid("forgot-continue"), tid("forgot-phrase-input")]);
    await closeSheet();
    await wd.waitFor(tid("lock-field"), "the lock screen");

    await wd.press(tid("lock-mixing-only"));
    await enter("lock-field", PASSPHRASE);
    await wd.press(tid("lock-submit"));
    await wd.waitGone(tid("lock-screen"), "the lock screen");
    await walkAt("home: unlocked for mixing only", [tid("status-unlock"), tid("hero")]);
    await wd.press(tid("status-unlock"));
    await walkAt("unlock sheet", [tid("unlock-sheet"), tid("unlock-sheet-field")]);
    await enter("unlock-sheet-field", PASSPHRASE);
    await wd.press(tid("unlock-sheet-submit"));
    await wd.waitGone(tid("unlock-sheet"), "the unlock sheet");

    // Contacts' header Add contact, while Contacts shows.
    const contactsAdd = async () => {
      if (await exists(tid("contacts-add"))) await sheet("contacts: add contact (header)", tid("contacts-add"), "add-contact");
      else skip("contacts: add contact (header)");
    };
    // Every sidebar screen, the Pay contact list, and Join DashPay’s sheets.
    let contactsWalked = false;
    for (const id of await ids(`${tid("sidebar")} button[data-testid^="sidebar-"]`)) {
      const route = id.slice("sidebar-".length);
      await wd.press(tid(id));
      const more = route === "home" ? [`${tid("history-empty")}, ${tid("history-syncing")}`] : route === "activity" ? [`${tid("tx-empty")}, ${tid("tx-syncing")}`] : [];
      await walkAt(`${(await texts(`${tid(id)} .sidebar-title`))[0] || route}`, [tid(`page-${route}`), ...more]);
      if (route === "pay") {
        await wd.press(tid("pay-contacts"));
        await walkAt("Pay: contacts", [tid("pay-contact-none"), tid("pay-address")]);
        await wd.press(tid("pay-contacts"));
      }
      if (route === "receive") {
        // A request for any amount: the wallet issues an address and keeps the request, offline.
        await poll(TIMEOUT_MS, "Create Request to be enabled", async () =>
          (await wd.script<boolean>(`const b = document.querySelector(arguments[0]); return !!b && !b.disabled && b.getAttribute("aria-disabled") !== "true"`, [tid("request-create")])) ? true : undefined,
        );
        await wd.press(tid("request-create"));
        await walkAt("receive: payment request", [tid("request-sheet"), tid("request-qr")]);
        await closeSheet();
        await walkAt("receive: with a request", [tid("requests-table"), '[data-testid^="request-show-"]']);
      }
      if (route === "contacts") {
        contactsWalked = true;
        await contactsAdd();
      }
      if (route === "joinDashPay") {
        await sheet("join DashPay: questions", tid("join-faq"), "dashpay-faq");
        await sheet("join DashPay: username voting", tid("join-voting-info"), "voting-info");
        await sheet("join DashPay: invitation", tid("join-invitation"), "claim-paste");
      }
      if (route === "activity" && (await ids('[data-testid^="tx-row"]')).length === 0) skip("transaction detail");
    }
    // Contacts (Ctrl+4): without an identity the sidebar offers Join DashPay in its place.
    if (!contactsWalked) {
      shortcut("ctrl+4");
      await walkAt("Contacts", tid("page-contacts"));
      await contactsAdd();
    }
    // Add contact from a dashpay: link, forwarded by a second launch (smoke.ts).
    const forward = spawnSync(APP, ["--datadir", walletRoot, `dashpay://user?id=${IDENTITY}&username=bob`], { encoding: "utf8", timeout: 60_000, killSignal: "SIGKILL" });
    if (forward.status !== 0) throw new Error(`the second launch did not hand over its link (status ${forward.status})`);
    await walkAt("contacts: add contact from a link", [tid("add-contact"), tid("add-contact-link-error")]);
    await closeSheet();

    // The toolbar: notifications, and the identity chip (My Profile) when there is an identity.
    await wd.press(tid("toolbar-bell"));
    await poll(TIMEOUT_MS, "the notifications", async () => ((await exists(tid("bell-popover"))) || (await exists(tid("page-notifications"))) ? true : undefined));
    if (await exists(tid("bell-popover"))) {
      await walkAt("notifications (bell)", tid("bell-popover"));
      if (await exists(tid("bell-see-all"))) {
        await wd.press(tid("bell-see-all"));
        await walkAt("notifications", tid("page-notifications"));
      }
    } else await walkAt("notifications", tid("page-notifications"));
    if (await exists(tid("toolbar-chip"))) {
      await wd.press(tid("toolbar-chip"));
      await poll(TIMEOUT_MS, "the identity page", async () => ((await exists(tid("page-joinDashPay"))) || (await exists(tid("page-profile"))) ? true : undefined));
    }
    if (await exists(tid("page-profile"))) await walkAt("my profile", tid("page-profile"));
    else skip("my profile");

    // Settings: every tab, and the sheets each opens without funds.
    shortcut("ctrl+comma");
    await wd.waitFor(tid("page-settings"), "Settings");
    const settingsSheets: Record<string, [screen: string, opener: string, dialog: string, more?: string[]][]> = {
      general: [["settings: reset settings", tid("settings-reset"), "settings-reset-sheet"]],
      security: [
        ["settings: change passphrase", tid("settings-change-passphrase"), "settings-change-sheet"],
        ["settings: payment authentication", tid("settings-payment-auth"), "settings-payment-auth-sheet"],
        ["settings: show recovery phrase", tid("settings-security-recovery-phrase"), "recovery-phrase"],
        ["settings: wipe", tid("settings-wipe"), "settings-wipe-sheet"],
      ],
      wallets: [
        ["settings: rename wallet", '[data-testid^="settings-wallet-"][data-testid$="-rename"]', "settings-rename-sheet"],
        ["settings: extended public key", '[data-testid^="settings-wallet-"][data-testid$="-xpub"]', "settings-xpub-sheet", [tid("settings-xpub-qr")]],
        ["settings: add watch-only wallet", tid("settings-add-watch-only"), "settings-watch-only-sheet"],
      ],
      coinjoin: [["settings: restore CoinJoin defaults", tid("settings-coinjoin-restore"), "settings-coinjoin-restore-sheet"]],
      backups: [
        ["settings: back up wallet", tid("settings-backup"), "settings-backup-sheet"],
        ["settings: export keys", tid("settings-export"), "settings-export-sheet"],
        ["settings: import keys", tid("settings-import-keys"), "settings-import-keys-sheet"],
        ["settings: import file", tid("settings-import-file"), "settings-import-file-sheet"],
      ],
    };
    const settingsTabs = {
      general: "settings-display",
      security: "settings-encryption",
      wallets: "settings-wallet",
      coinjoin: "settings-coinjoin",
      backups: "settings-backup-card",
      advanced: "identities-page",
      about: "settings-about",
    };
    await eachTab("settings-tabs", "settings", settingsTabs, async (tab) => {
      for (const [screen, opener, dialog, more] of settingsSheets[tab] ?? []) await sheet(screen, opener, dialog, more);
    });

    // Tools: every tab.
    shortcut("ctrl+shift+i");
    await wd.waitFor(tid("page-tools"), "Tools");
    await eachTab("tools-tabs", "tools", {
      information: "tools-information",
      console: "tools-console",
      peers: "tools-peers",
      repair: "tools-repair",
      addressBook: "address-book-search",
      signVerify: "tools-signVerify",
      psbt: "tools-psbt",
      coinSelection: "tools-coinSelection",
    });

    // Masternodes, once Settings ▸ General shows them: each tab and Create proposal.
    shortcut("ctrl+comma");
    await wd.press(tid("settings-tabs-general"));
    await wd.press(tid("settings-show-masternodes-on"));
    // Governance data is off on regtest: the setting asks whether to load it; no, stay offline.
    await walkAt("settings: load governance data?", tid("settings-governance-ask"));
    await closeSheet();
    await wd.press(tid("sidebar-masternodes"));
    await wd.waitFor(tid("page-masternodes"), "Masternodes");
    await eachTab(
      "masternodes-tabs",
      "masternodes",
      // Offline on regtest the governance data cannot be read.
      { myMasternodes: "mn-empty", governance: ["governance-error", "governance-disabled", "governance-clock"], names: ["names-empty", "names-error"] },
      async (tab) => {
        if (tab === "governance") await sheet("masternodes: create proposal", tid("governance-create"), "create-sheet");
      },
    );

    // Shell dialogs: Open URI (Ctrl+O).
    shortcut("ctrl+o");
    await walkAt("open URI", tid("open-uri"));
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
  const unsettled = results.filter((r) => r.problems.length > 0);
  const report = {
    ok: failure === null && unnamed.length === 0 && unsettled.length === 0,
    screens: results.length,
    interactive: results.reduce((n, r) => n + r.interactive, 0),
    unnamed: unnamed.length,
    unsettled: unsettled.map((r) => ({ screen: r.screen, problems: r.problems })),
    skipped,
    error: failure ? (failure instanceof Error ? failure.message : String(failure)) : null,
    results,
  };
  writeFileSync(join(OUT, "atspi-unnamed.json"), `${JSON.stringify(report, null, 2)}\n`);
  const secrets = [restorePhrase, created.join(" "), PASSPHRASE, ONBOARDING_PASSPHRASE].filter((s) => s !== "");
  // Only this run's directory: every file in it is one the run wrote, so a leak there is deleted.
  const found = leaks(OUT, secrets);
  if (found.length > 0) {
    const kept = removeOwned(found, [canonical(OUT)]);
    throw new Error(`secrets in plain text: ${found.join(", ")}${kept.length > 0 ? `; not deleted (not this run's files): ${kept.join(", ")}` : " (deleted)"}`);
  }

  console.log(`atspi: ${results.length} screens, ${report.interactive} interactive nodes checked, ${unnamed.length} unnamed`);
  for (const s of skipped) console.log(`atspi: skipped ${s.screen}: ${s.why}`);
  for (const r of unsettled) console.error(`atspi: ${r.screen} does not count: ${r.problems.join("; ")}`);
  if (unnamed.length > 0) {
    const rows = unnamed.map((u) => [u.screen, u.role, pathText(u), nearText(u)]);
    const head = ["screen", "role", "path", "nearby"];
    const width = head.map((h, i) => Math.min(70, Math.max(h.length, ...rows.map((r) => r[i]!.length))));
    const line = (cells: string[]) => cells.map((c, i) => (i < 3 ? c.slice(0, width[i]).padEnd(width[i]!) : c)).join(" │ ");
    console.error(line(head));
    for (const r of rows) console.error(line(r));
  }
  if (failure) throw failure;
  if (unsettled.length > 0) throw new Error(`${unsettled.length} screens the tree did not catch up with (${join(OUT, "atspi-unnamed.json")})`);
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
