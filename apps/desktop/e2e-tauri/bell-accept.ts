// A contact request accepted from the bell, on the real app, the real engine and public testnet
// (DP2-09 GL tier): dwcli sends one contact request from a wallet of the testnet DashPay fixture
// to another that is not its contact yet (DP2-09 D19, D20), then tauri-driver drives the
// production renderer in WebKitGTK with the receiver restored (`--sync`), opens the bell, accepts
// the request under the passphrase sheet, and finds the sender under My Contacts. Writes: the one
// contact request, and the accept.
//
//   DW_APP=… DWCLI=… TAURI_DRIVER=… DW_TESTNET_FIXTURE=<dir> node e2e-tauri/bell-accept.ts
//
// <dir> holds `<W>.mnemonic` (0600) and `fixture.json`/`<W>.json` (the identities).
// DW_BELL_SENDER and DW_BELL_RECEIVER pick the wallets (default C and B). The send is recorded in
// `<dir>/DP2-09.json`, written exclusively before it is made: a rerun never sends again, it accepts
// the request already sent if it is still waiting. Phrases go to dwcli on stdin, never argv;
// whatever happens, no file of OUT (nor of DW_E2E_SCAN_EXTRA, the run's log directory) may hold a
// phrase or the passphrase afterwards. `scripts/e2e-tauri-bell-accept.sh` builds the binaries and
// runs this file under a private session bus, on the private X display in $DISPLAY.

import { spawnSync, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { APP, driverGone, DWCLI, env, leaks, poll, startDriver, stopDriver, tid, TIMEOUT_MS, WebDriver } from "./driver.ts";

const FIXTURE = resolve(env("DW_TESTNET_FIXTURE"));
const SENDER = env("DW_BELL_SENDER", "C");
const RECEIVER = env("DW_BELL_RECEIVER", "B");
const BIRTH_HEIGHT = env("DW_BELL_BIRTH_HEIGHT", "1568806");
const OUT = resolve(env("DW_E2E_OUT", "test-results/tauri-bell-accept"));
const PORT = Number(env("DW_E2E_PORT", "4766"));
const SCAN_EXTRA = process.env.DW_E2E_SCAN_EXTRA ? resolve(process.env.DW_E2E_SCAN_EXTRA) : null;
/** DashPay's bring-up on a fresh engine, then a pass that finds the new request. */
const DASHPAY_TIMEOUT_MS = Number(env("DW_E2E_DASHPAY_TIMEOUT_MS", "1200000"));
/** The accept: a Platform state transition and its proof. */
const ACCEPT_TIMEOUT_MS = Number(env("DW_E2E_ACCEPT_TIMEOUT_MS", "600000"));
const LEDGER = join(FIXTURE, "DP2-09.json");
const NETWORK = ["--network", "testnet"];

let driver: ChildProcess | undefined;
/** The data roots and the passphrase file: out of OUT; removed at the end. */
let work: string | undefined;
/** What no file of OUT may hold: each phrase (as read and single-spaced) and the passphrase. */
const secrets: string[] = [];

/** dwcli on a testnet data root; its stdout (JSON, no secrets). Its stderr is shown with every
 * word of a phrase and the passphrase masked, wherever they stand. */
function dwcli(root: string, passFile: string, args: string[], input?: string): string {
  const r = spawnSync(DWCLI, ["--datadir", root, ...NETWORK, "--passphrase-file", passFile, ...args], { encoding: "utf8", input, timeout: 900_000 });
  if (r.status === 0) return r.stdout;
  const words = [...new Set(secrets.flatMap((s) => s.split(/\s+/)))].filter(Boolean).sort((a, b) => b.length - a.length);
  // Whole words only: a phrase word inside a longer one ("air" in "repair") stays, so a mask does
  // not tell which words the phrase holds.
  const alternation = words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|");
  const masked = words.length ? `${r.stderr}${r.stdout}`.replace(new RegExp(`(?<![\\w-])(?:${alternation})(?![\\w-])`, "g"), "***") : r.stderr;
  throw new Error(`dwcli ${args.slice(0, 3).join(" ")} failed (${r.status}): ${masked.slice(-800)}`);
}

/** dwcli's `{"ok":true,"result":…}`. */
const result = <T>(stdout: string): T => (JSON.parse(stdout) as { result: T }).result;

/** A wallet's identity id: `fixture.json`'s row, else `<W>.json` (wallet C's). */
function identityOf(wallet: string): string {
  const fixture = JSON.parse(readFileSync(join(FIXTURE, "fixture.json"), "utf8")) as Record<string, { identity_id?: string }>;
  const id = fixture[wallet]?.identity_id ?? (JSON.parse(readFileSync(join(FIXTURE, `${wallet}.json`), "utf8")) as { identity_id?: string }).identity_id;
  if (!id) throw new Error(`the fixture names no identity for wallet ${wallet}`);
  return id;
}

/** A restored wallet's data root under `work`, its vault sealed with `passFile`. */
function restore(root: string, passFile: string, wallet: string): void {
  const phrase = readFileSync(join(FIXTURE, `${wallet}.mnemonic`), "utf8").trim();
  dwcli(root, passFile, ["init-vault"]);
  dwcli(root, passFile, ["import", "--birth-height", BIRTH_HEIGHT, "--name", `Fixture ${wallet}`], `${phrase}\n`);
}

interface ContactRow {
  contact: string;
  relation: string;
}

/** `identity`'s rows as dwcli lists them (`--spv`: after DashPay's bring-up and a pass). */
function contacts(root: string, passFile: string, identity: string): ContactRow[] {
  const list = result<{ sections: { contacts: ContactRow[] }[] }>(dwcli(root, passFile, ["contact", "--spv", "list", "--identity", identity]));
  return list.sections.flatMap((s) => s.contacts);
}

interface Ledger {
  sender: string;
  receiver: string;
  senderIdentity: string;
  receiverIdentity: string;
  state: "sending" | "sent" | "failed";
  outcome?: unknown;
  error?: string;
  before?: ContactRow[];
  at: string;
}

/** The one contact request (D19): sent unless the ledger says it was (or may have been). */
function send(root: string, passFile: string, from: string, to: string, step: (s: string) => void): Ledger {
  if (existsSync(LEDGER)) {
    const ledger = JSON.parse(readFileSync(LEDGER, "utf8")) as Ledger;
    if (ledger.state !== "sent") throw new Error(`${LEDGER} says ${ledger.state}: a send may have reached Platform; look before running again`);
    step(`the request ${SENDER}→${RECEIVER} was sent before (${ledger.at}); not sending again`);
    return ledger;
  }
  restore(root, passFile, SENDER);
  const before = contacts(root, passFile, from);
  const known = before.find((r) => r.contact === to);
  step(`sender ${SENDER} before: ${before.map((r) => `${r.contact.slice(0, 6)} ${r.relation}`).join(", ") || "no rows"}`);
  if (known) throw new Error(`${SENDER} already relates to ${RECEIVER} (${known.relation}): pick a pair that is not (D20)`);
  const ledger: Ledger = { sender: SENDER, receiver: RECEIVER, senderIdentity: from, receiverIdentity: to, state: "sending", before, at: new Date().toISOString() };
  // Exclusive: two runs never both send.
  writeFileSync(LEDGER, JSON.stringify(ledger, null, 2), { flag: "wx", mode: 0o600 });
  try {
    ledger.outcome = result(dwcli(root, passFile, ["contact", "--spv", "request", "--identity", from, to]));
    ledger.state = "sent";
  } catch (e) {
    ledger.state = "failed";
    ledger.error = e instanceof Error ? e.message : String(e);
    throw e;
  } finally {
    writeFileSync(LEDGER, JSON.stringify(ledger, null, 2), { mode: 0o600 });
  }
  step(`sent the request ${SENDER}→${RECEIVER}: ${JSON.stringify(ledger.outcome)}`);
  return ledger;
}

/** The bell's rows, as shown. */
const BELL_SCRIPT = `
  const p = document.querySelector('${tid("bell-popover")}');
  if (!p) return null;
  return {
    requests: [...p.querySelectorAll('${tid("bell-request")}')].map((r) => ({ contact: r.dataset.contact, text: r.innerText })),
    events: [...p.querySelectorAll('${tid("bell-event")}')].map((r) => r.innerText),
    outcome: p.querySelector('${tid("bell-outcome")}')?.textContent ?? null,
  };`;

interface Bell {
  requests: { contact: string; text: string }[];
  events: string[];
  outcome: string | null;
}

async function run(passphrase: string, sender: string, wd: WebDriver, step: (s: string) => void): Promise<Record<string, unknown>> {
  const out: Record<string, unknown> = {};
  await wd.fill(tid("lock-field"), passphrase);
  await wd.press(tid("lock-submit"));
  await wd.waitGone(tid("lock-screen"), "the lock screen");
  if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");
  step(`unlocked the restored wallet ${RECEIVER}`);

  // Contacts once DashPay knows the identity; opening it asks for a pass soon.
  await poll(DASHPAY_TIMEOUT_MS, "the sidebar to offer Contacts", () => wd.find(tid("sidebar-contacts")).then((e) => e ?? undefined));
  await wd.press(tid("sidebar-contacts"));
  await wd.waitFor(tid("page-contacts"), "the Contacts page");
  out.badgeBefore = await wd.script<string | null>(`return document.querySelector('${tid("toolbar-bell-badge")}')?.textContent ?? null`);
  step(`Contacts opened; bell badge ${String(out.badgeBefore)}`);

  // The bell lists the sender's request once a pass found it: open it, look, close it, again.
  const before = await poll(DASHPAY_TIMEOUT_MS, `the bell to list the request from ${sender}`, async () => {
    await wd.press(tid("toolbar-bell"));
    await wd.waitFor(tid("bell-popover"), "the bell's popover");
    // The feed's first page, read.
    const bell = await poll(TIMEOUT_MS, "the bell's feed", async () => {
      const b = await wd.script<Bell | null>(BELL_SCRIPT);
      const loading = await wd.script<boolean>(`return !!document.querySelector('${tid("bell-popover")} .placeholder-note')?.textContent?.startsWith('Loading')`);
      return b && !loading ? b : undefined;
    });
    if (bell.requests.some((r) => r.contact === sender)) return bell;
    await wd.press(tid("toolbar-bell"));
    await wd.waitGone(tid("bell-popover"), "the bell's popover");
    // Contacts asks for a pass again when it opens.
    await wd.press(tid("sidebar-home"));
    await wd.press(tid("sidebar-contacts"));
    await new Promise((r) => setTimeout(r, 15_000));
    return undefined;
  });
  out.bellBefore = before;
  await wd.screenshot("1-bell-request");
  step(`the bell lists ${before.requests.length} request(s): ${before.requests.map((r) => r.text.replace(/\s+/g, " ")).join("; ")}`);

  // Accept, under the passphrase sheet; the popover stays open through it.
  await wd.press(`${tid("bell-request")}[data-contact="${sender}"] ${tid("bell-accept")}`);
  await wd.fill(tid("grant-sheet-field"), passphrase);
  await wd.screenshot("2-grant-sheet");
  await wd.type(await wd.waitFor(tid("grant-sheet-field")), "");
  await wd.waitGone(tid("grant-sheet"), "the passphrase sheet");
  step("accepted from the bell; passphrase given");
  const outcome = await poll(ACCEPT_TIMEOUT_MS, "the accept's outcome in the bell", async () => (await wd.script<Bell | null>(BELL_SCRIPT))?.outcome ?? undefined);
  out.outcome = outcome;
  await wd.screenshot("3-accepted");
  if (!outcome.endsWith("are now contacts.")) throw new Error(`the accept failed: ${outcome}`);
  step(`outcome: ${outcome}`);
  const after = await poll(TIMEOUT_MS, `the request from ${sender} to leave the bell`, async () => {
    const b = await wd.script<Bell | null>(BELL_SCRIPT);
    return b && !b.requests.some((r) => r.contact === sender) ? b : undefined;
  });
  out.bellAfter = after;

  // My Contacts lists the sender now.
  await wd.press(tid("toolbar-bell"));
  await wd.waitGone(tid("bell-popover"), "the bell's popover");
  const row = await poll(TIMEOUT_MS, `My Contacts to list ${sender}`, () =>
    wd
      .script<{ name: string; group: string } | null>(
        `const r = document.querySelector('${tid("contacts-group-contacts")} [data-contact="${sender}"]');
         return r ? { name: r.querySelector('.contact-row-name')?.textContent ?? '', group: 'My Contacts' } : null;`,
      )
      .then((r) => r ?? undefined),
  );
  await wd.press(`${tid("contacts-group-contacts")} [data-contact="${sender}"]`);
  out.contact = { ...row, relation: await wd.text(await wd.waitFor(tid("contact-detail-relation"))) };
  await wd.screenshot("4-my-contacts");
  step(`My Contacts lists ${row.name} (${sender})`);
  return out;
}

async function main() {
  mkdirSync(OUT, { recursive: true });
  work = mkdtempSync(join(tmpdir(), "dw-bell-")); // mode 0700
  const senderRoot = join(work, "sender");
  const appRoot = join(work, "app");
  const passFile = join(work, "passphrase");
  const passphrase = randomBytes(18).toString("base64url");
  for (const w of [SENDER, RECEIVER]) {
    const phrase = readFileSync(join(FIXTURE, `${w}.mnemonic`), "utf8").trim();
    secrets.push(phrase, phrase.split(/\s+/).join(" "));
  }
  secrets.push(passphrase);
  writeFileSync(passFile, `${passphrase}\n`, { mode: 0o600 });
  const steps: string[] = [];
  const step = (s: string) => {
    steps.push(`${new Date().toISOString()} ${s}`);
    console.log(`bell-accept: ${s}`);
  };

  const sender = identityOf(SENDER);
  const receiver = identityOf(RECEIVER);
  let failure: unknown;
  let ledger: Ledger | undefined;
  let res: Record<string, unknown> = {};
  let senderAfter: ContactRow[] | undefined;
  try {
    ledger = send(senderRoot, passFile, sender, receiver, step);
    restore(appRoot, passFile, RECEIVER);
    step(`testnet data root with fixture wallet ${RECEIVER}, birth ${BIRTH_HEIGHT}`);
    const started = await startDriver(PORT, OUT, (d) => (driver = d));
    driver = started.driver;
    const wd = new WebDriver(started.base, OUT);
    try {
      await wd.start({ alwaysMatch: { "tauri:options": { application: APP, args: ["--datadir", appRoot, ...NETWORK, "--sync", "--theme", "light"] } } });
      res = await run(passphrase, sender, wd, step);
    } catch (e) {
      failure = e;
      await wd.screenshot("failure").catch(() => undefined);
    } finally {
      await wd.end().catch(() => undefined);
    }
    // The sender's side after the accept: the receiver is its contact (a pass reads the reverse
    // request). Only once the app accepted.
    if (!failure && existsSync(senderRoot)) {
      senderAfter = await poll(DASHPAY_TIMEOUT_MS, `${SENDER} to list ${RECEIVER} as a contact`, async () => {
        const rows = contacts(senderRoot, passFile, sender);
        return rows.some((r) => r.contact === receiver && r.relation === "contact") ? rows : undefined;
      });
      step(`sender ${SENDER} after: ${senderAfter.map((r) => `${r.contact.slice(0, 6)} ${r.relation}`).join(", ")}`);
    }
  } catch (e) {
    failure ??= e;
  } finally {
    // Whatever the outcome, once the app and the driver have exited and their output is on disk.
    stopDriver(driver);
    await driverGone(driver);
    driver = undefined;
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
    JSON.stringify({ ok: true, sender: { wallet: SENDER, identity: sender }, receiver: { wallet: RECEIVER, identity: receiver }, ledger, ...res, senderAfter, steps }, null, 2),
  );
  console.log(`bell-accept: PASS (${steps.length} steps); screenshots in ${OUT}`);
}

/** Interrupted (Ctrl-C, an outer timeout): stop the app and wait for it, delete the vaults and
 * passphrase, and delete any file of OUT holding a secret, as a normal exit does. A signal that
 * lands while dwcli runs waits for it (`spawnSync`). */
async function interrupted(signal: NodeJS.Signals) {
  console.error(`bell-accept: FAIL: ${signal}`);
  stopDriver(driver);
  await driverGone(driver);
  try {
    if (work) rmSync(work, { recursive: true, force: true, maxRetries: 3 });
  } catch (e) {
    console.error(`bell-accept: could not remove ${work}: ${e instanceof Error ? e.message : String(e)}`);
  }
  for (const f of [...leaks(OUT, secrets), ...(SCAN_EXTRA ? leaks(SCAN_EXTRA, secrets) : [])]) rmSync(f.replace(/ \(\d+\)$/, ""), { force: true });
  process.exit(1);
}
process.once("SIGINT", (s) => void interrupted(s));
process.once("SIGTERM", (s) => void interrupted(s));

main()
  .catch((e) => {
    console.error(`bell-accept: FAIL: ${e instanceof Error ? e.message : String(e)}`);
    process.exitCode = 1;
  })
  .finally(() => {
    stopDriver(driver);
    try {
      if (work) rmSync(work, { recursive: true, force: true, maxRetries: 3 });
    } catch (e) {
      console.warn(`bell-accept: could not remove ${work}: ${e instanceof Error ? e.message : String(e)}`);
    }
  });
