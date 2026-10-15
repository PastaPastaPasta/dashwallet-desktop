// The DashPay status surfaces on the real app, the real engine and public testnet (DP1-08 GL
// tier): tauri-driver drives the production renderer in WebKitGTK. A fresh data root restores a
// wallet of the testnet DashPay fixture (an identity with a username), the app syncs (`--sync`:
// SPV and DashPay's bring-up), and the window is read every half second meanwhile: each Home
// card state and identity chip text it passes through is recorded, with a screenshot of each
// new card state. At the end the chip names the wallet's main username and Home shows no card
// (DP1-08 D2). Reads only: nothing is sent or registered.
//
//   DW_APP=… DWCLI=… TAURI_DRIVER=… DW_TESTNET_FIXTURE=<dir> node e2e-tauri/dashpay-status.ts
//
// <dir> holds `<W>.mnemonic` (0600) and `fixture.json` (the identities); DW_STATUS_WALLET picks
// W (default A). The phrase goes to dwcli on stdin, never argv; whatever happens, no file of OUT
// may hold it or the passphrase afterwards. `scripts/e2e-tauri-dashpay-status.sh` builds the
// binaries and runs this file under a private session bus, on the private X display in $DISPLAY.

import { spawnSync, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { APP, driverGone, DWCLI, env, leaks, poll, startDriver, stopDriver, tid, WebDriver } from "./driver.ts";

const FIXTURE = resolve(env("DW_TESTNET_FIXTURE"));
const WALLET = env("DW_STATUS_WALLET", "A");
const BIRTH_HEIGHT = env("DW_STATUS_BIRTH_HEIGHT", "1568806");
const OUT = resolve(env("DW_E2E_OUT", "test-results/tauri-dashpay-status"));
const PORT = Number(env("DW_E2E_PORT", "4765"));
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
  // Whole words only: a phrase word inside a longer one ("air" in "repair") stays, so a mask does
  // not tell which words the phrase holds.
  const alternation = words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|");
  const masked = words.length ? r.stderr.replace(new RegExp(`(?<![\\w-])(?:${alternation})(?![\\w-])`, "g"), "***") : r.stderr;
  throw new Error(`dwcli ${args[0]} failed (${r.status}): ${masked.slice(-600)}`);
}

/** What the window shows of DashPay: the Home card's state and title, the chip's text, the route. */
interface Seen {
  card: string | null;
  title: string | null;
  chip: string | null;
}

const SEEN_SCRIPT = `
  const card = document.querySelector('[data-testid="dashpay-card"]');
  return {
    card: card?.dataset.state ?? null,
    title: card?.querySelector('h2')?.textContent ?? null,
    chip: document.querySelector('[data-testid="toolbar-chip"]')?.textContent ?? null,
  };`;

async function run(passphrase: string, wd: WebDriver, step: (s: string) => void): Promise<Record<string, unknown>> {
  const fixture = JSON.parse(readFileSync(join(FIXTURE, "fixture.json"), "utf8")) as Record<string, { expected_main_name?: string }>;
  const expected = fixture[WALLET]?.expected_main_name;
  if (!expected) throw new Error(`fixture.json names no expected_main_name for wallet ${WALLET}`);

  await wd.fill(tid("lock-field"), passphrase);
  await wd.press(tid("lock-submit"));
  await wd.waitGone(tid("lock-screen"), "the lock screen");
  if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");
  await wd.waitFor(tid("history-header"), "Home");
  step("unlocked the restored wallet, on Home");

  // Every change of the card or the chip, in order, until the chip names the main username and
  // Home shows no card (S16).
  const timeline: (Seen & { at: string })[] = [];
  const shot = new Set<string>();
  const done = await poll(DASHPAY_TIMEOUT_MS, `the chip to read @${expected} with no Home card`, async () => {
    const seen = await wd.script<Seen>(SEEN_SCRIPT);
    const last = timeline.at(-1);
    if (!last || last.card !== seen.card || last.title !== seen.title || last.chip !== seen.chip) {
      timeline.push({ ...seen, at: new Date().toISOString() });
      step(`card ${seen.card ?? "none"}${seen.title ? ` (“${seen.title}”)` : ""}, chip ${seen.chip ?? "none"}`);
      if (seen.card && !shot.has(seen.card)) {
        shot.add(seen.card);
        await wd.screenshot(`card-${seen.card}`);
      }
    }
    return seen.chip?.includes(`@${expected}`) && seen.card === null ? seen : undefined;
  });
  await wd.screenshot("home-ready");
  step(`ready: chip ${done.chip}, no card`);
  // D21: a wallet with a username is never asked to choose one, not even while its names load.
  if (shot.has("noUsername")) throw new Error("Home asked for a username on a wallet that has one");
  // The chip opens My Profile for an identity; the Join page is not offered.
  if (await wd.find(tid("sidebar-joinDashPay"))) throw new Error("the sidebar still offers Join DashPay");
  return { expectedMainName: expected, timeline, cardStatesSeen: [...shot] };
}

async function main() {
  mkdirSync(OUT, { recursive: true });
  work = mkdtempSync(join(tmpdir(), "dw-dashpay-status-")); // mode 0700
  const dataRoot = join(work, "app");
  const passFile = join(work, "passphrase");
  const passphrase = randomBytes(18).toString("base64url");
  const phrase = readFileSync(join(FIXTURE, `${WALLET}.mnemonic`), "utf8").trim();
  secrets.push(...new Set([phrase, phrase.split(/\s+/).join(" "), passphrase]));
  writeFileSync(passFile, `${passphrase}\n`, { mode: 0o600 });
  const steps: string[] = [];
  const step = (s: string) => {
    steps.push(`${new Date().toISOString()} ${s}`);
    console.log(`dashpay-status: ${s}`);
  };

  let failure: unknown;
  let result: Record<string, unknown> = {};
  try {
    dwcli(dataRoot, passFile, ["init-vault"]);
    dwcli(dataRoot, passFile, ["import", "--birth-height", BIRTH_HEIGHT, "--name", `Fixture ${WALLET}`], `${phrase}\n`);
    step(`testnet data root with fixture wallet ${WALLET}, birth ${BIRTH_HEIGHT}`);
    const started = await startDriver(PORT, OUT, (d) => (driver = d));
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
  console.log(`dashpay-status: PASS (${steps.length} steps); screenshots in ${OUT}`);
}

/** Interrupted (Ctrl-C, an outer timeout): stop the app and wait for it, delete the vault and
 * passphrase, and delete any file of OUT holding a secret, as a normal exit does. A signal that
 * lands while dwcli runs waits for it (`spawnSync`). */
async function interrupted(signal: NodeJS.Signals) {
  console.error(`dashpay-status: FAIL: ${signal}`);
  stopDriver(driver);
  await driverGone(driver);
  try {
    if (work) rmSync(work, { recursive: true, force: true, maxRetries: 3 });
  } catch (e) {
    console.error(`dashpay-status: could not remove ${work}: ${e instanceof Error ? e.message : String(e)}`);
  }
  for (const f of leaks(OUT, secrets)) rmSync(f.replace(/ \(\d+\)$/, ""), { force: true });
  process.exit(1);
}
process.once("SIGINT", (s) => void interrupted(s));
process.once("SIGTERM", (s) => void interrupted(s));

main()
  .catch((e) => {
    console.error(`dashpay-status: FAIL: ${e instanceof Error ? e.message : String(e)}`);
    process.exitCode = 1;
  })
  .finally(() => {
    stopDriver(driver);
    try {
      if (work) rmSync(work, { recursive: true, force: true, maxRetries: 3 });
    } catch (e) {
      console.warn(`dashpay-status: could not remove ${work}: ${e instanceof Error ? e.message : String(e)}`);
    }
  });
