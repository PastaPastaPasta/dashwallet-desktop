// A payment to a contact from Pay ▸ To a contact, on the real app, the real engine and public
// testnet (DP3-06 GL tier): tauri-driver drives the production renderer in WebKitGTK with a wallet
// of the testnet DashPay fixture restored (`--sync`), picks the contact from the frequent strip (the
// picker when the strip does not list it), pays it under the confirm sheet's passphrase, and opens
// the payment in Activity, attributed to the contact. dwcli reads the payee's DashPay activity
// with the payer before and after: the payment arrives on its receival account. Writes: the one
// payment.
//
//   DW_APP=… DWCLI=… TAURI_DRIVER=… DW_TESTNET_FIXTURE=<dir> node e2e-tauri/pay-contact.ts
//
// <dir> holds `<W>.mnemonic` (0600) and `fixture.json` (the identities). DW_PAY_PAYER and
// DW_PAY_PAYEE pick the wallets (default A and B, contacts since the fixture's set-up), and
// DW_PAY_USERNAME the payee's username (default dwrecoverb2, B's). The payment is recorded in
// `<dir>/DP3-06.json`, written exclusively before Send is pressed: a rerun never pays again, it
// checks the payment already sent. Phrases go to dwcli on stdin, never argv; whatever happens, no
// file of OUT (nor of DW_E2E_SCAN_EXTRA, the run's log directory) may hold a phrase or the
// passphrase afterwards. `scripts/e2e-tauri-pay-contact.sh` builds the binaries and runs this file
// under a private session bus, on the private X display in $DISPLAY.

import { spawnSync, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { DWCLI, driverGone, env, leaks, poll, startDriver, stopDriver, tid, TIMEOUT_MS, WebDriver, APP } from "./driver.ts";

const FIXTURE = resolve(env("DW_TESTNET_FIXTURE"));
const PAYER = env("DW_PAY_PAYER", "A");
const PAYEE = env("DW_PAY_PAYEE", "B");
const USERNAME = env("DW_PAY_USERNAME", "dwrecoverb2");
const BIRTH_HEIGHT = env("DW_PAY_BIRTH_HEIGHT", "1568806");
/** The payment, in DASH as typed, and in duffs. */
const AMOUNT = env("DW_PAY_AMOUNT", "0.001");
const DUFFS = Math.round(Number(AMOUNT) * 1e8);
const NOTE = "DP3-06 GL";
const OUT = resolve(env("DW_E2E_OUT", "test-results/tauri-pay-contact"));
const PORT = Number(env("DW_E2E_PORT", "4786"));
const SCAN_EXTRA = process.env.DW_E2E_SCAN_EXTRA ? resolve(process.env.DW_E2E_SCAN_EXTRA) : null;
/** DashPay's bring-up on a fresh engine and a pass: the tab lists the contacts after it. */
const DASHPAY_TIMEOUT_MS = Number(env("DW_E2E_DASHPAY_TIMEOUT_MS", "1200000"));
/** The wallet's sync to a spendable balance, then the broadcast and its InstantSend lock. */
const SEND_TIMEOUT_MS = Number(env("DW_E2E_SEND_TIMEOUT_MS", "900000"));
const LEDGER = join(FIXTURE, "DP3-06.json");
const NETWORK = ["--network", "testnet"];

let driver: ChildProcess | undefined;
/** The data roots and the passphrase file: out of OUT; removed at the end. */
let work: string | undefined;
/** What no file of OUT may hold: each phrase (as read and single-spaced) and the passphrase. */
const secrets: string[] = [];

/** `text` with every word of a phrase and the passphrase masked, wherever they stand. */
function mask(text: string): string {
  const words = [...new Set(secrets.flatMap((s) => s.split(/\s+/)))].filter(Boolean).sort((a, b) => b.length - a.length);
  if (!words.length) return text;
  const alternation = words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|");
  return text.replace(new RegExp(`(?<![\\w-])(?:${alternation})(?![\\w-])`, "g"), "***");
}

/** dwcli on a testnet data root; its stdout (JSON, no secrets), shown masked when it fails. */
function dwcli(root: string, passFile: string, args: string[], input?: string, timeout = 900_000): string {
  const r = spawnSync(DWCLI, ["--datadir", root, ...NETWORK, "--passphrase-file", passFile, ...args], { encoding: "utf8", input, timeout });
  if (r.status === 0) return r.stdout;
  throw new Error(`dwcli ${args.slice(0, 3).join(" ")} failed (${r.status}): ${mask(`${r.stderr}${r.stdout}`).slice(-800)}`);
}

const result = <T>(stdout: string): T => (JSON.parse(stdout) as { result: T }).result;

/** A wallet's identity id: `fixture.json`'s row (a string, or `{identity_id}`). */
function identityOf(wallet: string): string {
  const fixture = JSON.parse(readFileSync(join(FIXTURE, "fixture.json"), "utf8")) as Record<string, string | { identity_id?: string }>;
  const row = fixture[wallet];
  const id = typeof row === "string" ? row : row?.identity_id;
  if (!id) throw new Error(`the fixture names no identity for wallet ${wallet}`);
  return id;
}

/** A restored wallet's data root under `work`, its vault sealed with `passFile`. */
function restore(root: string, passFile: string, wallet: string): void {
  const phrase = readFileSync(join(FIXTURE, `${wallet}.mnemonic`), "utf8").trim();
  dwcli(root, passFile, ["init-vault"]);
  dwcli(root, passFile, ["import", "--birth-height", BIRTH_HEIGHT, "--name", `Fixture ${wallet}`], `${phrase}\n`);
}

interface Item {
  txid: string;
  direction: string;
  amount: number;
}

/** SPV caught up on `root` (DashPay's bring-up runs with it, so the receival accounts are
 * watched), or until `txid` is in its history. `--spv` alone returns before the chain is synced. */
function sync(root: string, passFile: string, txid?: string): void {
  const secs = Math.round(SEND_TIMEOUT_MS / 1000);
  dwcli(root, passFile, ["sync", "--timeout-secs", String(secs), ...(txid ? ["--txid", txid] : [])], undefined, SEND_TIMEOUT_MS + 60_000);
}

/** What `identity` received from `from`, as dwcli reads it after a sync. */
function received(root: string, passFile: string, identity: string, from: string): Item[] {
  const items: Item[] = [];
  let cursor: string | null = null;
  do {
    const page: { items: Item[]; nextCursor?: string | null; next_cursor?: string | null } = result(
      dwcli(root, passFile, ["contact", "--spv", "activity", "--identity", identity, from, "--filter", "received", ...(cursor ? ["--cursor", cursor] : [])]),
    );
    items.push(...page.items);
    cursor = page.nextCursor ?? page.next_cursor ?? null;
  } while (cursor);
  return items;
}

/** The data roots' logs into the run's log directory, scanned for secrets like OUT. */
function keepLogs(roots: Record<string, string>) {
  if (!SCAN_EXTRA) return;
  for (const [name, root] of Object.entries(roots)) {
    const dirs = existsSync(root) ? ["", ...readdirSync(root)].map((d) => join(root, d, "logs")) : [];
    for (const dir of dirs.filter((d) => existsSync(d))) cpSync(dir, join(SCAN_EXTRA, name, dir.slice(root.length + 1)), { recursive: true });
  }
}

interface Ledger {
  payer: string;
  payee: string;
  payerIdentity: string;
  payeeIdentity: string;
  duffs: number;
  state: "sending" | "sent" | "failed";
  txid?: string;
  outcome?: string;
  error?: string;
  at: string;
}

/** The ledger of an earlier run, if any: only a finished payment for this pair is reused. */
function earlier(from: string, to: string): Ledger | null {
  if (!existsSync(LEDGER)) return null;
  const ledger = JSON.parse(readFileSync(LEDGER, "utf8")) as Ledger;
  if (ledger.payerIdentity !== from || ledger.payeeIdentity !== to)
    throw new Error(`${LEDGER} records ${ledger.payer}→${ledger.payee}, not ${PAYER}→${PAYEE}: run that pair, or move the ledger aside`);
  if (ledger.state !== "sent" || !ledger.txid) throw new Error(`${LEDGER} says ${ledger.state}: a payment may have been sent; look before running again`);
  return ledger;
}

async function run(
  passphrase: string,
  payee: string,
  wd: WebDriver,
  ledgerIn: Ledger | null,
  writeLedger: (l: Ledger) => void,
  base: Omit<Ledger, "state" | "at">,
  step: (s: string) => void,
): Promise<{ out: Record<string, unknown>; ledger: Ledger }> {
  const out: Record<string, unknown> = {};
  const textOf = (css: string) => wd.script<string>(`return document.querySelector('${css}')?.innerText.replace(/\\s+/g, " ").trim() ?? ""`);
  const disabled = (id: string) => wd.script<boolean>(`return !!document.querySelector('${tid(id)}')?.disabled`);
  await wd.fill(tid("lock-field"), passphrase);
  await wd.press(tid("lock-submit"));
  await wd.waitGone(tid("lock-screen"), "the lock screen");
  if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");
  step(`unlocked the restored wallet ${PAYER}`);
  // The strip ranks the wallet's history: wait for the chain, as a user returning to the wallet.
  await poll(SEND_TIMEOUT_MS, "the wallet to sync", async () => ((await textOf(tid("status-bar"))).includes("Synced") ? true : undefined));
  step("the status bar says Synced");

  if (ledgerIn) {
    step(`the payment ${PAYER}→${PAYEE} was sent before (${ledgerIn.at}, ${ledgerIn.txid}); not paying again`);
    await wd.press(tid("sidebar-activity"));
    const row = await poll(SEND_TIMEOUT_MS, "the payment in Activity", async () => (await wd.find(`[data-txid="${ledgerIn!.txid}"]`)) ?? undefined);
    await wd.click(row);
  } else {
    await wd.press(tid("sidebar-pay"));
    await wd.press(tid("pay-tabs-contact"));
    // The strip and the picker fill once DashPay's bring-up has run.
    const chip = `${tid("pay-frequent-chip")}[data-contact="${payee}"]`;
    const row = `${tid("pay-picker-row")}[data-contact="${payee}"]`;
    const named = (css: string) => wd.script<boolean>(`return !!document.querySelector('${css}')?.innerText.includes('${USERNAME}')`);
    await poll(DASHPAY_TIMEOUT_MS, `Pay ▸ To a contact to list ${USERNAME}`, async () => ((await named(chip)) || (await named(row)) ? true : undefined));
    // The strip follows the picker once DashPay's payments are read: give it a moment.
    const via = await poll(TIMEOUT_MS, "the frequent strip", async () => ((await named(chip)) ? "the frequent strip" : undefined)).catch(() => "the picker");
    out.frequent = await wd.script<string[]>(`return [...document.querySelectorAll('${tid("pay-frequent-chip")}')].map((c) => c.innerText.replace(/\\s+/g, " ").trim())`);
    out.via = via;
    await wd.screenshot("1-chooser");
    step(`To a contact lists ${USERNAME} in ${via}; strip: ${JSON.stringify(out.frequent)}`);
    await wd.press(via === "the frequent strip" ? chip : row);

    const payeeName = await poll(TIMEOUT_MS, `the payee card to name ${USERNAME}`, async () => {
      const t = await textOf(tid("pay-payee-name"));
      return t.includes(USERNAME) ? t : undefined;
    });
    const banner = await wd.script<{ kind: string; text: string } | null>(
      `const b = document.querySelector('${tid("pay-banner")}'); return b ? { kind: b.dataset.kind, text: b.innerText } : null`,
    );
    out.banner = banner;
    if (banner && banner.kind !== "note") throw new Error(`the form is held back: ${banner.kind}: ${banner.text}`);
    await wd.fill(tid("pay-amount"), AMOUNT);
    await wd.fill(tid("pay-note"), NOTE);
    // Review is offered once the wallet has synced enough to spend and the engine estimated a fee.
    await poll(SEND_TIMEOUT_MS, "the engine's estimate", async () => ((await disabled("pay-continue")) ? undefined : true));
    out.form = await textOf(tid("pay-contact-form"));
    await wd.screenshot("2-form");
    step(`form: ${payeeName}, ${AMOUNT} DASH, note "${NOTE}"${banner ? `, banner note "${banner.text}"` : ""}`);

    await wd.press(tid("pay-continue"));
    const payTo = await poll(TIMEOUT_MS, "the confirm sheet", async () => (await textOf(tid("pay-confirm-contact"))) || undefined);
    if (!payTo.includes(USERNAME)) throw new Error(`the confirm sheet pays ${payTo}, not ${USERNAME}`);
    if (await wd.find(tid("pay-confirm-unsigned"))) throw new Error("the confirm sheet offers an unsigned transaction for a contact");
    out.confirm = await textOf(tid("pay-confirm-facts"));
    await wd.fill(tid("pay-confirm-passphrase"), passphrase);
    await wd.screenshot("3-confirm");
    step(`confirm: ${out.confirm}`);

    // Exclusive: two runs never both pay.
    const ledger: Ledger = { ...base, state: "sending", at: new Date().toISOString() };
    writeLedger(ledger);
    try {
      await wd.press(tid("pay-confirm-send"));
      let kind = await poll(SEND_TIMEOUT_MS, "the payment's result", async () => {
        for (const k of ["sent", "unknown", "failed"]) if (await wd.find(tid(`pay-result-${k}`))) return k;
        const error = await textOf(tid("pay-confirm-error"));
        if (error) throw new Error(`the confirm sheet refused: ${error}`);
        return undefined;
      });
      await wd.screenshot("4-result");
      const retries = () => wd.script<number>(`return Number(document.querySelector('[data-retries]')?.dataset.retries ?? -1)`);
      // "Status unknown" (the network did not answer in time) offers Try again, which broadcasts the
      // same signed transaction again: as a user would, up to three times, never a new payment.
      for (let attempt = 1; kind === "unknown" && attempt <= 3 && (await wd.find(tid("pay-result-retry"))); attempt++) {
        step(`result: status unknown; Try again (${attempt})`);
        await new Promise((r) => setTimeout(r, 30_000));
        // The sheet counts finished retries: read before the click, so a retry that answers unknown
        // at once (its busy state never seen) is still waited for, not missed.
        const before = await retries();
        await wd.press(tid("pay-result-retry"));
        kind = await poll(SEND_TIMEOUT_MS, "the retry's result", async () => {
          for (const k of ["sent", "failed"]) if (await wd.find(tid(`pay-result-${k}`))) return k;
          return (await retries()) > before ? "unknown" : undefined;
        });
        await wd.screenshot(`4-result-retry-${attempt}`);
      }
      ledger.outcome = kind;
      if (kind !== "sent") throw new Error(`the payment's result is ${kind}: ${await textOf(tid("pay-result-text"))}`);
      ledger.txid = (await textOf(tid("pay-result-txid"))).match(/[0-9a-f]{64}/)?.[0];
      if (!ledger.txid) throw new Error("the result sheet shows no transaction id");
      ledger.state = "sent";
      out.note = await textOf(tid("pay-result-note"));
      step(`result: Payment sent, ${ledger.txid}`);
    } catch (e) {
      ledger.state = ledger.state === "sent" ? "sent" : "failed";
      ledger.error = mask(e instanceof Error ? e.message : String(e));
      throw e;
    } finally {
      writeLedger(ledger);
    }
    ledgerIn = ledger;
    await wd.press(tid("pay-result-show"));
  }

  // Activity: the payment's detail, attributed to the contact (DP3-02).
  const attributed = await poll(DASHPAY_TIMEOUT_MS, "the detail sheet to name the contact", async () => {
    const t = await textOf(tid("tx-detail-contact"));
    return t.includes(USERNAME) ? t : undefined;
  });
  out.activity = attributed;
  out.detail = await textOf(tid("tx-detail-facts"));
  await wd.screenshot("5-activity-detail");
  step(`Activity: ${attributed}`);
  return { out, ledger: ledgerIn };
}

async function main() {
  mkdirSync(OUT, { recursive: true });
  work = mkdtempSync(join(tmpdir(), "dw-pay-")); // mode 0700
  const payeeRoot = join(work, "payee");
  const appRoot = join(work, "app");
  const passFile = join(work, "passphrase");
  const passphrase = randomBytes(18).toString("base64url");
  for (const w of [PAYER, PAYEE]) {
    const phrase = readFileSync(join(FIXTURE, `${w}.mnemonic`), "utf8").trim();
    secrets.push(phrase, phrase.split(/\s+/).join(" "));
  }
  secrets.push(passphrase);
  writeFileSync(passFile, `${passphrase}\n`, { mode: 0o600 });
  const steps: string[] = [];
  const step = (s: string) => {
    steps.push(`${new Date().toISOString()} ${s}`);
    console.log(`pay-contact: ${s}`);
  };
  const writeLedger = (l: Ledger) => writeFileSync(LEDGER, JSON.stringify(l, null, 2), { flag: l.state === "sending" ? "wx" : "w", mode: 0o600 });

  const payer = identityOf(PAYER);
  const payee = identityOf(PAYEE);
  let failure: unknown;
  let ledger: Ledger | null = null;
  let res: Record<string, unknown> = {};
  let before: Item[] | undefined;
  let arrived: Item | undefined;
  try {
    ledger = earlier(payer, payee);
    restore(payeeRoot, passFile, PAYEE);
    dwcli(payeeRoot, passFile, ["contact", "--spv", "list", "--identity", payee]);
    sync(payeeRoot, passFile);
    before = received(payeeRoot, passFile, payee, payer);
    step(`payee ${PAYEE} received from ${PAYER} before: ${before.length} payments`);
    restore(appRoot, passFile, PAYER);
    step(`testnet data root with fixture wallet ${PAYER}, birth ${BIRTH_HEIGHT}`);
    const base = { payer: PAYER, payee: PAYEE, payerIdentity: payer, payeeIdentity: payee, duffs: DUFFS };
    const started = await startDriver(PORT, OUT, (d) => (driver = d));
    driver = started.driver;
    const wd = new WebDriver(started.base, OUT);
    try {
      await wd.start({ alwaysMatch: { "tauri:options": { application: APP, args: ["--datadir", appRoot, ...NETWORK, "--sync", "--theme", "light"] } } });
      const r = await run(passphrase, payee, wd, ledger, writeLedger, base, step);
      res = r.out;
      ledger = r.ledger;
    } catch (e) {
      failure = e;
      await wd.screenshot("failure").catch(() => undefined);
    } finally {
      await wd.end().catch(() => undefined);
    }
    // The payee's side: the payment arrives on its receival account for the payer.
    const txid = ledger?.txid;
    if (!failure && txid) {
      sync(payeeRoot, passFile, txid);
      arrived = await poll(DASHPAY_TIMEOUT_MS, `${PAYEE} to receive ${txid}`, async () => received(payeeRoot, passFile, payee, payer).find((i) => i.txid === txid));
      if (Math.abs(arrived.amount) !== DUFFS) throw new Error(`${PAYEE} received ${arrived.amount} duffs in ${txid}, not ${DUFFS}`);
      step(`payee ${PAYEE} received ${arrived.amount} duffs from ${PAYER} in ${txid} (${arrived.direction})`);
    }
  } catch (e) {
    failure ??= e;
  } finally {
    stopDriver(driver);
    await driverGone(driver);
    driver = undefined;
    keepLogs({ app: appRoot, payee: payeeRoot });
  }
  const found = [...leaks(OUT, secrets), ...(SCAN_EXTRA ? leaks(SCAN_EXTRA, secrets) : [])];
  if (found.length > 0) {
    for (const f of found) rmSync(f.replace(/ \(\d+\)$/, ""), { force: true });
    const leak = `secrets in plain text: ${found.join(", ")}`;
    throw new Error(failure ? `${failure instanceof Error ? failure.message : String(failure)}; ${leak}` : leak);
  }
  if (failure) throw failure;
  step(`no phrase or passphrase in OUT${SCAN_EXTRA ? " or the run's logs" : ""}`);
  writeFileSync(
    join(OUT, "result.json"),
    JSON.stringify(
      { ok: true, payer: { wallet: PAYER, identity: payer }, payee: { wallet: PAYEE, identity: payee, username: USERNAME }, ledger, ...res, receivedBefore: before?.length, arrived, steps },
      null,
      2,
    ),
  );
  console.log(`pay-contact: PASS (${steps.length} steps); screenshots in ${OUT}`);
}

/** Interrupted: stop the app and wait for it, delete the vaults and passphrase, and delete any file
 * of OUT holding a secret, as a normal exit does. */
async function interrupted(signal: NodeJS.Signals) {
  console.error(`pay-contact: FAIL: ${signal}`);
  stopDriver(driver);
  await driverGone(driver);
  try {
    if (work) rmSync(work, { recursive: true, force: true, maxRetries: 3 });
  } catch (e) {
    console.error(`pay-contact: could not remove ${work}: ${e instanceof Error ? e.message : String(e)}`);
  }
  for (const f of [...leaks(OUT, secrets), ...(SCAN_EXTRA ? leaks(SCAN_EXTRA, secrets) : [])]) rmSync(f.replace(/ \(\d+\)$/, ""), { force: true });
  process.exit(1);
}
process.once("SIGINT", (s) => void interrupted(s));
process.once("SIGTERM", (s) => void interrupted(s));

main()
  .catch((e) => {
    console.error(`pay-contact: FAIL: ${mask(e instanceof Error ? e.message : String(e))}`);
    process.exitCode = 1;
  })
  .finally(() => {
    stopDriver(driver);
    try {
      if (work) rmSync(work, { recursive: true, force: true, maxRetries: 3 });
    } catch (e) {
      console.warn(`pay-contact: could not remove ${work}: ${e instanceof Error ? e.message : String(e)}`);
    }
  });
