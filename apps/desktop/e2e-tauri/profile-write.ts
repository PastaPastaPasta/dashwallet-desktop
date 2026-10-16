// A DashPay profile edit from the real app on public testnet, timed against SPV's sync phases
// (BL-162): tauri-driver starts the production app on a fresh data root with a wallet of the
// testnet DashPay fixture restored (`--sync`), unlocks it, logs every sync-phase change and a
// profile read every DW_PROFILE_READ_EVERY_S, then edits the wallet's profile through the app's
// own IPC (`authorize` + `dashpay_update_profile`, as a profile editor would) at the phase
// DW_PROFILE_WRITE_AT names:
//
//   early    as soon as DashPay knows the identity (SPV still syncing)
//   synced   DW_PROFILE_AFTER_SYNCED_S seconds after the status bar says Synced and masternode
//            sync is done (default 60)
//   never    reads only: as `synced`, then ends without writing
//
//   DW_APP=… DWCLI=… TAURI_DRIVER=… DW_TESTNET_FIXTURE=<dir> node e2e-tauri/profile-write.ts
//
// Writes: ONE profile edit of wallet DW_PROFILE_WALLET (default A), harmless and repeatable
// (display name and public message DW_PROFILE_LABEL). Reads otherwise. Phrases go to dwcli on
// stdin, never argv. Set DW_LOG (EnvFilter) for the engine's log in OUT/tauri-driver.log.
//
// Files: every runtime file (data root, passphrase file, the app's XDG dirs and TMPDIR, OUT) lies
// in a new run directory under one scratch root, DW_PROFILE_ROOT (default $TMPDIR/dw-profile-write;
// never inside the checkout). DW_E2E_OUT, if set, must be a new directory under that root. Every
// mode ends the same way: result.json, then a scan of OUT and the app's XDG dirs and TMPDIR, whose
// leaked files are deleted, and of DW_E2E_SCAN_EXTRA (another log directory), which is only read.
// The data root and passphrase are deleted; OUT stays. `scripts/e2e-tauri-profile-write.sh` runs
// this file under a private session bus, on the private X display in $DISPLAY.

import { spawnSync, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import type { AuthGrant } from "../src/api/generated/AuthGrant";
import type { DashPayProfile } from "../src/api/generated/DashPayProfile";
import type { SyncStatus } from "../src/api/generated/SyncStatus";
import type { WalletSummary } from "../src/api/generated/WalletSummary";
import { APP, driverGone, DWCLI, env, leaks, poll, startDriver, stopDriver, tid, WebDriver } from "./driver.ts";
import { canonical, newDirIn, removeOwned, scratchRoot } from "./scratch.ts";

const FIXTURE = resolve(env("DW_TESTNET_FIXTURE"));
const WALLET = env("DW_PROFILE_WALLET", "A");
const WRITE_AT = env("DW_PROFILE_WRITE_AT", "synced");
const AFTER_SYNCED_S = Number(env("DW_PROFILE_AFTER_SYNCED_S", "60"));
const READ_EVERY_S = Number(env("DW_PROFILE_READ_EVERY_S", "20"));
const LABEL = env("DW_PROFILE_LABEL", `BL-162 ${new Date().toISOString().slice(0, 16)}`);
const BIRTH_HEIGHT = env("DW_PROFILE_BIRTH_HEIGHT", "1568806");
const PORT = Number(env("DW_E2E_PORT", "4770"));
/** Read-only: scanned for secrets, never written or swept. */
const SCAN_EXTRA = process.env.DW_E2E_SCAN_EXTRA ? canonical(process.env.DW_E2E_SCAN_EXTRA) : null;
/** A fresh testnet sync, headers to masternodes. */
const SYNC_TIMEOUT_MS = Number(env("DW_E2E_SYNC_TIMEOUT_MS", "1800000"));
/** The write: a Platform state transition and its proof. */
const WRITE_TIMEOUT_MS = Number(env("DW_E2E_WRITE_TIMEOUT_MS", "600000"));
const NETWORK = ["--network", "testnet"];

if (!["early", "synced", "never"].includes(WRITE_AT)) throw new Error(`DW_PROFILE_WRITE_AT=${WRITE_AT}: early, synced or never`);

let driver: ChildProcess | undefined;
/** This run's directory under the scratch root, and its output directory (below it by default). */
let run: string | undefined;
let OUT = "";
const secrets: string[] = [];

/** Creates the run directory and OUT, and points the app's XDG dirs and TMPDIR into the run. */
function layout() {
  const checkout = resolve(import.meta.dirname, "../../..");
  const root = scratchRoot(process.env.DW_PROFILE_ROOT ?? join(tmpdir(), "dw-profile-write"), checkout);
  run = mkdtempSync(join(root, "run-")); // mode 0700
  OUT = newDirIn(root, process.env.DW_E2E_OUT ?? join(run, "out"));
  for (const [name, dir] of [
    ["XDG_DATA_HOME", "home/data"],
    ["XDG_CACHE_HOME", "home/cache"],
    ["XDG_CONFIG_HOME", "home/config"],
    ["TMPDIR", "tmp"],
  ]) {
    process.env[name] = join(run, dir);
    mkdirSync(process.env[name], { recursive: true, mode: 0o700 });
  }
}

/** Every file of OUT and the app's XDG dirs and TMPDIR holding a secret is deleted; one under
 * DW_E2E_SCAN_EXTRA is reported and left. Returns all of them. */
function sweep(): string[] {
  if (!run) return [];
  const owned = [OUT, join(run, "home"), join(run, "tmp")].filter((d) => existsSync(d));
  const found = [...owned.flatMap((d) => leaks(d, secrets)), ...(SCAN_EXTRA && existsSync(SCAN_EXTRA) ? leaks(SCAN_EXTRA, secrets) : [])];
  removeOwned(found, [run, OUT]);
  return found;
}

/** Deletes the run's runtime files (data root, passphrase, XDG dirs, TMPDIR); OUT stays. */
function dropRuntime() {
  if (!run || !existsSync(run)) return;
  for (const name of readdirSync(run)) if (join(run, name) !== OUT) rmSync(join(run, name), { recursive: true, force: true, maxRetries: 3 });
  if (readdirSync(run).length === 0) rmSync(run, { recursive: true, force: true });
}

function mask(text: string): string {
  const words = [...new Set(secrets.flatMap((s) => s.split(/\s+/)))].filter(Boolean).sort((a, b) => b.length - a.length);
  if (!words.length) return text;
  const alternation = words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|");
  return text.replace(new RegExp(`(?<![\\w-])(?:${alternation})(?![\\w-])`, "g"), "***");
}

function dwcli(root: string, passFile: string, args: string[], input?: string): string {
  const r = spawnSync(DWCLI, ["--datadir", root, ...NETWORK, "--passphrase-file", passFile, ...args], { encoding: "utf8", input, timeout: 900_000 });
  if (r.status === 0) return r.stdout;
  throw new Error(`dwcli ${args.slice(0, 3).join(" ")} failed (${r.status}): ${mask(`${r.stderr}${r.stdout}`).slice(-800)}`);
}

function identityOf(wallet: string): string {
  const fixture = JSON.parse(readFileSync(join(FIXTURE, "fixture.json"), "utf8")) as Record<string, string | { identity_id?: string }>;
  const row = fixture[wallet];
  const id = typeof row === "string" ? row : row?.identity_id;
  if (!id) throw new Error(`the fixture names no identity for wallet ${wallet}`);
  return id;
}

/** An IPC call from the renderer's own context, as the app's screens make them. */
const INVOKE = `const [cmd, args, done] = arguments;
  window.__TAURI_INTERNALS__.invoke(cmd, args).then((ok) => done({ ok }), (err) => done({ err }));`;

interface Answer<T> {
  ok?: T;
  err?: unknown;
}

async function invoke<T>(wd: WebDriver, cmd: string, args: Record<string, unknown> = {}): Promise<Answer<T>> {
  return wd.scriptAsync<Answer<T>>(INVOKE, [cmd, args]);
}

interface WriteResult {
  at: string;
  phase: string;
  ms: number;
  ok: boolean;
  error: string | null;
  label: string;
  readBack?: string | null;
}

const phaseLine = (s: SyncStatus) => `${s.caughtUp ? "Synced" : "Syncing"} active=${s.activePhase ?? "-"} done=[${s.phases.filter((p) => p.done).map((p) => p.phase).join(",")}]`;
const masternodesDone = (s: SyncStatus) => s.caughtUp && s.phases.some((p) => p.phase === "masternodes" && p.done);

async function main() {
  layout();
  if (!run) throw new Error("no run directory");
  const appRoot = join(run, "app");
  const passFile = join(run, "passphrase");
  const passphrase = randomBytes(18).toString("base64url");
  const phrase = readFileSync(join(FIXTURE, `${WALLET}.mnemonic`), "utf8").trim();
  secrets.push(phrase, phrase.split(/\s+/).join(" "), passphrase);
  writeFileSync(passFile, `${passphrase}\n`, { mode: 0o600 });
  const steps: string[] = [];
  const t0 = Date.now();
  const step = (s: string) => {
    const line = `${new Date().toISOString()} +${((Date.now() - t0) / 1000).toFixed(1)}s ${s}`;
    steps.push(line);
    console.log(`profile-write: ${line}`);
  };
  const identity = identityOf(WALLET);
  let failure: unknown;
  let write: WriteResult | undefined;
  try {
    dwcli(appRoot, passFile, ["init-vault"]);
    dwcli(appRoot, passFile, ["import", "--birth-height", BIRTH_HEIGHT, "--name", `Fixture ${WALLET}`], `${phrase}\n`);
    step(`fresh testnet data root with fixture wallet ${WALLET}, birth ${BIRTH_HEIGHT}; write at ${WRITE_AT}; run ${run}, output ${OUT}`);
    const started = await startDriver(PORT, OUT, (d) => (driver = d));
    const wd = new WebDriver(started.base, OUT);
    try {
      await wd.start({ alwaysMatch: { "tauri:options": { application: APP, args: ["--datadir", appRoot, ...NETWORK, "--sync", "--theme", "light"] } } });
      await wd.call("POST", wd.s("/timeouts"), { script: WRITE_TIMEOUT_MS });
      await wd.fill(tid("lock-field"), passphrase);
      await wd.press(tid("lock-submit"));
      await wd.waitGone(tid("lock-screen"), "the lock screen");
      if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");
      const wallets = await invoke<WalletSummary[]>(wd, "list_wallets");
      const walletId = wallets.ok?.[0]?.walletId;
      if (!walletId) throw new Error(`no wallet: ${JSON.stringify(wallets.err)}`);
      step(`unlocked wallet ${WALLET} (${walletId.slice(0, 8)})`);

      let lastPhaseLine = "";
      let syncedAt: number | null = null;
      let lastRead = 0;
      let identityKnown = false;
      const observe = async () => {
        const s = await invoke<SyncStatus>(wd, "sync_status");
        if (s.ok) {
          const line = phaseLine(s.ok);
          if (line !== lastPhaseLine) step(`sync: ${line}`);
          lastPhaseLine = line;
          if (syncedAt === null && masternodesDone(s.ok)) syncedAt = Date.now();
        }
        if (Date.now() - lastRead >= READ_EVERY_S * 1000) {
          lastRead = Date.now();
          const t = Date.now();
          const p = await invoke<DashPayProfile | null>(wd, "dashpay_profile", { walletId, identity });
          identityKnown ||= p.ok !== undefined;
          step(`read profile in ${Date.now() - t} ms: ${p.err ? `error ${JSON.stringify(p.err)}` : JSON.stringify(p.ok?.displayName ?? null)}`);
        }
      };
      await poll(SYNC_TIMEOUT_MS, `the write phase (${WRITE_AT})`, async () => {
        await observe();
        const ready = WRITE_AT === "early" ? identityKnown : syncedAt !== null && Date.now() - syncedAt >= AFTER_SYNCED_S * 1000;
        return ready || undefined;
      });
      await wd.screenshot("1-before-write");
      if (WRITE_AT === "never") step("reads only: no write");
      else write = await writeProfile(wd, { walletId, identity, passphrase, phase: lastPhaseLine }, step);
    } catch (e) {
      failure = e;
      await wd.screenshot("failure").catch(() => undefined);
    } finally {
      await wd.end().catch(() => undefined);
    }
  } catch (e) {
    failure ??= e;
  } finally {
    stopDriver(driver);
    await driverGone(driver);
    driver = undefined;
  }
  // Every mode ends here. result.json first: the scan covers it.
  writeFileSync(join(OUT, "result.json"), JSON.stringify({ wallet: WALLET, identity, writeAt: WRITE_AT, write: write ?? null, failure: failure ? mask(String(failure)) : null, steps }, null, 2));
  const found = sweep();
  dropRuntime();
  if (found.length > 0) {
    const leak = `secrets in plain text: ${found.join(", ")}`;
    throw new Error(failure ? `${failure instanceof Error ? failure.message : String(failure)}; ${leak}` : leak);
  }
  if (failure) throw failure;
  step(`no phrase or passphrase in OUT, the app's XDG dirs or TMPDIR${SCAN_EXTRA ? `, or ${SCAN_EXTRA}` : ""}`);
  if (WRITE_AT !== "never" && !write?.ok) throw new Error("the write failed");
  console.log(`profile-write: PASS (${steps.length} steps, ${WRITE_AT === "never" ? "reads only" : "one write"}); output in ${OUT}`);
}

/** The one profile edit, under a single-use grant, then a read back. */
async function writeProfile(
  wd: WebDriver,
  { walletId, identity, passphrase, phase }: { walletId: string; identity: string; passphrase: string; phase: string },
  step: (s: string) => void,
): Promise<WriteResult> {
  const grant = await invoke<AuthGrant>(wd, "authorize", {
    purpose: { kind: "platformOp", maxDuffs: 0, maxCredits: "300000000" },
    walletId,
    credential: { kind: "passphrase", passphrase },
  });
  if (!grant.ok) throw new Error(`authorize: ${JSON.stringify(grant.err)}`);
  const edit = { displayName: LABEL, publicMessage: `${LABEL}, written ${WRITE_AT}`, avatar: { kind: "keep" } };
  step(`writing the profile (${phase})`);
  const t = Date.now();
  const r = await invoke<null>(wd, "dashpay_update_profile", { walletId, identity, edit, grant: grant.ok.id });
  const ms = Date.now() - t;
  const ok = r.err === undefined;
  const write: WriteResult = { at: WRITE_AT, phase, ms, ok, error: ok ? null : mask(JSON.stringify(r.err)), label: LABEL };
  step(`write ${ok ? "OK" : `FAILED ${write.error}`} in ${ms} ms`);
  await wd.screenshot("2-after-write");
  if (ok) {
    const back = await invoke<DashPayProfile | null>(wd, "dashpay_profile", { walletId, identity });
    step(`read back: ${JSON.stringify(back.ok?.displayName ?? back.err)}`);
    write.readBack = back.ok?.displayName ?? null;
  }
  return write;
}

async function interrupted(signal: NodeJS.Signals) {
  console.error(`profile-write: FAIL: ${signal}`);
  stopDriver(driver);
  await driverGone(driver);
  sweep();
  dropRuntime();
  process.exit(1);
}
process.once("SIGINT", (s) => void interrupted(s));
process.once("SIGTERM", (s) => void interrupted(s));

main()
  .catch((e) => {
    console.error(`profile-write: FAIL: ${mask(e instanceof Error ? e.message : String(e))}`);
    process.exitCode = 1;
  })
  .finally(() => {
    stopDriver(driver);
    dropRuntime();
  });
