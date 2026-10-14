// End-to-end smoke of the real app on the real engine (U-07): tauri-driver drives the
// production renderer in WebKitGTK through WebKitWebDriver. No fixture is involved: a fresh
// regtest data root gets an encrypted vault and a wallet from dwcli, and the app opens it.
//
// Flow (U-07, U-01): launched with a `dash:` link to the wallet's address → lock screen → wrong
// passphrase refused → unlock → Pay, prefilled from the link → Home (zero balance, empty
// history) → Activity → a second launch with a `dashpay:` link hands it to the running app and
// exits → Contacts ▸ Add contact → a second launch with a `pay:` link → Pay → lock again.
// Screenshots and the app's log go to $DW_E2E_OUT.
//
// The second launches need the single-instance plugin's D-Bus session bus: run under
// `dbus-run-session` (scripts/e2e-tauri.sh does), never on a desktop's shared bus.
//
// Run on Linux under a private X display (CI: xvfb-run; agentbox: `agent-gui start N`), never
// a developer's display:
//   DW_APP=…/dash-wallet DWCLI=…/dwcli TAURI_DRIVER=…/tauri-driver node e2e-tauri/smoke.ts
// `scripts/e2e-tauri.sh` builds the binaries and runs it. Node 24 runs this file as is (type
// stripping); it uses W3C WebDriver over fetch and no other dependency.

import { spawnSync, type ChildProcess } from "node:child_process";
import { mkdirSync, mkdtempSync, readdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { APP, CHAIN, dwcli, env, poll, startDriver, stopDriver, tid, TIMEOUT_MS, WebDriver } from "./driver.ts";

const OUT = resolve(env("DW_E2E_OUT", "test-results/tauri-e2e"));
const PORT = Number(env("DW_E2E_PORT", "4744"));
const PASSPHRASE = "e2e smoke passphrase";
/** A Base58 identity id (32 bytes of 7) for the `dashpay://user` link. */
const IDENTITY = "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx";

// ------------------------------------------------------------------ steps

/** A second launch of the app on the same data root with `link`: it must hand the link to the
 * running app (the single-instance plugin) and exit at once instead of opening a window. */
function secondLaunch(dataRoot: string, link: string) {
  launchAndForward(["--datadir", dataRoot, link]);
}

function launchAndForward(args: string[], extraEnv: NodeJS.ProcessEnv = {}) {
  const env = { ...process.env, ...extraEnv };
  const r = spawnSync(APP, args, { encoding: "utf8", env, timeout: 60_000, killSignal: "SIGKILL" });
  if (r.error || r.status !== 0)
    throw new Error(`the second launch did not hand over its link and exit (status ${r.status}, ${r.error?.message ?? r.stderr.slice(-400)})`);
}

/** A second launch on the same data root that cannot hand its link over (no session bus, or a
 * malformed bus address): it must stop at the root's instance lock with a clear message, without
 * a panic and without opening the wallet (U-01 D2). */
function refusedLaunch(dataRoot: string, link: string, bus: Record<string, string>) {
  const env: NodeJS.ProcessEnv = { ...process.env, ...bus };
  if (!("DBUS_SESSION_BUS_ADDRESS" in bus)) delete env.DBUS_SESSION_BUS_ADDRESS;
  const r = spawnSync(APP, ["--datadir", dataRoot, link], { encoding: "utf8", env, timeout: 60_000, killSignal: "SIGKILL" });
  const tail = r.error?.message ?? r.stderr.slice(-600);
  if (r.status !== 1 || !r.stderr.includes("another Dash Wallet is already running"))
    throw new Error(`the second launch was not refused at the instance lock (status ${r.status}): ${tail}`);
  if (r.stderr.includes("panicked")) throw new Error(`the second launch panicked: ${tail}`);
  if (!r.stderr.includes("no D-Bus session bus")) throw new Error(`the missing bus was not logged: ${tail}`);
}

let driver: ChildProcess | undefined;
/** Holds the vault and its passphrase file: out of OUT, which CI uploads; removed at the end. */
let work: string | undefined;

async function main() {
  mkdirSync(OUT, { recursive: true });
  work = mkdtempSync(join(tmpdir(), "dw-e2e-")); // mode 0700
  // The default root's place under XDG_DATA_HOME=<work>/xdg, so a launch without --datadir can
  // meet this one (U-01 D2).
  const xdgData = join(work, "xdg");
  const dataRoot = join(xdgData, "org.dash.wallet.desktop");
  const passFile = join(work, "passphrase");
  writeFileSync(passFile, `${PASSPHRASE}\n`, { mode: 0o600 });

  dwcli(dataRoot, passFile, "init-vault");
  const walletId = /wallet_id (\S+)/.exec(dwcli(dataRoot, passFile, "create"))?.[1];
  if (!walletId) throw new Error("dwcli create printed no wallet id");
  const address = /address (\S+)/.exec(dwcli(dataRoot, passFile, "receive"))?.[1];
  if (!address) throw new Error("dwcli receive printed no address");
  console.log(`smoke: regtest data root ${dataRoot}, wallet ${walletId.slice(0, 16)}…`);

  const started = await startDriver(PORT, OUT);
  driver = started.driver;
  const wd = new WebDriver(started.base, OUT);
  const launchLink = `dash:${address}?amount=0.5&label=Launch%20link`;
  await wd.start({ alwaysMatch: { "tauri:options": { application: APP, args: ["--datadir", dataRoot, ...CHAIN, "--theme", "light", launchLink] } } });
  const value = (id: string) => wd.script<string>(`return document.querySelector('${tid(id)}')?.value ?? ""`);
  const waitValue = (id: string, want: string) =>
    poll(TIMEOUT_MS, `${id} to read ${want}`, async () => ((await value(id)) === want ? true : undefined));
  const steps: string[] = [];
  const step = (s: string) => {
    steps.push(s);
    console.log(`smoke: ${s}`);
  };
  try {
    // The engine opened the vault locked; the production renderer has no fixture.
    const field = await wd.waitFor(tid("lock-field"), "the lock screen");
    if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");
    await wd.screenshot("1-lock-screen");
    step("lock screen on the real vault");

    await wd.type(field, "not the passphrase");
    await wd.press(tid("lock-submit"));
    const error = await wd.waitFor("#lock-error:not(:empty)", "the wrong-passphrase message");
    if (!(await wd.text(error)).includes("incorrect")) throw new Error(`unexpected lock error: ${await wd.text(error)}`);
    step("wrong passphrase refused by the vault");

    await wd.fill(tid("lock-field"), PASSPHRASE);
    await wd.press(tid("lock-submit"));
    // The shell is rendered under the lock screen, so wait for the lock screen to go first.
    await wd.waitGone(tid("lock-screen"), "the lock screen");
    // The launch link was routed while locked: Pay is up, prefilled, once unlocked.
    await wd.waitFor(tid("page-pay"), "Pay, opened by the launch link");
    await waitValue("pay-address", address);
    await waitValue("pay-label", "Launch link");
    await wd.screenshot("2-pay-from-launch-link");
    step("launch link: Pay ▸ To an address, prefilled, after unlock");

    await wd.press(tid("sidebar-home"));
    await wd.waitFor(tid("hero"), "the Overview after unlock");
    await wd.waitFor(tid("history-empty"), "the empty history of a new wallet");
    await wd.screenshot("3-overview");
    step("unlocked: Overview of the new wallet, history empty");

    await wd.press(tid("sidebar-activity"));
    await wd.waitFor(tid("tx-empty"), "the Transactions page");
    await wd.screenshot("4-activity");
    step("Activity page");

    secondLaunch(dataRoot, `dashpay://user?id=${IDENTITY}&username=bob`);
    const username = await wd.waitFor(tid("add-contact-username"), "Add contact, from the forwarded dashpay: link");
    if ((await wd.text(username)) !== "@bob") throw new Error(`unexpected contact: ${await wd.text(username)}`);
    await wd.screenshot("5-add-contact-from-second-launch");
    step("second launch: dashpay: link forwarded, Contacts ▸ Add contact");

    secondLaunch(dataRoot, `pay:${address}?label=Forwarded`);
    await waitValue("pay-label", "Forwarded");
    await wd.screenshot("6-pay-from-second-launch");
    step("second launch: pay: link forwarded, Pay prefilled");

    // The same root through a symlinked parent: one instance name (the hash of the canonical root).
    const linked = join(work, "linked");
    symlinkSync(work, linked);
    secondLaunch(join(linked, "xdg", "org.dash.wallet.desktop"), `pay:${address}?label=Symlinked`);
    await waitValue("pay-label", "Symlinked");
    step("second launch through a symlinked parent: forwarded to the same instance");

    // The default root (no --datadir) is the same root as the --datadir the app runs on.
    launchAndForward([`pay:${address}?label=Default`], { XDG_DATA_HOME: xdgData });
    await waitValue("pay-label", "Default");
    step("second launch on the default root: forwarded to the --datadir instance of that root");

    // No session bus (and no fallback socket in an empty runtime directory), then a malformed bus
    // address: no plugin, so the second launch stops at the lock; the running app keeps its page.
    const runtime = mkdtempSync(join(work, "runtime-"));
    refusedLaunch(dataRoot, `pay:${address}?label=Refused`, { XDG_RUNTIME_DIR: runtime });
    refusedLaunch(dataRoot, `pay:${address}?label=Refused`, { DBUS_SESSION_BUS_ADDRESS: "nonsense", XDG_RUNTIME_DIR: runtime });
    // Refused before GTK started: no session bus autolaunched, nothing written to the runtime dir.
    const left = readdirSync(runtime);
    if (left.length > 0) throw new Error(`the refused launches left ${left.join(", ")} in their runtime dir`);
    await waitValue("pay-label", "Default");
    await wd.screenshot("7-after-refused-launches");
    step("second launch without a session bus: refused at the instance lock, no panic");

    await wd.press(tid("toolbar-lock"));
    await wd.waitFor(tid("lock-field"), "the lock screen after locking");
    await wd.screenshot("8-locked-again");
    step("locked again");
  } catch (e) {
    await wd.screenshot("failure").catch(() => undefined);
    throw e;
  } finally {
    await wd.end().catch(() => undefined);
  }
  writeFileSync(join(OUT, "result.json"), JSON.stringify({ ok: true, steps }, null, 2));
  console.log(`smoke: PASS (${steps.length} steps); screenshots in ${OUT}`);
}

main()
  .catch((e) => {
    console.error(`smoke: FAIL: ${e instanceof Error ? e.message : String(e)}`);
    process.exitCode = 1;
  })
  .finally(() => {
    stopDriver(driver);
    try {
      if (work) rmSync(work, { recursive: true, force: true, maxRetries: 3 });
    } catch (e) {
      console.warn(`smoke: could not remove ${work}: ${e instanceof Error ? e.message : String(e)}`);
    }
  });
