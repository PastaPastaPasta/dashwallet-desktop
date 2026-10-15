// Contacts on the real app, the real engine and public testnet (DP2-07 GL tier): tauri-driver
// drives the production renderer in WebKitGTK. A fresh data root restores a wallet of the
// testnet DashPay fixture (an identity with established contacts), the app syncs (`--sync`: SPV
// and DashPay's bring-up), and the Contacts page must list them: the contact rows, the detail
// slot, a sort, the local and the network search. Reads only: nothing is sent, accepted or
// registered.
//
//   DW_APP=… DWCLI=… TAURI_DRIVER=… DW_TESTNET_FIXTURE=<dir> node e2e-tauri/contacts.ts
//
// <dir> holds `<W>.mnemonic` (0600) and `fixture.json` (the identities); DW_CONTACTS_WALLET
// picks W (default A) and DW_CONTACTS_EXPECT the comma-separated usernames its My Contacts must
// list (default dwrecoverb2, wallet B's name). The phrase goes to dwcli on stdin, never argv;
// whatever happens, no file of OUT may hold it or the passphrase afterwards.
// `scripts/e2e-tauri-contacts.sh` builds the binaries and runs this file under a private
// session bus, on the private X display in $DISPLAY.

import { spawnSync, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { APP, driverGone, DWCLI, env, leaks, poll, startDriver, stopDriver, tid, TIMEOUT_MS, WebDriver } from "./driver.ts";

const FIXTURE = resolve(env("DW_TESTNET_FIXTURE"));
const WALLET = env("DW_CONTACTS_WALLET", "A");
const EXPECT = env("DW_CONTACTS_EXPECT", "dwrecoverb2").split(",");
const BIRTH_HEIGHT = env("DW_CONTACTS_BIRTH_HEIGHT", "1568806");
const OUT = resolve(env("DW_E2E_OUT", "test-results/tauri-contacts"));
const PORT = Number(env("DW_E2E_PORT", "4764"));
/** DashPay's bring-up on a fresh engine (identities, names, contact requests) and the verified
 * re-fetch of the names (the first answer can come from the trusted fallback). */
const DASHPAY_TIMEOUT_MS = Number(env("DW_E2E_DASHPAY_TIMEOUT_MS", "900000"));
const NETWORK = ["--network", "testnet"];

let driver: ChildProcess | undefined;
/** The data root and the passphrase file: out of OUT; removed at the end. */
let work: string | undefined;
/** What no file of OUT may hold: the phrase (as read and with its words single-spaced) and the
 * passphrase. */
const secrets: string[] = [];

/** dwcli on the testnet data root. Its stderr is printed with every word of the phrase and the
 * passphrase masked, wherever they stand (quoted, in a list). */
function dwcli(root: string, passFile: string, args: string[], input?: string): void {
  const r = spawnSync(DWCLI, ["--datadir", root, ...NETWORK, "--passphrase-file", passFile, ...args], { encoding: "utf8", input, timeout: 600_000 });
  if (r.status === 0) return;
  const words = [...new Set(secrets.flatMap((s) => s.split(/\s+/)))].filter(Boolean).sort((a, b) => b.length - a.length);
  const masked = words.length ? r.stderr.replace(new RegExp(words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|"), "g"), "***") : r.stderr;
  throw new Error(`dwcli ${args[0]} failed (${r.status}): ${masked.slice(-600)}`);
}

interface Row {
  group: string;
  name: string;
  username: string | null;
  chips: string[];
  note: string | null;
}

/** The page's groups and rows, as shown. */
const ROWS_SCRIPT = `
  return [...document.querySelectorAll('[data-testid^="contacts-group-"]')].flatMap((g) => {
    const group = g.querySelector('h2')?.textContent ?? '';
    return [...g.querySelectorAll('[data-testid="contact-row"]')].map((r) => ({
      group,
      name: r.querySelector('.contact-row-name')?.textContent ?? '',
      username: r.querySelector('.contact-row-username')?.textContent ?? null,
      chips: [...r.querySelectorAll('.chip')].map((c) => c.textContent),
      note: r.querySelector('.contact-row-note')?.textContent ?? null,
    }));
  });`;

const isRow = (r: Row, username: string) => r.name === username || r.username === `@${username}`;

async function run(passphrase: string, wd: WebDriver, step: (s: string) => void): Promise<Record<string, unknown>> {
  const result: Record<string, unknown> = {};
  await wd.fill(tid("lock-field"), passphrase);
  await wd.press(tid("lock-submit"));
  await wd.waitGone(tid("lock-screen"), "the lock screen");
  if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");
  step("unlocked the restored wallet");

  // The sidebar offers Contacts once DashPay knows the identity (before that, Join DashPay):
  // on a fresh restore, after SPV and DashPay's bring-up.
  await poll(DASHPAY_TIMEOUT_MS, "the sidebar to offer Contacts", () => wd.find(tid("sidebar-contacts")).then((e) => e ?? undefined));
  await wd.press(tid("sidebar-contacts"));
  await wd.waitFor(tid("page-contacts"), "the Contacts page");
  await wd.screenshot("1-contacts-opened");
  step("Contacts opened");

  // Every expected username listed in My Contacts, verified (no Unverified chip).
  const rows = await poll(DASHPAY_TIMEOUT_MS, `My Contacts to list ${EXPECT.join(", ")}, verified`, async () => {
    const rows = await wd.script<Row[]>(ROWS_SCRIPT);
    const mine = rows.filter((r) => r.group === "My Contacts");
    return EXPECT.every((u) => mine.some((r) => isRow(r, u) && !r.chips.includes("Unverified"))) ? rows : undefined;
  });
  result.rows = rows;
  result.hint = await wd.script<string | null>(`return document.querySelector('${tid("contacts-setup-hint")}')?.textContent ?? null`);
  await wd.screenshot("2-contacts-list");
  step(`listed ${rows.length} row(s): ${rows.map((r) => `${r.group}: ${r.name}${r.username ? ` ${r.username}` : ""}`).join("; ")}`);

  // The detail slot shows the first expected contact's identity, one of the fixture's.
  const contactIds = Object.entries(JSON.parse(readFileSync(join(FIXTURE, "fixture.json"), "utf8")) as Record<string, { identity_id?: string }>)
    .filter(([w]) => w !== WALLET)
    .map(([, v]) => v?.identity_id);
  const picked = await wd.script<string | null>(
    `const u = arguments[0];
     const row = [...document.querySelectorAll('[data-testid="contact-row"]')].find((r) =>
       r.querySelector('.contact-row-name')?.textContent === u || r.querySelector('.contact-row-username')?.textContent === '@' + u);
     if (!row) return null; row.click(); return row.dataset.contact;`,
    [EXPECT[0]],
  );
  const shown = await wd.text(await wd.waitFor(tid("contact-detail-identity"), "the detail slot"));
  if (!picked || shown !== picked) throw new Error(`the detail slot shows ${shown}, not the row's ${picked}`);
  if (!contactIds.includes(shown)) throw new Error(`${EXPECT[0]} is ${shown}, not an identity of the fixture`);
  result.selected = { username: EXPECT[0], identity: shown, relation: await wd.text(await wd.waitFor(tid("contact-detail-relation"))) };
  await wd.screenshot("3-contact-selected");
  step(`selected ${EXPECT[0]}: identity ${shown}`);

  // Another order: the shell's store keeps it, so Contacts opened again still shows it.
  await wd.press(`${tid("contacts-sort")} option[value="dateAdded"]`);
  await wd.press(tid("sidebar-home"));
  await wd.press(tid("sidebar-contacts"));
  await poll(TIMEOUT_MS, "Sort by Date added after reopening Contacts", async () =>
    (await wd.script<string | null>(`return document.querySelector('${tid("contacts-sort")}')?.value ?? null`)) === "dateAdded" ? true : undefined,
  );
  await wd.waitFor(tid("contact-row"), "the rows sorted by date added");
  result.byDateAdded = (await wd.script<Row[]>(ROWS_SCRIPT)).map((r) => `${r.group}: ${r.name}`);
  await wd.screenshot("4-sorted-by-date-added");
  step("sorted by Date added (kept across a reopen)");
  await wd.press(`${tid("contacts-sort")} option[value="displayName"]`);

  // The local search keeps the contact; the network search reads DPNS (a read, no write).
  const prefix = EXPECT[0].slice(0, 5);
  await wd.fill(`${tid("contacts-search")} input`, prefix);
  const network = await poll(TIMEOUT_MS, "the network search to answer", () =>
    wd.script<string | undefined>(`
      const n = document.querySelector('${tid("contacts-network")}');
      if (!n || n.getAttribute('aria-busy') === 'true') return undefined;
      return n.innerText;`),
  );
  const filtered = await poll(TIMEOUT_MS, `the search “${prefix}” to keep ${EXPECT[0]}`, async () => {
    const rows = await wd.script<Row[]>(ROWS_SCRIPT);
    return rows.some((r) => isRow(r, EXPECT[0])) ? rows : undefined;
  });
  result.search = { prefix, rows: filtered.map((r) => r.name), network };
  await wd.screenshot("5-search");
  step(`searched “${prefix}”: ${filtered.length} row(s); network section: ${network.replace(/\s+/g, " ").slice(0, 200)}`);
  return result;
}

async function main() {
  mkdirSync(OUT, { recursive: true });
  work = mkdtempSync(join(tmpdir(), "dw-contacts-")); // mode 0700
  const dataRoot = join(work, "app");
  const passFile = join(work, "passphrase");
  const passphrase = randomBytes(18).toString("base64url");
  const phrase = readFileSync(join(FIXTURE, `${WALLET}.mnemonic`), "utf8").trim();
  secrets.push(...new Set([phrase, phrase.split(/\s+/).join(" "), passphrase]));
  writeFileSync(passFile, `${passphrase}\n`, { mode: 0o600 });
  const steps: string[] = [];
  const step = (s: string) => {
    steps.push(`${new Date().toISOString()} ${s}`);
    console.log(`contacts: ${s}`);
  };

  let failure: unknown;
  let result: Record<string, unknown> = {};
  try {
    dwcli(dataRoot, passFile, ["init-vault"]);
    dwcli(dataRoot, passFile, ["import", "--birth-height", BIRTH_HEIGHT, "--name", `Fixture ${WALLET}`], `${phrase}\n`);
    step(`testnet data root with fixture wallet ${WALLET}, birth ${BIRTH_HEIGHT}`);
    const started = await startDriver(PORT, OUT);
    driver = started.driver;
    const wd = new WebDriver(started.base, OUT);
    try {
      await wd.start({ alwaysMatch: { "tauri:options": { application: APP, args: ["--datadir", dataRoot, ...NETWORK, "--sync", "--theme", "light"] } } });
      result = await run(passphrase, wd, step);
    } catch (e) {
      failure = e;
      await wd.screenshot("failure").catch(() => undefined);
    } finally {
      await wd.end().catch(() => undefined);
    }
  } catch (e) {
    failure ??= e;
  } finally {
    // Whatever the outcome, once the app and the driver have exited and their output is on disk.
    stopDriver(driver);
    await driverGone(driver);
    driver = undefined;
  }
  const found = leaks(OUT, secrets);
  if (found.length > 0) {
    for (const f of found) rmSync(f.replace(/ \(\d+\)$/, ""), { force: true });
    const leak = `secrets in plain text: ${found.join(", ")}`;
    throw new Error(failure ? `${failure instanceof Error ? failure.message : String(failure)}; ${leak}` : leak);
  }
  if (failure) throw failure;
  step("no phrase or passphrase in OUT");
  writeFileSync(join(OUT, "result.json"), JSON.stringify({ ok: true, wallet: WALLET, birthHeight: BIRTH_HEIGHT, ...result, steps }, null, 2));
  console.log(`contacts: PASS (${steps.length} steps); screenshots in ${OUT}`);
}

/** Interrupted (Ctrl-C, an outer timeout): stop the app, delete the vault and passphrase, and
 * delete any file of OUT holding a secret, as a normal exit does. */
function interrupted(signal: NodeJS.Signals) {
  stopDriver(driver);
  if (work) rmSync(work, { recursive: true, force: true });
  for (const f of leaks(OUT, secrets)) rmSync(f.replace(/ \(\d+\)$/, ""), { force: true });
  console.error(`contacts: FAIL: ${signal}`);
  process.exit(1);
}
process.once("SIGINT", interrupted);
process.once("SIGTERM", interrupted);

main()
  .catch((e) => {
    console.error(`contacts: FAIL: ${e instanceof Error ? e.message : String(e)}`);
    process.exitCode = 1;
  })
  .finally(() => {
    stopDriver(driver);
    try {
      if (work) rmSync(work, { recursive: true, force: true, maxRetries: 3 });
    } catch (e) {
      console.warn(`contacts: could not remove ${work}: ${e instanceof Error ? e.message : String(e)}`);
    }
  });
