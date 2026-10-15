// Settings on the real app, the real engine and a dashd regtest node (U-04 T1). tauri-driver
// drives the production renderer in WebKitGTK; SPV syncs from the node. No fixture is involved.
//
// Flow: dwcli makes an encrypted vault and a wallet in the app's data root, dashd funds it ▸ the
// app opens locked, unlocks, the balance arrives through SPV ▸ General: Dark, mDASH with three
// digits on Home (QT-139, QT-020), subtract fee by default in Pay (QT-137) ▸ Security: a wrong
// current passphrase counts, then the change (QT-111); payment authentication off and on through
// its sheet (IOS-016); autohide (IOS-020) ▸ Wallets: rename (IOS-110), the account xpub
// (IOS-111) makes a watch-only wallet (QT-114), which closes and stays closed (QT-101) ▸
// Backups: a backup and a dumpwallet export are files on disk (QT-110, QT-109) ▸ relaunch:
// settings.json kept the unit, digits, theme and autohide (D2, D14), the closed wallet stayed
// closed, the new passphrase unlocks ▸ the watch-only wallet opens and is removed ▸ Wipe with
// the typed sentence (IOS-009): every wallet goes, the vault's deletion fails (a blocker in its
// directory) and the sheet says so with onboarding held back; Try again deletes it, then
// onboarding, no wallet, no vault. At the end, pass or fail, no
// file of the data root (the plain-text export, which says so, is deleted once checked) or of
// OUT holds the phrase or a passphrase in plain text.
//
// Settings is opened with its menu shortcut (Ctrl+,), pressed through xdotool.
// `scripts/e2e-tauri-settings.sh` brings up the node, builds the binaries and runs this file:
//   DW_APP=… DWCLI=… TAURI_DRIVER=… DW_REGTEST_SH=…/regtest.sh DW_REGTEST_P2P=127.0.0.1:<port> \
//     node e2e-tauri/settings.ts

import { spawnSync, type ChildProcess } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import type { LockStatus } from "../src/api/generated/LockStatus";
import type { UiSettings } from "../src/api/generated/UiSettings";
import type { WalletLoadStatus } from "../src/api/generated/WalletLoadStatus";
import type { WalletSummary } from "../src/api/generated/WalletSummary";
import { APP, CHAIN, driverGone, DWCLI, env, poll, startDriver, stopDriver, tid, TIMEOUT_MS, WebDriver } from "./driver.ts";

const REGTEST = resolve(env("DW_REGTEST_SH"));
const P2P = env("DW_REGTEST_P2P");
const OUT = resolve(env("DW_E2E_OUT", "test-results/tauri-settings"));
const PORT = Number(env("DW_E2E_PORT", "4755"));
const SYNC_TIMEOUT_MS = Number(env("DW_E2E_SYNC_TIMEOUT_MS", "300000"));
const PASSPHRASE = "U-04 first passphrase 3#";
const NEW_PASSPHRASE = "U-04 changed passphrase 8%";
const AMOUNT = "1.5";

let driver: ChildProcess | undefined;
/** The data root and the passphrase file: out of OUT, which CI uploads; removed at the end. */
let work: string | undefined;

function run(cmd: string, args: string[]): string {
  const r = spawnSync(cmd, args, { encoding: "utf8" });
  if (r.status !== 0) throw new Error(`${cmd} ${args[0]} failed (${r.status}): ${r.stderr.slice(-600)}`);
  return r.stdout;
}

/** A menu shortcut through the X server (the menu bar owns it); the keys are always released. */
function shortcut(keys: string) {
  try {
    run("xdotool", ["search", "--sync", "--onlyvisible", "--name", "^Dash Wallet", "windowfocus", "key", "--clearmodifiers", keys]);
  } finally {
    run("xdotool", ["keyup", keys]);
  }
}

/** Every file under `dir` that holds one of `secrets` as plain text. */
function leaks(dir: string, secrets: string[]): string[] {
  const found: string[] = [];
  const walk = (d: string) => {
    for (const name of readdirSync(d)) {
      const p = join(d, name);
      const st = statSync(p, { throwIfNoEntry: false });
      if (!st) continue;
      if (st.isDirectory()) walk(p);
      else if (st.isFile()) {
        const text = readFileSync(p).toString("latin1");
        for (const s of secrets) if (text.includes(Buffer.from(s, "utf8").toString("latin1"))) found.push(`${p} (${secrets.indexOf(s)})`);
      }
    }
  };
  walk(dir);
  return found;
}

async function main() {
  mkdirSync(OUT, { recursive: true });
  work = mkdtempSync(join(tmpdir(), "dw-settings-")); // mode 0700
  const appRoot = join(work, "app");
  const passFile = join(work, "passphrase");
  writeFileSync(passFile, `${PASSPHRASE}\n`, { mode: 0o600 });

  // The app's own vault and wallet, made before it starts; dashd funds the wallet.
  const dwcli = (...args: string[]) => run(DWCLI, ["--datadir", appRoot, ...CHAIN, "--passphrase-file", passFile, ...args]);
  dwcli("init-vault");
  const phrase = /^mnemonic (.+)$/m.exec(dwcli("create"))?.[1]?.trim();
  if (!phrase || phrase.split(" ").length !== 12) throw new Error("dwcli create printed no 12-word phrase");
  const address = /address (\S+)/.exec(dwcli("receive"))?.[1];
  if (!address) throw new Error("dwcli receive printed no address");
  const txid = run("sh", [REGTEST, "fund", address, AMOUNT]).trim().split("\n").pop()!;
  run("sh", [REGTEST, "mine", "2"]);
  console.log(`settings: dashd funded ${address} with ${AMOUNT} DASH (${txid.slice(0, 16)}…)`);

  const started = await startDriver(PORT, OUT);
  driver = started.driver;
  const wd = new WebDriver(started.base, OUT);
  // No --theme: the saved appearance is the one in use.
  const launch = () =>
    wd.start({ alwaysMatch: { "tauri:options": { application: APP, args: ["--datadir", appRoot, ...CHAIN, "--sync", "--spv-peer", P2P] } } });

  const steps: string[] = [];
  const step = (s: string) => {
    steps.push(s);
    console.log(`settings: ${s}`);
  };
  const textOf = async (css: string) => wd.text(await wd.waitFor(css));
  const waitText = (css: string, want: RegExp, what: string, ms = TIMEOUT_MS) =>
    poll(ms, what, async () => {
      const t = await wd.script<string>(`return document.querySelector(arguments[0])?.textContent ?? ""`, [css]);
      return want.test(t) ? t : undefined;
    });
  /** Types a secret and checks by length only that every key arrived (up to three tries). */
  const enter = async (id: string, text: string) => {
    for (let i = 0; i < 3; i++) {
      await wd.clear(await wd.waitFor(tid(id)));
      await wd.fill(tid(id), text);
      if ((await wd.script<number>(`return document.querySelector(arguments[0])?.value.length ?? -1`, [tid(id)])) === text.length) return;
    }
    throw new Error(`${id} did not take ${text.length} characters`);
  };
  /** Chooses an option of a <select> as its change event reports it to React. */
  const choose = (id: string, value: string) =>
    wd.script(
      `const el = document.querySelector(arguments[0]);
       Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set.call(el, arguments[1]);
       el.dispatchEvent(new Event("change", { bubbles: true }));`,
      [tid(id), value],
    );
  const checked = (id: string) => wd.script<boolean>(`return !!document.querySelector(arguments[0])?.checked`, [tid(id)]);
  const invoke = async <T>(command: string, args: Record<string, unknown> = {}): Promise<T> => {
    const r = await wd.scriptAsync<{ ok?: T; err?: string }>(
      `const done = arguments[arguments.length - 1];
       window.__TAURI_INTERNALS__.invoke(arguments[0], arguments[1]).then((ok) => done({ ok }), (e) => done({ err: e?.code ?? String(e) }));`,
      [command, args],
    );
    if (r.err !== undefined) throw new Error(`${command}: ${r.err}`);
    return r.ok as T;
  };
  const loadStates = () => invoke<WalletLoadStatus[]>("wallet_load_states");
  const unlock = async (passphrase: string) => {
    await wd.waitFor(tid("lock-field"), "the lock screen");
    await enter("lock-field", passphrase);
    await wd.press(tid("lock-submit"));
    await wd.waitGone(tid("lock-screen"), "the lock screen");
  };
  const tab = async (name: string) => {
    if (!(await wd.find(tid("page-settings")))) shortcut("ctrl+comma");
    await wd.press(tid(`settings-tabs-${name}`));
  };
  const heroText = () => wd.script<string>(`return document.querySelector('[data-testid="hero-amount"]')?.textContent ?? ""`);

  await launch();
  let failure: unknown = null;
  let exportPath: string | null = null;
  try {
    // ---- the vault dwcli made: locked at start, the balance through SPV
    await unlock(PASSPHRASE);
    if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");
    const [main] = await invoke<WalletSummary[]>("list_wallets");
    if (!main) throw new Error("list_wallets is empty");
    const row = `settings-wallet-${main.walletId.slice(0, 8)}`;
    await poll(SYNC_TIMEOUT_MS, `the balance (${AMOUNT} DASH) through SPV`, async () => {
      const title = await wd.script<string>(`return document.querySelector('[data-testid="hero-amount"] .amount')?.title || ""`);
      return title.includes("1.50000000") ? title : undefined;
    });
    await wd.screenshot("01-home");
    step(`unlocked; ${main.name} holds ${AMOUNT} DASH through SPV`);

    // ---- General: appearance, unit, digits, subtract fee (QT-135, QT-139, QT-020, QT-137)
    await tab("general");
    await wd.press(tid("settings-theme-dark"));
    await poll(TIMEOUT_MS, "the dark appearance", async () => ((await wd.script<string>("return document.documentElement.dataset.theme ?? ''")) === "dark" ? true : undefined));
    await wd.press(tid("settings-unit-milliDash"));
    await choose("settings-digits", "3");
    await wd.press(tid("settings-subtract-fee"));
    await poll(TIMEOUT_MS, "the saved settings", async () => {
      const s = await invoke<UiSettings>("ui_settings");
      return s.unit === "milliDash" && s.decimalDigits === 3 && s.subtractFeeByDefault && s.theme === "dark" ? s : undefined;
    });
    await wd.screenshot("02-general");
    await wd.press(tid("sidebar-home"));
    await waitText(tid("hero-amount"), /^1500\.000\s?mtDASH$/, "1500.000 mtDASH on Home");
    await wd.press(tid("sidebar-pay"));
    await wd.press(tid("pay-tabs-address"));
    if (!(await checked("pay-subtract-fee"))) throw new Error("Pay does not start with Subtract fee");
    await waitText(`${tid("pay-address-form")}`, /Amount \(mtDASH\)/, "the amount field in mtDASH");
    step("General: Dark, mDASH with 3 digits (Home 1500.000 mtDASH), Pay starts with Subtract fee; ui_settings agrees");

    // ---- Security: change passphrase, payment authentication, autohide (QT-111, IOS-016, IOS-020)
    await tab("security");
    await wd.press(tid("settings-change-passphrase"));
    await enter("settings-change-current", "not the passphrase");
    await enter("settings-change-new", NEW_PASSPHRASE);
    await enter("settings-change-repeat", NEW_PASSPHRASE);
    await wd.press(tid("settings-change-submit"));
    await waitText(tid("settings-change-error"), /incorrect.*7 attempts remaining/, "a wrong current passphrase counted");
    await enter("settings-change-current", PASSPHRASE);
    await enter("settings-change-new", NEW_PASSPHRASE);
    await enter("settings-change-repeat", NEW_PASSPHRASE);
    await wd.press(tid("settings-change-submit"));
    await wd.waitFor(tid("settings-security-notice"), "the passphrase-changed notice");
    if (!(await checked("settings-payment-auth"))) throw new Error("payment authentication is not on by default");
    await wd.press(tid("settings-payment-auth"));
    await enter("settings-payment-auth-passphrase-field", PASSPHRASE);
    await wd.press(tid("settings-payment-auth-passphrase-submit"));
    await waitText(tid("settings-payment-auth-passphrase-error"), /incorrect/, "the old passphrase refused");
    await enter("settings-payment-auth-passphrase-field", NEW_PASSPHRASE);
    await wd.press(tid("settings-payment-auth-passphrase-submit"));
    await wd.waitGone(tid("settings-payment-auth-sheet"), "the payment authentication sheet");
    if ((await invoke<boolean>("payment_authentication")) !== false) throw new Error("payment_authentication is still on");
    await wd.press(tid("settings-payment-auth"));
    await enter("settings-payment-auth-passphrase-field", NEW_PASSPHRASE);
    await wd.press(tid("settings-payment-auth-passphrase-submit"));
    await wd.waitGone(tid("settings-payment-auth-sheet"), "the payment authentication sheet");
    if ((await invoke<boolean>("payment_authentication")) !== true) throw new Error("payment_authentication is not back on");
    await wd.press(tid("settings-autohide"));
    await wd.screenshot("03-security");
    step("Security: a wrong current passphrase counted, then changed (the old one refused after); payment authentication off and on; autohide on");

    // ---- Wallets: rename, xpub, watch-only, close (IOS-110, IOS-111, QT-114, QT-101)
    await tab("wallets");
    await wd.press(tid(`${row}-rename`));
    await enter("settings-rename-field", "Everyday");
    await wd.press(tid("settings-rename-save"));
    await poll(TIMEOUT_MS, "the new name", async () => ((await invoke<WalletSummary[]>("list_wallets"))[0]?.name === "Everyday" ? true : undefined));
    await wd.press(tid(`${row}-xpub`));
    await wd.waitFor(tid("settings-xpub-qr"), "the xpub QR code");
    const xpub = (await textOf(`${tid("settings-xpub-key")} .mono`)).trim();
    if (!/^tpub/.test(xpub)) throw new Error(`the account key is not a tpub: ${xpub.slice(0, 8)}`);
    await wd.screenshot("04-xpub");
    await wd.press(`${tid("settings-xpub-sheet")} .sheet-close`);
    await wd.waitGone(tid("settings-xpub-sheet"), "the xpub sheet");
    await wd.press(tid("settings-add-watch-only"));
    await wd.fill(tid("settings-watch-only-xpub"), xpub);
    await wd.fill(tid("settings-watch-only-name"), "Watcher");
    await wd.press(tid("settings-watch-only-add"));
    const watcher = await poll(TIMEOUT_MS, "the watch-only wallet", async () => (await loadStates()).find((w) => w.name === "Watcher" && w.watchOnly));
    const watcherRow = `settings-wallet-${watcher.walletId.slice(0, 8)}`;
    await wd.press(tid(`${watcherRow}-close`));
    await waitText(tid(`${watcherRow}-state`), /^Closed$/, "Watcher closed");
    const closed = (await loadStates()).find((w) => w.walletId === watcher.walletId);
    if (!closed || closed.loaded || closed.loadOnStartup) throw new Error(`wallet_load_states after Close: ${JSON.stringify(closed)}`);
    await wd.screenshot("05-wallets");
    step("Wallets: renamed Everyday; the account tpub made the watch-only Watcher; Close leaves it unloaded and off the startup list");

    // ---- Backups: a backup and a dumpwallet export on disk (QT-110, QT-109)
    await tab("backups");
    await wd.press(tid("settings-backup"));
    await wd.press(tid("settings-backup-write"));
    const backupPath = (await textOf(`${tid("settings-backup-path")} .mono`)).trim();
    if (!existsSync(backupPath) || !backupPath.startsWith(appRoot)) throw new Error(`no backup file at ${backupPath}`);
    await wd.press(`${tid("settings-backup-sheet")} .sheet-close`);
    await wd.waitGone(tid("settings-backup-sheet"), "the backup sheet");
    await wd.press(tid("settings-export"));
    await enter("settings-export-passphrase-field", NEW_PASSPHRASE);
    await wd.press(tid("settings-export-passphrase-submit"));
    exportPath = (await textOf(`${tid("settings-export-path")} .mono`)).trim();
    const dump = existsSync(exportPath) ? readFileSync(exportPath, "utf8") : "";
    if (!dump.startsWith("# Wallet dump")) throw new Error(`the export at ${exportPath} is not a dumpwallet file`);
    await wd.screenshot("06-export-done");
    await wd.press(`${tid("settings-export-sheet")} .sheet-close`);
    // The export is plain text by design (the sheet says so): it leaves before the leak check.
    rmSync(exportPath);
    step(`Backups: ${backupPath.slice(appRoot.length)} written; the dumpwallet export is a "# Wallet dump" file`);

    // ---- relaunch: the settings file, the closed wallet, the new passphrase (D2, D14, QT-101)
    await wd.end();
    const saved = JSON.parse(readFileSync(join(appRoot, "settings.json"), "utf8")) as Record<string, unknown>;
    if (saved.unit !== "milliDash" || saved.decimalDigits !== 3 || saved.theme !== "dark" || saved.autohideBalance !== true)
      throw new Error(`settings.json after the run: ${JSON.stringify(saved)}`);
    await launch();
    await unlock(NEW_PASSPHRASE);
    await waitText(tid("hero-amount"), /^#\.###\s?mtDASH$/, "the balance hidden at start (#.### mtDASH)");
    if ((await wd.script<string>("return document.documentElement.dataset.theme ?? ''")) !== "dark") throw new Error("the relaunch is not dark");
    const after = await loadStates();
    if (after.find((w) => w.walletId === watcher.walletId)?.loaded !== false) throw new Error("Watcher was loaded at start");
    await wd.screenshot("07-relaunched");
    step(`relaunch: settings.json kept mDASH, 3 digits, Dark and autohide (Home ${await heroText()}); Watcher stayed closed; the new passphrase unlocks`);

    // ---- open and remove the watch-only wallet (IOS-110)
    await tab("wallets");
    await wd.press(tid(`${watcherRow}-open`));
    await waitText(tid(`${watcherRow}-state`), /^Open$/, "Watcher open");
    await wd.press(tid(`${watcherRow}-remove`));
    await enter("settings-remove-passphrase-field", NEW_PASSPHRASE);
    await wd.press(tid("settings-remove-passphrase-submit"));
    await wd.waitGone(tid(watcherRow), "Watcher's row");
    if ((await loadStates()).some((w) => w.walletId === watcher.walletId)) throw new Error("wallet_load_states still lists Watcher");
    step("Wallets: Watcher opened again, then removed with the passphrase");

    // ---- wipe with the typed sentence (IOS-009)
    await tab("security");
    // The engine's own deletion fails first (D33): a directory where dw-vault's remove_all expects
    // a "replaced" copy of the vault file cannot be unlinked. Wallet removals never touch it.
    const vaultDir = join(appRoot, "regtest", "vault");
    if (!existsSync(join(vaultDir, "vault.dwv"))) throw new Error(`no vault file in ${vaultDir}`);
    const blocker = join(vaultDir, "vault.dwv.replaced-t1-blocker");
    mkdirSync(blocker);
    await wd.press(tid("settings-wipe"));
    // The sentence to type is the field's label ("Type: …"), as the user reads it.
    const sentence = await wd.script<string>(`return document.querySelector(arguments[0]).labels[0].textContent.replace(/^Type: /, "")`, [tid("settings-wipe-sentence")]);
    await enter("settings-wipe-sentence", sentence);
    await wd.press(tid("settings-wipe-continue"));
    await enter("settings-wipe-passphrase-field", NEW_PASSPHRASE);
    await wd.press(tid("settings-wipe-passphrase-submit"));
    const kept = await waitText(tid("settings-wipe-error"), /the vault could not be deleted/, "the vault's failed deletion on the sheet");
    if ((await invoke<WalletSummary[]>("list_wallets")).length !== 0) throw new Error("the wallets were not removed before the vault");
    // The wallet list is empty, yet onboarding waits: the failure is on screen, not under it.
    if (await wd.find(tid("onboarding"))) throw new Error("onboarding covers the failed vault deletion");
    await wd.screenshot("08-vault-kept");
    step(`Wipe: the typed sentence and the passphrase; every wallet removed, the vault's deletion fails and the sheet says so ("${kept.trim()}"), no onboarding`);

    rmSync(blocker, { recursive: true });
    await wd.press(tid("settings-wipe-retry"));
    await enter("settings-wipe-passphrase-field", NEW_PASSPHRASE);
    await wd.press(tid("settings-wipe-passphrase-submit"));
    await wd.waitFor(tid("onboarding-welcome"), "onboarding after the wipe");
    // Onboarding opens only once the wipe has seen lock_status read noVault (D33).
    const vault = await invoke<LockStatus>("lock_status");
    if (vault.state !== "noVault") throw new Error(`onboarding opened with the vault ${vault.state}`);
    await wd.screenshot("09-wiped");
    const left = await invoke<WalletSummary[]>("list_wallets");
    if (left.length !== 0) throw new Error(`after the wipe: ${left.length} wallets`);
    step("Wipe: Try again deletes the vault; onboarding opens with no wallet and no vault");
  } catch (e) {
    failure = e;
    await wd.screenshot("failure").catch(() => undefined);
  } finally {
    await wd.end().catch(() => undefined);
    if (exportPath) rmSync(exportPath, { force: true });
  }

  stopDriver(driver);
  driver = undefined;
  await driverGone(started.driver);
  const secrets = [phrase, PASSPHRASE, NEW_PASSPHRASE];
  const found = [...leaks(appRoot, secrets), ...leaks(OUT, secrets)];
  if (found.length > 0) {
    for (const f of found) if (f.startsWith(OUT)) rmSync(f.replace(/ \(\d+\)$/, ""), { force: true });
    const leak = `secrets in plain text: ${found.join(", ")}`;
    throw new Error(failure ? `${failure instanceof Error ? failure.message : String(failure)}; ${leak}` : leak);
  }
  if (failure) throw failure;
  step("no phrase or passphrase in the data root, the logs or the other files of OUT");

  writeFileSync(join(OUT, "result.json"), JSON.stringify({ ok: true, txid, steps }, null, 2));
  console.log(`settings: PASS (${steps.length} steps); screenshots in ${OUT}`);
}

main()
  .catch((e) => {
    console.error(`settings: FAIL: ${e instanceof Error ? e.message : String(e)}`);
    process.exitCode = 1;
  })
  .finally(() => {
    stopDriver(driver);
    try {
      if (work) rmSync(work, { recursive: true, force: true, maxRetries: 3 });
    } catch (e) {
      console.warn(`settings: could not remove ${work}: ${e instanceof Error ? e.message : String(e)}`);
    }
  });
