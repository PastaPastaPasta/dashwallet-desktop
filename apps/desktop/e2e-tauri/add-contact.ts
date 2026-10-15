// A contact request sent from Add Contact, on the real app, the real engine and public testnet
// (DP2-08 GL tier): tauri-driver drives the production renderer in WebKitGTK with a wallet of the
// testnet DashPay fixture restored (`--sync`), searches another fixture wallet's username, sends
// it a contact request under the passphrase sheet, then hands the app that user's
// `dashpay://user` link through a second launch and finds Add Contact prefilled and verified, with
// no second Send. dwcli reads the recipient's contacts before and after. Writes: the one contact
// request, left unaccepted.
//
//   DW_APP=… DWCLI=… TAURI_DRIVER=… DW_TESTNET_FIXTURE=<dir> node e2e-tauri/add-contact.ts
//
// <dir> holds `<W>.mnemonic` (0600) and `fixture.json` (the identities, the recipient's username
// in DW_ADD_USERNAME). DW_ADD_SENDER and DW_ADD_RECIPIENT pick the wallets (default A and D, a
// pair that is not contacts after DP2-09). The send is recorded in `<dir>/DP2-08.json`, written
// exclusively before Send is pressed: a rerun never sends again, it checks the request already
// sent. Phrases go to dwcli on stdin, never argv; whatever happens, no file of OUT (nor of
// DW_E2E_SCAN_EXTRA, the run's log directory) may hold a phrase or the passphrase afterwards.
// `scripts/e2e-tauri-add-contact.sh` builds the binaries and runs this file under a private
// session bus, on the private X display in $DISPLAY.

import { spawnSync, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { APP, driverGone, DWCLI, env, leaks, poll, startDriver, stopDriver, tid, TIMEOUT_MS, WebDriver } from "./driver.ts";

const FIXTURE = resolve(env("DW_TESTNET_FIXTURE"));
const SENDER = env("DW_ADD_SENDER", "A");
const RECIPIENT = env("DW_ADD_RECIPIENT", "D");
const USERNAME = env("DW_ADD_USERNAME", "dwtdashd2");
const BIRTH_HEIGHT = env("DW_ADD_BIRTH_HEIGHT", "1568806");
const OUT = resolve(env("DW_E2E_OUT", "test-results/tauri-add-contact"));
const PORT = Number(env("DW_E2E_PORT", "4768"));
const SCAN_EXTRA = process.env.DW_E2E_SCAN_EXTRA ? resolve(process.env.DW_E2E_SCAN_EXTRA) : null;
/** DashPay's bring-up on a fresh engine, then a pass that finds the new request. */
const DASHPAY_TIMEOUT_MS = Number(env("DW_E2E_DASHPAY_TIMEOUT_MS", "1200000"));
/** The send: a Platform state transition and its proof. */
const SEND_TIMEOUT_MS = Number(env("DW_E2E_SEND_TIMEOUT_MS", "600000"));
const LEDGER = join(FIXTURE, "DP2-08.json");
const NETWORK = ["--network", "testnet"];
const SENT_LINE = "Contact request sent.";

let driver: ChildProcess | undefined;
/** The data roots and the passphrase file: out of OUT; removed at the end. */
let work: string | undefined;
/** What no file of OUT may hold: each phrase (as read and single-spaced) and the passphrase. */
const secrets: string[] = [];

/** `text` with every word of a phrase and the passphrase masked, wherever they stand. */
function mask(text: string): string {
  const words = [...new Set(secrets.flatMap((s) => s.split(/\s+/)))].filter(Boolean).sort((a, b) => b.length - a.length);
  if (!words.length) return text;
  // Whole words only: a phrase word inside a longer one ("air" in "repair") stays, so a mask does
  // not tell which words the phrase holds.
  const alternation = words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|");
  return text.replace(new RegExp(`(?<![\\w-])(?:${alternation})(?![\\w-])`, "g"), "***");
}

/** dwcli on a testnet data root; its stdout (JSON, no secrets). Its output is shown masked when it
 * fails. */
function dwcli(root: string, passFile: string, args: string[], input?: string): string {
  const r = spawnSync(DWCLI, ["--datadir", root, ...NETWORK, "--passphrase-file", passFile, ...args], { encoding: "utf8", input, timeout: 900_000 });
  if (r.status === 0) return r.stdout;
  throw new Error(`dwcli ${args.slice(0, 3).join(" ")} failed (${r.status}): ${mask(`${r.stderr}${r.stdout}`).slice(-800)}`);
}

/** dwcli's `{"ok":true,"result":…}`. */
const result = <T>(stdout: string): T => (JSON.parse(stdout) as { result: T }).result;

/** A wallet's identity id: `fixture.json`'s row (a string, or `{identity_id}`), else `<W>.json`. */
function identityOf(wallet: string): string {
  const fixture = JSON.parse(readFileSync(join(FIXTURE, "fixture.json"), "utf8")) as Record<string, string | { identity_id?: string }>;
  const row = fixture[wallet];
  const id =
    (typeof row === "string" ? row : row?.identity_id) ??
    (existsSync(join(FIXTURE, `${wallet}.json`)) ? (JSON.parse(readFileSync(join(FIXTURE, `${wallet}.json`), "utf8")) as { identity_id?: string }).identity_id : undefined);
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

const rows = (list: ContactRow[]) => list.map((r) => `${r.contact.slice(0, 6)} ${r.relation}`).join(", ") || "no rows";

interface Ledger {
  sender: string;
  recipient: string;
  senderIdentity: string;
  recipientIdentity: string;
  username: string;
  state: "sending" | "sent" | "failed";
  outcome?: string;
  error?: string;
  /** The recipient's rows before the send (dwcli). */
  before?: ContactRow[];
  at: string;
}

/** The ledger of an earlier run, if any (D24): only a finished send for this pair is reused. */
function earlier(from: string, to: string): Ledger | null {
  if (!existsSync(LEDGER)) return null;
  const ledger = JSON.parse(readFileSync(LEDGER, "utf8")) as Ledger;
  if (ledger.senderIdentity !== from || ledger.recipientIdentity !== to)
    throw new Error(`${LEDGER} records ${ledger.sender}→${ledger.recipient}, not ${SENDER}→${RECIPIENT}: run that pair, or move the ledger aside for a new one`);
  if (ledger.state !== "sent") throw new Error(`${LEDGER} says ${ledger.state}: a send may have reached Platform; look before running again`);
  return ledger;
}

/** The candidate card, as shown. */
const CARD_SCRIPT = `
  const root = document.querySelector('${tid("add-contact")}');
  if (!root) return null;
  const text = (id) => root.querySelector('[data-testid="' + id + '"]')?.innerText.trim() ?? null;
  return {
    name: text("add-contact-name"),
    username: text("add-contact-username"),
    identity: root.querySelector('${tid("add-contact-identity")}')?.innerText.replace(/\\s+/g, "") ?? null,
    verified: text("add-contact-verified"),
    line: text("add-contact-line"),
    send: !!root.querySelector('${tid("add-contact-send")}'),
    show: text("add-contact-show"),
    outcome: text("add-contact-outcome"),
    linkError: text("add-contact-link-error"),
  };`;

interface Card {
  name: string | null;
  username: string | null;
  identity: string | null;
  verified: string | null;
  line: string | null;
  send: boolean;
  show: string | null;
  outcome: string | null;
  linkError: string | null;
}

const card = (wd: WebDriver) => wd.script<Card | null>(CARD_SCRIPT);

/** The card once it settled: a next step, or a line, for `identity`. */
function settled(wd: WebDriver, identity: string, what: string, ms = TIMEOUT_MS): Promise<Card> {
  return poll(ms, what, async () => {
    const c = await card(wd);
    if (c?.linkError) throw new Error(`the link did not verify: ${c.linkError}`);
    return c && c.identity?.includes(identity) && (c.send || c.line) ? c : undefined;
  });
}

/** The second launch: hands `link` to the running app (U-01) and exits. */
function secondLaunch(dataRoot: string, link: string) {
  const r = spawnSync(APP, ["--datadir", dataRoot, link], { encoding: "utf8", timeout: 60_000, killSignal: "SIGKILL" });
  if (r.error || r.status !== 0)
    throw new Error(`the second launch did not hand over its link and exit (status ${r.status}, ${r.error?.message ?? r.stderr.slice(-400)})`);
}

async function run(
  passphrase: string,
  appRoot: string,
  recipient: string,
  wd: WebDriver,
  ledgerIn: Ledger | null,
  writeLedger: (l: Ledger) => void,
  base: Omit<Ledger, "state" | "at">,
  step: (s: string) => void,
): Promise<{ out: Record<string, unknown>; ledger: Ledger | null }> {
  const out: Record<string, unknown> = {};
  let ledger = ledgerIn;
  await wd.fill(tid("lock-field"), passphrase);
  await wd.press(tid("lock-submit"));
  await wd.waitGone(tid("lock-screen"), "the lock screen");
  if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");
  step(`unlocked the restored wallet ${SENDER}`);

  await poll(DASHPAY_TIMEOUT_MS, "the sidebar to offer Contacts", () => wd.find(tid("sidebar-contacts")).then((e) => e ?? undefined));
  await wd.press(tid("sidebar-contacts"));
  await wd.waitFor(tid("page-contacts"), "the Contacts page");
  await wd.press(tid("contacts-add"));
  await wd.waitFor(tid("add-contact"), "Add Contact");

  // Search finds the username once DashPay knows the identity; a failed search is tried again.
  await wd.fill(`${tid("add-contact-search")} input`, USERNAME);
  const hit = `${tid("add-contact-hit")}`;
  const hitText = await poll(DASHPAY_TIMEOUT_MS, `Search to find ${USERNAME}`, async () => {
    const texts = await wd.script<string[]>(`return [...document.querySelectorAll('${hit}')].map((h) => h.innerText)`);
    const found = texts.find((t) => t.includes(`@${USERNAME}`) || t.split("\n")[0] === USERNAME);
    if (found) return found;
    const retry = await wd.find(`${tid("add-contact-search-error")} button`);
    if (retry) {
      await new Promise((r) => setTimeout(r, 15_000));
      await wd.click(retry);
    }
    return undefined;
  });
  step(`Search lists ${hitText.replace(/\s+/g, " ")}`);
  await wd.screenshot("1-search");
  const index = await wd.script<number>(`return [...document.querySelectorAll('${hit}')].findIndex((h) => h.innerText.includes('@${USERNAME}') || h.innerText.split('\\n')[0] === '${USERNAME}')`);
  await wd.click((await wd.findAll(hit))[index]!);

  const before = await settled(wd, recipient, `the card for ${USERNAME} to settle`, DASHPAY_TIMEOUT_MS);
  out.cardBefore = before;
  step(`card: ${before.name} ${before.username ?? ""}, ${before.verified?.replace(/\s+/g, " ")}, line ${JSON.stringify(before.line)}, send ${before.send}`);
  await wd.screenshot("2-candidate");

  if (ledger) {
    step(`the request ${SENDER}→${RECIPIENT} was sent before (${ledger.at}); not sending again`);
    if (before.send) throw new Error("the ledger records a send, yet the card offers Send");
  } else {
    if (!before.send) throw new Error(`the card offers no Send: ${before.line}`);
    // Exclusive: two runs never both send.
    ledger = { ...base, state: "sending", at: new Date().toISOString() };
    writeLedger(ledger);
    try {
      await wd.press(tid("add-contact-send"));
      await wd.fill(tid("grant-sheet-field"), passphrase);
      await wd.screenshot("3-grant-sheet");
      await wd.press(tid("grant-sheet-submit"));
      await wd.waitGone(tid("grant-sheet"), "the passphrase sheet");
      step("Send Contact Request; passphrase given");
      const outcome = await poll(SEND_TIMEOUT_MS, "the send's outcome", async () => (await card(wd))?.outcome ?? undefined);
      ledger.outcome = outcome;
      if (!outcome.startsWith("Contact request sent to")) throw new Error(`the send failed: ${outcome}`);
      ledger.state = "sent";
      step(`outcome: ${outcome}`);
    } catch (e) {
      ledger.state = "failed";
      ledger.error = mask(e instanceof Error ? e.message : String(e));
      throw e;
    } finally {
      writeLedger(ledger);
    }
  }
  const after = await poll(DASHPAY_TIMEOUT_MS, `the card to say "${SENT_LINE}"`, async () => {
    const c = await card(wd);
    return c?.line === SENT_LINE ? c : undefined;
  });
  if (after.send) throw new Error("the card still offers Send after the send");
  out.cardAfter = after;
  await wd.screenshot("4-sent");
  step(`card after: line ${JSON.stringify(after.line)}, ${after.show}`);

  // The user's link, from outside (U-01): Add Contact opens on it, verified, with no Send.
  await wd.press(tid("add-contact-close"));
  await wd.waitGone(tid("add-contact"), "Add Contact");
  const link = `dashpay://user?id=${recipient}&username=${USERNAME}`;
  secondLaunch(appRoot, link);
  step(`second launch handed over ${link}`);
  const linked = await settled(wd, recipient, "Add Contact to open on the link", DASHPAY_TIMEOUT_MS);
  out.link = link;
  out.cardLink = linked;
  await wd.screenshot("5-link");
  if (!linked.verified?.includes("Verified on the Dash network") || !linked.verified.includes("From a link"))
    throw new Error(`the link's card is not verified: ${linked.verified}`);
  if (linked.send) throw new Error("the link's card offers a second Send");
  if (linked.line !== SENT_LINE) throw new Error(`the link's card says ${JSON.stringify(linked.line)}, not "${SENT_LINE}"`);
  step(`link card: ${linked.name} ${linked.username ?? ""}, ${linked.verified.replace(/\s+/g, " ")}, line ${JSON.stringify(linked.line)}, no Send`);
  return { out, ledger };
}

async function main() {
  mkdirSync(OUT, { recursive: true });
  work = mkdtempSync(join(tmpdir(), "dw-add-")); // mode 0700
  const recipientRoot = join(work, "recipient");
  const appRoot = join(work, "app");
  const passFile = join(work, "passphrase");
  const passphrase = randomBytes(18).toString("base64url");
  for (const w of [SENDER, RECIPIENT]) {
    const phrase = readFileSync(join(FIXTURE, `${w}.mnemonic`), "utf8").trim();
    secrets.push(phrase, phrase.split(/\s+/).join(" "));
  }
  secrets.push(passphrase);
  writeFileSync(passFile, `${passphrase}\n`, { mode: 0o600 });
  const steps: string[] = [];
  const step = (s: string) => {
    steps.push(`${new Date().toISOString()} ${s}`);
    console.log(`add-contact: ${s}`);
  };
  const writeLedger = (l: Ledger) => writeFileSync(LEDGER, JSON.stringify(l, null, 2), { flag: l.state === "sending" ? "wx" : "w", mode: 0o600 });

  const sender = identityOf(SENDER);
  const recipient = identityOf(RECIPIENT);
  let failure: unknown;
  let ledger: Ledger | null = null;
  let res: Record<string, unknown> = {};
  let recipientBefore: ContactRow[] | undefined;
  let recipientAfter: ContactRow[] | undefined;
  try {
    ledger = earlier(sender, recipient);
    // The recipient's side first: a pair that relates already is refused before the app runs (D24).
    restore(recipientRoot, passFile, RECIPIENT);
    recipientBefore = contacts(recipientRoot, passFile, recipient);
    step(`recipient ${RECIPIENT} before: ${rows(recipientBefore)}`);
    const known = recipientBefore.find((r) => r.contact === sender);
    if (!ledger && known) throw new Error(`${RECIPIENT} already relates to ${SENDER} (${known.relation}): pick a pair that does not`);
    if (known?.relation === "contact") throw new Error(`${SENDER} and ${RECIPIENT} are contacts: the request was accepted; move the ledger aside and pick a new pair`);
    restore(appRoot, passFile, SENDER);
    step(`testnet data root with fixture wallet ${SENDER}, birth ${BIRTH_HEIGHT}`);
    const base = { sender: SENDER, recipient: RECIPIENT, senderIdentity: sender, recipientIdentity: recipient, username: USERNAME, before: recipientBefore };
    const started = await startDriver(PORT, OUT, (d) => (driver = d));
    driver = started.driver;
    const wd = new WebDriver(started.base, OUT);
    try {
      await wd.start({ alwaysMatch: { "tauri:options": { application: APP, args: ["--datadir", appRoot, ...NETWORK, "--sync", "--theme", "light"] } } });
      const r = await run(passphrase, appRoot, recipient, wd, ledger, writeLedger, base, step);
      res = r.out;
      ledger = r.ledger;
    } catch (e) {
      failure = e;
      await wd.screenshot("failure").catch(() => undefined);
    } finally {
      await wd.end().catch(() => undefined);
    }
    // The recipient's side after the send: the sender's request waits there, unaccepted.
    if (!failure) {
      recipientAfter = await poll(DASHPAY_TIMEOUT_MS, `${RECIPIENT} to list ${SENDER}'s request`, async () => {
        const list = contacts(recipientRoot, passFile, recipient);
        return list.some((r) => r.contact === sender && r.relation === "pending_incoming") ? list : undefined;
      });
      step(`recipient ${RECIPIENT} after: ${rows(recipientAfter)}`);
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
    JSON.stringify(
      { ok: true, sender: { wallet: SENDER, identity: sender }, recipient: { wallet: RECIPIENT, identity: recipient, username: USERNAME }, ledger, ...res, recipientBefore, recipientAfter, steps },
      null,
      2,
    ),
  );
  console.log(`add-contact: PASS (${steps.length} steps); screenshots in ${OUT}`);
}

/** Interrupted (Ctrl-C, an outer timeout): stop the app and wait for it, delete the vaults and
 * passphrase, and delete any file of OUT holding a secret, as a normal exit does. A signal that
 * lands while dwcli runs waits for it (`spawnSync`). */
async function interrupted(signal: NodeJS.Signals) {
  console.error(`add-contact: FAIL: ${signal}`);
  stopDriver(driver);
  await driverGone(driver);
  try {
    if (work) rmSync(work, { recursive: true, force: true, maxRetries: 3 });
  } catch (e) {
    console.error(`add-contact: could not remove ${work}: ${e instanceof Error ? e.message : String(e)}`);
  }
  for (const f of [...leaks(OUT, secrets), ...(SCAN_EXTRA ? leaks(SCAN_EXTRA, secrets) : [])]) rmSync(f.replace(/ \(\d+\)$/, ""), { force: true });
  process.exit(1);
}
process.once("SIGINT", (s) => void interrupted(s));
process.once("SIGTERM", (s) => void interrupted(s));

main()
  .catch((e) => {
    console.error(`add-contact: FAIL: ${mask(e instanceof Error ? e.message : String(e))}`);
    process.exitCode = 1;
  })
  .finally(() => {
    stopDriver(driver);
    try {
      if (work) rmSync(work, { recursive: true, force: true, maxRetries: 3 });
    } catch (e) {
      console.warn(`add-contact: could not remove ${work}: ${e instanceof Error ? e.message : String(e)}`);
    }
  });
