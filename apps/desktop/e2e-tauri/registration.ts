// A username registered from the wizard, closed mid-flow and resumed, on the real app, the real
// engine and public testnet (DP1-09 GL tier, D17): the app creates a new wallet E (onboarding),
// dwcli funds it from fixture wallet A's Core coins, then the wizard registers a non-contested
// username. Once the row has its asset lock's txid (fundingSent or proofWaiting: before the
// identity is confirmed) the app is closed. The relaunch opens locked; Home says the registration
// is paused (nothing resumes alone, D12); Continue under the passphrase resumes the same row and
// the same lock, and the run ends on Hello @name. The engine's rows say one registration, one
// txid; E's balance fell by one lock, not two. Writes: one payment A→E, one asset lock, one
// identity and its name.
//
//   DW_APP=… DWCLI=… TAURI_DRIVER=… DW_TESTNET_FIXTURE=<dir> node e2e-tauri/registration.ts
//
// E's phrase is written to `<dir>/E.mnemonic` (0600, exclusively) as soon as the app shows it,
// before any coin moves. The run is recorded in `<dir>/DP1-09.json` (written exclusively, then
// at each step), and the app's data root and passphrase stay in DW_REG_STATE (0700) so that a run
// that stopped after the payment goes on where it stopped: a rerun never creates a second wallet,
// never pays twice and never starts a second registration. Phrases go to dwcli on stdin, never
// argv; whatever happens, no file of OUT (nor of DW_E2E_SCAN_EXTRA, the run's log directory) may
// hold a phrase or the passphrase afterwards. `scripts/e2e-tauri-registration.sh` builds the
// binaries and runs this file under a private session bus, on the private X display in $DISPLAY.

import { spawnSync, type ChildProcess } from "node:child_process";
import { randomBytes, randomInt } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import type { DashPayStatus } from "../src/api/generated/DashPayStatus";
import type { RegistrationQuote } from "../src/api/generated/RegistrationQuote";
import type { RegistrationStatus } from "../src/api/generated/RegistrationStatus";
import type { SyncStatus } from "../src/api/generated/SyncStatus";
import type { WalletSummary } from "../src/api/generated/WalletSummary";
import { APP, driverGone, DWCLI, env, leaks, poll, startDriver, stopDriver, tid, TIMEOUT_MS, WebDriver } from "./driver.ts";

const FIXTURE = resolve(env("DW_TESTNET_FIXTURE"));
const STATE = resolve(env("DW_REG_STATE"));
const FUNDER = env("DW_REG_FUNDER", "A");
const FUNDER_BIRTH = env("DW_REG_FUNDER_BIRTH_HEIGHT", "1568806");
/** What A pays E: the registration (about 0.03 tDASH) and room for its fees. */
const FUND_DUFFS = Number(env("DW_REG_FUND_DUFFS", "10000000"));
const OUT = resolve(env("DW_E2E_OUT", "test-results/tauri-registration"));
const PORT = Number(env("DW_E2E_PORT", "4776"));
const SCAN_EXTRA = process.env.DW_E2E_SCAN_EXTRA ? resolve(process.env.DW_E2E_SCAN_EXTRA) : null;
/** SPV from the wallet's birth to the tip, and the masternode lists (BL-162: writes wait for them). */
const SYNC_TIMEOUT_MS = Number(env("DW_E2E_SYNC_TIMEOUT_MS", "1800000"));
/** The payment to E until the quote can spend it. */
const FUNDS_TIMEOUT_MS = Number(env("DW_E2E_FUNDS_TIMEOUT_MS", "1800000"));
/** The asset lock, the identity, the name: Platform state transitions and their proofs. */
const REGISTRATION_TIMEOUT_MS = Number(env("DW_E2E_REGISTRATION_TIMEOUT_MS", "1200000"));
/** How long the screen may still read paused after Continue (a read or two behind the resume). */
const PAUSED_AFTER_CONTINUE_MS = 60_000;
const LEDGER = join(FIXTURE, "DP1-09.json");
const MNEMONIC = join(FIXTURE, "E.mnemonic");
const ROOT = join(STATE, "app");
const FUNDER_ROOT = join(STATE, "funder");
const PASS_FILE = join(STATE, "passphrase");
const NETWORK = ["--network", "testnet"];

// WebDriver key codes (W3C "keyboard actions").
const CONTROL = "";
const BACKSPACE = "";

let driver: ChildProcess | undefined;
/** What no file of OUT may hold: each phrase (as read and single-spaced) and the passphrase. */
const secrets: string[] = [];

/** `text` with every word of a phrase and the passphrase masked, wherever they stand. */
function mask(text: string): string {
  const words = [...new Set(secrets.flatMap((s) => s.split(/\s+/)))].filter(Boolean).sort((a, b) => b.length - a.length);
  if (!words.length) return text;
  // Whole words only, as bell-accept.ts.
  const alternation = words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|");
  return text.replace(new RegExp(`(?<![\\w-])(?:${alternation})(?![\\w-])`, "g"), "***");
}

const addPhrase = (phrase: string) => secrets.push(phrase, phrase.split(/\s+/).join(" "));

/** dwcli on a testnet data root; its stdout (no secrets). Its output is shown masked when it fails. */
function dwcli(root: string, args: string[], input?: string): string {
  const r = spawnSync(DWCLI, ["--datadir", root, ...NETWORK, "--passphrase-file", PASS_FILE, ...args], { encoding: "utf8", input, timeout: 900_000 });
  if (r.status === 0) return r.stdout;
  throw new Error(`dwcli ${args.slice(0, 2).join(" ")} failed (${r.status}): ${mask(`${r.stderr}${r.stdout}`).slice(-800)}`);
}

interface Ledger {
  state: "creating" | "created" | "funding" | "funded" | "registering" | "closed" | "done";
  /** E's wallet id and birth height, once created. */
  walletId?: string;
  birthHeight?: number | null;
  address?: string;
  fundTxid?: string;
  label?: string;
  draft?: string;
  /** The asset lock's txid when the app was closed, and the row's phase then. */
  lockTxid?: string;
  closedAt?: { phase: string; waiting: string | null };
  identity?: string;
  balances?: { beforeRegistration?: number; after?: number; total?: number };
  error?: string;
  steps: string[];
  at: string;
}

let ledger: Ledger | undefined;

function save(patch: Partial<Ledger>, exclusive = false) {
  ledger = { ...(ledger ?? { state: "creating", steps: [], at: new Date().toISOString() }), ...patch };
  writeFileSync(LEDGER, JSON.stringify(ledger, null, 2), { flag: exclusive ? "wx" : "w", mode: 0o600 });
}

function step(s: string) {
  console.log(`registration: ${s}`);
  if (ledger) save({ steps: [...ledger.steps, `${new Date().toISOString()} ${s}`] });
}

/** The app, through tauri-driver; `launch` starts it on ROOT. */
function app(wd: WebDriver) {
  const launch = () =>
    wd.start({ alwaysMatch: { "tauri:options": { application: APP, args: ["--datadir", ROOT, ...NETWORK, "--sync", "--theme", "light"] } } });
  const texts = (css: string) => wd.script<string[]>(`return [...document.querySelectorAll(arguments[0])].map((e) => e.textContent.trim())`, [css]);
  const attr = (css: string, name: string) => wd.script<string | null>(`return document.querySelector(arguments[0])?.getAttribute(arguments[1]) ?? null`, [css, name]);
  const retype = async (id: string, text: string) => {
    const el = await wd.waitFor(tid(id));
    await wd.type(el, `${CONTROL}a${CONTROL}${BACKSPACE}${text}`);
  };
  /** Types into a field and checks by length that it took every key (onboarding.ts). */
  const enter = async (id: string, text: string) => {
    const lengths: number[] = [];
    for (let i = 0; i < 3; i++) {
      if (i === 0) await wd.fill(tid(id), text);
      else await retype(id, text);
      const length = await wd.script<number>(`return document.querySelector(arguments[0])?.value.length ?? -1`, [tid(id)]);
      if (length === text.length) return;
      lengths.push(length);
    }
    throw new Error(`${id} took ${lengths.join(", ")} characters, not ${text.length}`);
  };
  const clickText = async (css: string, text: string) => {
    for (const el of await wd.findAll(css)) if ((await wd.text(el)).trim() === text) return void (await wd.click(el));
    throw new Error(`no ${css} reads the expected text`);
  };
  /** The real engine through the app's own IPC: its answer, or its error's code. */
  const invoke = async <T>(command: string, args: Record<string, unknown> = {}): Promise<T> => {
    const r = await wd.scriptAsync<{ ok?: T; err?: string }>(
      `const done = arguments[arguments.length - 1];
       window.__TAURI_INTERNALS__.invoke(arguments[0], arguments[1]).then((ok) => done({ ok }), (e) => done({ err: e?.code ?? String(e) }));`,
      [command, args],
    );
    if (r.err !== undefined) throw new Error(`${command}: ${r.err}`);
    return r.ok as T;
  };
  return { launch, texts, attr, retype, enter, clickText, invoke };
}

type App = ReturnType<typeof app>;

/** Onboarding ▸ Create on the fresh root: the passphrase, the phrase (kept in E.mnemonic before
 * anything else happens; never screenshotted), verify, Open wallet. */
async function createWallet(wd: WebDriver, a: App, passphrase: string): Promise<WalletSummary> {
  await wd.waitFor(tid("onboarding-welcome"), "Welcome on a fresh data root");
  if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");
  await wd.press(tid("onboarding-create"));
  await a.enter("onboarding-passphrase-new", passphrase);
  await a.enter("onboarding-passphrase-repeat", passphrase);
  await wd.press(tid("onboarding-passphrase-submit"));
  await wd.waitFor(tid("onboarding-phrase-words"), "the recovery phrase");
  const words = await a.texts(`${tid("onboarding-phrase-words")} .phrase-word`);
  if (words.length !== 12) throw new Error(`the phrase step shows ${words.length} words`);
  addPhrase(words.join(" "));
  // Exclusive: never over another wallet's phrase.
  writeFileSync(MNEMONIC, `${words.join(" ")}\n`, { flag: "wx", mode: 0o600 });
  await wd.press(tid("onboarding-written-down"));
  await wd.press(tid("onboarding-phrase-continue"));
  await wd.waitFor(tid("onboarding-verifyPhrase"), "the verify step");
  const positions = (await a.texts(`${tid("verify-slot")} .verify-slot-label`)).map((l) => Number(l.replace("Word ", "")) - 1);
  for (const p of positions) await a.clickText(tid("verify-chip"), words[p]!);
  await wd.waitFor(tid("onboarding-done"), "Done after the last chip");
  await wd.press(tid("onboarding-open"));
  await wd.waitFor(tid("hero"), "the Overview of wallet E");
  const wallets = await a.invoke<WalletSummary[]>("list_wallets");
  if (wallets.length !== 1) throw new Error(`the fresh root lists ${wallets.length} wallets`);
  return wallets[0]!;
}

/** The passphrase sheet a grant asks with. */
async function grantPassphrase(wd: WebDriver, a: App, passphrase: string) {
  await a.enter("grant-sheet-field", passphrase);
  await wd.press(tid("grant-sheet-submit"));
}

async function unlock(wd: WebDriver, a: App, passphrase: string) {
  await wd.waitFor(tid("lock-field"), "the lock screen");
  if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");
  await a.enter("lock-field", passphrase);
  await wd.press(tid("lock-submit"));
  await wd.waitGone(tid("lock-screen"), "the lock screen");
}

/** SPV at the tip with the masternode lists (BL-162); the tip height. */
async function caughtUp(a: App): Promise<number> {
  const s = await poll(SYNC_TIMEOUT_MS, "SPV to catch up", async () => {
    const s = await a.invoke<SyncStatus>("sync_status");
    return s.caughtUp && s.tipHeight !== null ? s : undefined;
  });
  return s.tipHeight!;
}

const balanceOf = async (a: App, walletId: string) => {
  const w = (await a.invoke<WalletSummary[]>("list_wallets")).find((w) => w.walletId === walletId);
  return w?.balances ? w.balances.confirmed + w.balances.unconfirmed : null;
};

/** The one payment A→E (written as "funding" before it is made: a rerun never pays again). */
function fund(address: string, tip: number): string {
  if (!existsSync(FUNDER_ROOT)) {
    const phrase = readFileSync(join(FIXTURE, `${FUNDER}.mnemonic`), "utf8").trim();
    mkdirSync(FUNDER_ROOT, { mode: 0o700 });
    dwcli(FUNDER_ROOT, ["init-vault"]);
    dwcli(FUNDER_ROOT, ["import", "--birth-height", FUNDER_BIRTH, "--name", `Fixture ${FUNDER}`], `${phrase}\n`);
  }
  const funder = /^(\S+) name=/m.exec(dwcli(FUNDER_ROOT, ["list"]))?.[1];
  if (!funder) throw new Error(`dwcli list shows no wallet ${FUNDER}`);
  // dwcli waits 180 s for the height; a slow catch-up is resumed from its data root. Planning
  // starts only after the wait, so a retry never pays twice.
  for (let attempt = 1; ; attempt++) {
    const r = spawnSync(
      DWCLI,
      ["--datadir", FUNDER_ROOT, ...NETWORK, "--passphrase-file", PASS_FILE, "send", funder, "--to", `${address}:${FUND_DUFFS}`, "--sync-height", String(tip)],
      { encoding: "utf8", timeout: 900_000 },
    );
    const out = `${r.stderr}${r.stdout}`;
    if (r.status === 0) {
      const txid = /^broadcast (\S+)$/m.exec(r.stdout)?.[1];
      if (!txid) throw new Error(`dwcli send printed no broadcast txid: ${mask(out).slice(-400)}`);
      return txid;
    }
    if (!/did not reach/.test(out) || attempt >= 10) throw new Error(`dwcli send failed (${r.status}): ${mask(out).slice(-800)}`);
    step(`${FUNDER} still catching up to ${tip} (attempt ${attempt})`);
  }
}

/** A non-contested label (a digit 2–9, DIP-23) nobody holds yet. */
const newLabel = () => `dwdp109e${Array.from({ length: 4 }, () => randomInt(2, 10)).join("")}`;

const rowsOf = (a: App, walletId: string) => a.invoke<RegistrationStatus[]>("registrations", { walletId });

/** Home's card, then the wizard: the name (available), Confirm & pay, the passphrase; then the
 * app is closed as soon as the row has its lock's txid. */
async function registerAndClose(wd: WebDriver, a: App, walletId: string, passphrase: string) {
  // The card offers Join once DashPay knows E has no identity (BL-153).
  await poll(SYNC_TIMEOUT_MS, "Home's card to offer Join DashPay", async () => ((await a.attr(tid("dashpay-card"), "data-state")) === "join" ? true : undefined));
  await wd.press(`${tid("dashpay-card")} ${tid("dashpay-card-action")}`);
  await wd.waitFor(tid("page-joinDashPay"), "the Join page");
  await poll(TIMEOUT_MS, "Create username to be enabled", async () =>
    (await wd.script<boolean>(`return document.querySelector(arguments[0])?.disabled === false`, [tid("join-create")])) ? true : undefined,
  );
  await wd.screenshot("1-join");
  await wd.press(tid("join-create"));
  let label = "";
  for (let i = 0; i < 5 && !label; i++) {
    const candidate = newLabel();
    await a.retype("registration-name-field", candidate);
    const typed = await wd.script<string>(`return document.querySelector(arguments[0])?.value ?? ""`, [tid("registration-name-field")]);
    if (typed !== candidate) continue;
    const line = await poll(TIMEOUT_MS, "the availability answer", async () => {
      const tone = await a.attr(tid("registration-availability"), "data-tone");
      return tone && tone !== "pending" ? tone : undefined;
    });
    if (line === "ok") label = candidate;
  }
  if (!label) throw new Error("five labels in a row were not available");
  if (await wd.find(tid("registration-contested"))) throw new Error(`${label} is contested`);
  save({ label });
  await wd.screenshot("2-name");
  await wd.press(tid("registration-next"));
  const fee = await wd.text(await wd.waitFor(tid("registration-fee"), "the cost"));
  if (await wd.find(tid("registration-shortfall"))) throw new Error("step 2 shows a shortfall");
  await wd.screenshot("3-confirm");
  step(`step 2: @${label} costs ${fee}`);
  save({ state: "registering" });
  await wd.press(tid("registration-pay"));
  await grantPassphrase(wd, a, passphrase);
  const row = await poll(REGISTRATION_TIMEOUT_MS, "the asset lock's txid", async () => {
    const r = (await rowsOf(a, walletId)).find((r) => r.label === label);
    if (r?.failure) throw new Error(`the registration failed: ${r.failure.code} at ${r.failure.phase}`);
    return r?.txid ? r : undefined;
  });
  if (row.phase !== "fundingSent" && row.phase !== "proofWaiting") throw new Error(`the row reached ${row.phase} before the app could be closed`);
  // A run shows progress, whether or not its lease holds a key of its own (review r2 H1, D26).
  const kind = await a.attr(tid("registration-screen"), "data-kind");
  // Close first, look later: every step here is time the identity has to confirm.
  await wd.end();
  save({ state: "closed", draft: row.draft, lockTxid: row.txid!, closedAt: { phase: row.phase, waiting: row.waiting } });
  step(`closed the app at ${row.phase} (waiting ${row.waiting}, running ${row.running}, screen ${kind}), lock ${row.txid}`);
  // After the save: the row is resumable either way, so a rerun goes on from closed.
  if (kind !== "progress" || !row.running) throw new Error(`the running registration showed ${kind} (running ${row.running}, waiting ${row.waiting})`);
}

/** The relaunch: locked, Home's card paused, Continue under the passphrase, Hello @name. */
async function resume(wd: WebDriver, a: App, walletId: string, passphrase: string) {
  const { draft, lockTxid, label } = ledger!;
  await unlock(wd, a, passphrase);
  const after = await rowsOf(a, walletId);
  if (after.length !== 1 || after[0]!.draft !== draft || after[0]!.txid !== lockTxid)
    throw new Error(`after the relaunch the engine lists ${after.map((r) => `${r.draft} ${r.phase} ${r.txid}`).join("; ") || "no rows"}`);
  const parked = after[0]!;
  step(`relaunched: the row is ${parked.phase}, waiting ${parked.waiting}, running ${parked.running}`);
  // Nothing resumes alone (D12): Home says paused until Continue (D21, D26: nothing runs the row).
  let state: string | null = null;
  await poll(TIMEOUT_MS, "Home's card to say paused", async () => {
    state = await a.attr(tid("dashpay-card"), "data-state");
    return state === "creatingPaused" ? true : undefined;
  }).catch(() => {
    throw new Error(`Home's card after the relaunch is ${state}, not creatingPaused`);
  });
  await wd.screenshot("4-relaunched-home");
  await wd.press(`${tid("dashpay-card")} ${tid("dashpay-card-action")}`);
  await poll(TIMEOUT_MS, "the paused screen", async () => ((await a.attr(tid("registration-screen"), "data-kind")) === "paused" ? true : undefined));
  await wd.screenshot("5-paused");
  await wd.press(tid("registration-continue"));
  await grantPassphrase(wd, a, passphrase);
  await wd.waitGone(tid("grant-sheet"), "the passphrase sheet");
  step("Continue: passphrase given");
  // Continue's run shows progress: a paused screen that outlasts a few polls is the D26 bug.
  let pausedSince: number | null = null;
  const end = await poll(REGISTRATION_TIMEOUT_MS, "the registration to end", async () => {
    const kind = await a.attr(tid("registration-screen"), "data-kind");
    pausedSince = kind === "paused" ? (pausedSince ?? Date.now()) : null;
    if (pausedSince !== null && Date.now() - pausedSince > PAUSED_AFTER_CONTINUE_MS) throw new Error("the screen stayed paused after Continue");
    return kind && kind !== "progress" && kind !== "paused" ? kind : undefined;
  });
  await wd.screenshot("6-end");
  const rows = await rowsOf(a, walletId);
  if (end !== "done") throw new Error(`the registration ended on ${end}: ${JSON.stringify(rows.map((r) => ({ phase: r.phase, failure: r.failure, nameFailure: r.nameFailure })))}`);
  const title = await wd.text(await wd.waitFor(`${tid("registration-screen")} h2`));
  if (title !== `Hello @${label},`) throw new Error(`the done screen says ${title}`);
  if (rows.length !== 1 || rows[0]!.draft !== draft || rows[0]!.txid !== lockTxid) throw new Error("the finished row is not the one closed");
  const status = await a.invoke<DashPayStatus>("dashpay_status", { walletId });
  if (status.kind !== "ready") throw new Error(`DashPay status after the run: ${status.kind}`);
  save({ identity: status.main });
  step(`done: Hello @${label}, identity ${status.main}, one row, lock ${lockTxid}`);
}

async function main() {
  mkdirSync(OUT, { recursive: true });
  mkdirSync(STATE, { recursive: true, mode: 0o700 });
  if (existsSync(LEDGER)) {
    ledger = JSON.parse(readFileSync(LEDGER, "utf8")) as Ledger;
    if (ledger.state === "done") throw new Error(`${LEDGER} says done: wallet E is registered; a new run needs the ledger, E.mnemonic and ${STATE} moved aside`);
    if (ledger.state === "creating" || ledger.state === "funding" || ledger.state === "registering")
      throw new Error(`${LEDGER} says ${ledger.state}: a wallet, a payment or a lock may exist; look before running again`);
    if (!existsSync(PASS_FILE) || !existsSync(ROOT) || !existsSync(MNEMONIC)) throw new Error(`${LEDGER} says ${ledger.state} but ${STATE} or E.mnemonic is gone`);
    step(`going on from ${ledger.state}`);
  } else {
    if (existsSync(MNEMONIC)) throw new Error(`${MNEMONIC} exists without a ledger: pick another fixture or move it aside`);
    // Checked before the ledger is written, so a leftover never leaves a ledger that blocks reruns.
    for (const left of [ROOT, FUNDER_ROOT, PASS_FILE]) if (existsSync(left)) throw new Error(`${left} exists without a ledger: move it aside`);
    save({ state: "creating" }, true);
    writeFileSync(PASS_FILE, `${randomBytes(18).toString("base64url")}\n`, { flag: "wx", mode: 0o600 });
  }
  const passphrase = readFileSync(PASS_FILE, "utf8").trim();
  secrets.push(passphrase);
  addPhrase(readFileSync(join(FIXTURE, `${FUNDER}.mnemonic`), "utf8").trim());
  if (existsSync(MNEMONIC)) addPhrase(readFileSync(MNEMONIC, "utf8").trim());

  let failure: unknown;
  const started = await startDriver(PORT, OUT, (d) => (driver = d));
  driver = started.driver;
  const wd = new WebDriver(started.base, OUT);
  const a = app(wd);
  try {
    await a.launch();
    if (ledger!.state === "creating") {
      const e = await createWallet(wd, a, passphrase);
      save({ state: "created", walletId: e.walletId, birthHeight: e.birthHeight });
      step(`created wallet E ${e.walletId} (birth ${e.birthHeight}); phrase in E.mnemonic`);
    } else {
      await unlock(wd, a, passphrase);
    }
    const walletId = ledger!.walletId!;
    if (ledger!.state === "created") {
      const tip = await caughtUp(a);
      const address = (await a.invoke<{ address: string }>("current_receive_address", { walletId })).address;
      save({ state: "funding", address });
      step(`SPV at ${tip}; paying ${FUND_DUFFS} duffs from ${FUNDER} to ${address}`);
      const fundTxid = fund(address, tip);
      save({ state: "funded", fundTxid });
      step(`${FUNDER} paid E: ${fundTxid}`);
    }
    if (ledger!.state === "funded") {
      await caughtUp(a);
      const quote = await poll(FUNDS_TIMEOUT_MS, "the payment to be spendable", async () => {
        const q = await a.invoke<RegistrationQuote>("registration_quote", { walletId, request: { label: "dwdp109quote2", temporaryLabel: null, funding: { kind: "coreBalance" }, initialProfile: null } });
        return q.availableDuffs >= q.totalDuffs ? q : undefined;
      });
      const before = await balanceOf(a, walletId);
      save({ balances: { beforeRegistration: before ?? undefined, total: quote.totalDuffs } });
      step(`spendable ${quote.availableDuffs} ≥ total ${quote.totalDuffs} (lock ${quote.lockDuffs}); balance ${before}`);
      await registerAndClose(wd, a, walletId, passphrase);
      await a.launch();
    }
    if (ledger!.state === "closed") {
      await resume(wd, a, walletId, passphrase);
      const after = await balanceOf(a, walletId);
      const { beforeRegistration, total } = ledger!.balances ?? {};
      save({ balances: { ...ledger!.balances, after: after ?? undefined } });
      if (beforeRegistration !== undefined && after !== null && total !== undefined) {
        const spent = beforeRegistration - after;
        // One lock and its fee, never two locks.
        if (spent < total - 10_000 || spent >= 2 * (total - 10_000)) throw new Error(`E spent ${spent} duffs for one registration of ${total}`);
        step(`E spent ${spent} duffs (quote ${total}): one asset lock`);
      }
      save({ state: "done" });
    }
  } catch (e) {
    failure = e;
    // Never of a phrase screen (onboarding.ts): checked before and after.
    const onPhrase = () => wd.script<boolean>(`return !!document.querySelector('.phrase-grid, .verify-chips, .phrase-input')`).catch(() => true);
    if (!(await onPhrase())) {
      await wd.screenshot("failure").catch(() => undefined);
      if (await onPhrase()) rmSync(join(OUT, "failure.png"), { force: true });
    }
  } finally {
    await wd.end().catch(() => undefined);
    stopDriver(driver);
    await driverGone(driver);
    driver = undefined;
  }
  const found = [...leaks(OUT, secrets), ...(SCAN_EXTRA ? leaks(SCAN_EXTRA, secrets) : []), ...leaks(FIXTURE, secrets).filter((f) => !/\.mnemonic \(\d+\)$/.test(f))];
  if (found.length > 0) {
    for (const f of found) if (!f.startsWith(FIXTURE)) rmSync(f.replace(/ \(\d+\)$/, ""), { force: true });
    const leak = `secrets in plain text: ${found.join(", ")}`;
    throw new Error(failure ? `${failure instanceof Error ? failure.message : String(failure)}; ${leak}` : leak);
  }
  if (failure) throw failure;
  step(`no phrase or passphrase in OUT${SCAN_EXTRA ? ", the run's logs" : ""} or the fixture's other files`);
  writeFileSync(join(OUT, "result.json"), JSON.stringify({ ok: true, ...ledger }, null, 2));
  console.log(`registration: PASS; screenshots in ${OUT}`);
}

/** Interrupted (Ctrl-C, an outer timeout): stop the app and wait for it, and delete any file of OUT
 * holding a secret, as a normal exit does. The state stays for the next run. */
async function interrupted(signal: NodeJS.Signals) {
  console.error(`registration: FAIL: ${signal}`);
  stopDriver(driver);
  await driverGone(driver);
  for (const f of [...leaks(OUT, secrets), ...(SCAN_EXTRA ? leaks(SCAN_EXTRA, secrets) : [])]) rmSync(f.replace(/ \(\d+\)$/, ""), { force: true });
  process.exit(1);
}
process.once("SIGINT", (s) => void interrupted(s));
process.once("SIGTERM", (s) => void interrupted(s));

main()
  .catch((e) => {
    console.error(`registration: FAIL: ${mask(e instanceof Error ? e.message : String(e))}`);
    // The state stays: a run that stopped in creating, funding or registering waits for a look,
    // one that stopped in created, funded or closed goes on.
    if (ledger && ledger.state !== "done") save({ error: mask(e instanceof Error ? e.message : String(e)) });
    process.exitCode = 1;
  })
  .finally(() => stopDriver(driver));
