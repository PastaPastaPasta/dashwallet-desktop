// Contacts on the real app, the real engine and public testnet (DP2-07 GL tier): tauri-driver
// drives the production renderer in WebKitGTK. A fresh data root restores a wallet of the
// testnet DashPay fixture (an identity with established contacts), and the Contacts page must
// list them: the contact rows, the detail slot, a sort, the local search and the network
// search. Reads only: nothing is sent, accepted or registered.
//
//   DW_APP=… DWCLI=… TAURI_DRIVER=… DW_TESTNET_FIXTURE=<dir> node e2e-tauri/contacts.ts
//
// <dir> holds `<W>.mnemonic` (0600) and `fixture.json` (the identities); DW_CONTACTS_WALLET
// picks W (default A) and DW_CONTACTS_EXPECT the comma-separated usernames its My Contacts must
// list (default dwrecoverb2, wallet B's name). The phrase goes to dwcli on stdin, never argv
// or OUT; at the end no file of OUT may hold it or the passphrase.
// `scripts/e2e-tauri-contacts.sh` builds the binaries and runs this file under a private
// session bus, on the private X display in $DISPLAY.

import { spawnSync, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { APP, driverGone, DWCLI, env, poll, startDriver, stopDriver, tid, TIMEOUT_MS, WebDriver } from "./driver.ts";

const FIXTURE = resolve(env("DW_TESTNET_FIXTURE"));
const WALLET = env("DW_CONTACTS_WALLET", "A");
const EXPECT = env("DW_CONTACTS_EXPECT", "dwrecoverb2").split(",");
const BIRTH_HEIGHT = env("DW_CONTACTS_BIRTH_HEIGHT", "1568806");
const OUT = resolve(env("DW_E2E_OUT", "test-results/tauri-contacts"));
const PORT = Number(env("DW_E2E_PORT", "4764"));
/** DashPay's first pass on a fresh engine (identities, names, contact requests) and the
 * verified re-fetch of the names (the first answer can come from the trusted fallback). */
const DASHPAY_TIMEOUT_MS = Number(env("DW_E2E_DASHPAY_TIMEOUT_MS", "900000"));
const NETWORK = ["--network", "testnet"];

let driver: ChildProcess | undefined;
/** The data root and the passphrase file: out of OUT; removed at the end. */
let work: string | undefined;

function dwcli(root: string, passFile: string, args: string[], input?: string): string {
  const r = spawnSync(DWCLI, ["--datadir", root, ...NETWORK, "--passphrase-file", passFile, ...args], { encoding: "utf8", input, timeout: 600_000 });
  // stderr only: the phrase is never echoed, but stdout is not printed either.
  if (r.status !== 0) throw new Error(`dwcli ${args[0]} failed (${r.status}): ${r.stderr.slice(-600)}`);
  return r.stdout;
}

/** Files of `dir` that hold any of `secrets` in plain text. */
function leaks(dir: string, secrets: string[]): string[] {
  const found: string[] = [];
  const walk = (d: string) => {
    for (const name of readdirSync(d)) {
      const p = join(d, name);
      const st = statSync(p, { throwIfNoEntry: false });
      if (st?.isDirectory()) walk(p);
      else if (st?.isFile() && secrets.some((s) => readFileSync(p).toString("utf8").includes(s))) found.push(p);
    }
  };
  walk(dir);
  return found;
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

async function main() {
  mkdirSync(OUT, { recursive: true });
  work = mkdtempSync(join(tmpdir(), "dw-contacts-")); // mode 0700
  const dataRoot = join(work, "app");
  const passFile = join(work, "passphrase");
  const passphrase = randomBytes(18).toString("base64url");
  writeFileSync(passFile, `${passphrase}\n`, { mode: 0o600 });
  const phrase = readFileSync(join(FIXTURE, `${WALLET}.mnemonic`), "utf8").trim();
  const fixture = JSON.parse(readFileSync(join(FIXTURE, "fixture.json"), "utf8")) as Record<string, { identity_id?: string }>;
  const identity = fixture[WALLET]?.identity_id;
  const contactIds = Object.entries(fixture)
    .filter(([w, v]) => w !== WALLET && typeof v === "object" && v?.identity_id)
    .map(([, v]) => v.identity_id!);

  dwcli(dataRoot, passFile, ["init-vault"]);
  dwcli(dataRoot, passFile, ["import", "--birth-height", BIRTH_HEIGHT, "--name", `Fixture ${WALLET}`], `${phrase}\n`);
  console.log(`contacts: testnet data root with fixture wallet ${WALLET} (identity ${identity}), birth ${BIRTH_HEIGHT}`);

  const started = await startDriver(PORT, OUT);
  driver = started.driver;
  const wd = new WebDriver(started.base, OUT);
  await wd.start({ alwaysMatch: { "tauri:options": { application: APP, args: ["--datadir", dataRoot, ...NETWORK, "--theme", "light"] } } });
  const steps: string[] = [];
  const step = (s: string) => {
    steps.push(`${new Date().toISOString()} ${s}`);
    console.log(`contacts: ${s}`);
  };
  const result: Record<string, unknown> = { wallet: WALLET, identity, birthHeight: BIRTH_HEIGHT };
  let failure: unknown;
  try {
    await wd.fill(tid("lock-field"), passphrase);
    await wd.press(tid("lock-submit"));
    await wd.waitGone(tid("lock-screen"), "the lock screen");
    if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");
    step("unlocked the restored wallet");

    // The sidebar names Contacts once DashPay knows the wallet's identity (Join DashPay before).
    await poll(DASHPAY_TIMEOUT_MS, "the sidebar's Contacts", () => wd.find(tid("sidebar-contacts")).then((e) => e ?? undefined));
    await wd.press(tid("sidebar-contacts"));
    await wd.waitFor(tid("page-contacts"), "the Contacts page");
    await wd.screenshot("1-contacts-opened");
    step("Contacts opened");

    // Every expected username listed in My Contacts, verified (no Unverified chip).
    const rows = await poll(DASHPAY_TIMEOUT_MS, `My Contacts to list ${EXPECT.join(", ")}, verified`, async () => {
      const rows = await wd.script<Row[]>(ROWS_SCRIPT);
      const mine = rows.filter((r) => r.group === "My Contacts");
      const ok = EXPECT.every((u) => mine.some((r) => (r.name === u || r.username === `@${u}`) && !r.chips.includes("Unverified")));
      return ok ? rows : undefined;
    });
    result.rows = rows;
    result.groups = [...new Set(rows.map((r) => r.group))];
    result.hint = await wd.script<string | null>(`return document.querySelector('${tid("contacts-setup-hint")}')?.textContent ?? null`);
    await wd.screenshot("2-contacts-list");
    step(`listed ${rows.length} row(s): ${rows.map((r) => `${r.group}: ${r.name}`).join("; ")}`);

    // The detail slot shows the first expected contact's identity.
    const first = await wd.script<string | null>(
      `const u = arguments[0];
       const row = [...document.querySelectorAll('[data-testid="contact-row"]')].find((r) =>
         r.querySelector('.contact-row-name')?.textContent === u || r.querySelector('.contact-row-username')?.textContent === '@' + u);
       if (!row) return null; row.click(); return row.dataset.contact;`,
      [EXPECT[0]],
    );
    const shown = await wd.text(await wd.waitFor(tid("contact-detail-identity"), "the detail slot"));
    if (!first || shown !== first) throw new Error(`the detail slot shows ${shown}, not the row's ${first}`);
    if (contactIds.length > 0 && !contactIds.includes(shown)) throw new Error(`${EXPECT[0]} is ${shown}, not a fixture identity`);
    result.selected = { username: EXPECT[0], identity: shown, relation: await wd.text(await wd.waitFor(tid("contact-detail-relation"))) };
    await wd.screenshot("3-contact-selected");
    step(`selected ${EXPECT[0]}: identity ${shown}`);

    // Another order, then back.
    await wd.press(`${tid("contacts-sort")} option[value="dateAdded"]`);
    await poll(TIMEOUT_MS, "Sort by Date added", async () =>
      (await wd.script<string>(`return document.querySelector('${tid("contacts-sort")}').value`)) === "dateAdded" ? true : undefined,
    );
    await wd.screenshot("4-sorted-by-date-added");
    step("sorted by Date added");
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
    const filtered = await wd.script<Row[]>(ROWS_SCRIPT);
    if (!filtered.some((r) => r.name === EXPECT[0] || r.username === `@${EXPECT[0]}`)) throw new Error(`the search “${prefix}” dropped ${EXPECT[0]}`);
    result.search = { prefix, rows: filtered.map((r) => r.name), network };
    await wd.screenshot("5-search");
    step(`searched “${prefix}”: ${filtered.length} row(s); network section: ${network.replace(/\s+/g, " ").slice(0, 200)}`);
  } catch (e) {
    failure = e;
    await wd.screenshot("failure").catch(() => undefined);
  } finally {
    await wd.end().catch(() => undefined);
  }

  stopDriver(driver);
  driver = undefined;
  await driverGone(started.driver);
  const found = leaks(OUT, [phrase, passphrase]);
  if (found.length > 0) {
    for (const f of found) rmSync(f, { force: true });
    const leak = `secrets in plain text: ${found.join(", ")}`;
    throw new Error(failure ? `${failure instanceof Error ? failure.message : String(failure)}; ${leak}` : leak);
  }
  if (failure) throw failure;
  step("no phrase or passphrase in OUT");
  writeFileSync(join(OUT, "result.json"), JSON.stringify({ ok: true, ...result, steps }, null, 2));
  console.log(`contacts: PASS (${steps.length} steps); screenshots in ${OUT}`);
}

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
