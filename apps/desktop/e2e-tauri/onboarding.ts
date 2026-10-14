// Onboarding, lock and recovery on the real app, the real engine and a dashd regtest node (U-02
// T1 and GL tier, D20). tauri-driver drives the production renderer in WebKitGTK; SPV syncs from
// the node. No fixture is involved.
//
// Flow: a fresh data root opens on Welcome (no Cancel) ▸ Create: passphrase (a mismatch first),
// phrase, verify (a wrong chip first) ▸ Show Recovery Phrase equals the created phrase ▸ Add
// Wallet ▸ Restore a phrase dashd funded (an unknown word flagged, the last word completed
// from a suggestion), scan from genesis ▸ its balance arrives through SPV ▸ lock: a wrong
// passphrase (attempts remaining), unlock for mixing only, then fully from the status bar ▸
// lock ▸ Forgot passphrase with the restored phrase: the created wallet becomes watch-only ▸ the
// old passphrase fails, the new one unlocks ▸ three failures make the lock screen wait, and a
// relaunch keeps the wait ▸ after it, the new passphrase unlocks. At the end, pass or fail, no
// file of the data root or of OUT (logs included) holds the phrases or the passphrases in plain
// text; the screenshots are kept off phrase screens.
//
// Settings is opened with its menu shortcut (Ctrl+,), pressed through xdotool. Screenshots skip
// every screen that shows a phrase. `scripts/e2e-tauri-onboarding.sh` brings up the node, builds
// the binaries and runs this file under a private session bus:
//   DW_APP=… DWCLI=… TAURI_DRIVER=… DW_REGTEST_SH=…/regtest.sh DW_REGTEST_P2P=127.0.0.1:<port> \
//     node e2e-tauri/onboarding.ts

import { spawnSync, type ChildProcess } from "node:child_process";
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { env, poll, startDriver, stopDriver, tid, WebDriver } from "./webdriver.ts";

const APP = resolve(env("DW_APP"));
const DWCLI = resolve(env("DWCLI"));
const DRIVER = env("TAURI_DRIVER", "tauri-driver");
const REGTEST = resolve(env("DW_REGTEST_SH"));
const P2P = env("DW_REGTEST_P2P");
const OUT = resolve(env("DW_E2E_OUT", "test-results/tauri-onboarding"));
const PORT = Number(env("DW_E2E_PORT", "4754"));
const TIMEOUT_MS = Number(env("DW_E2E_TIMEOUT_MS", "120000"));
/** SPV from genesis on a fresh regtest chain: headers, filters and the wallet's blocks. */
const SYNC_TIMEOUT_MS = Number(env("DW_E2E_SYNC_TIMEOUT_MS", "300000"));
const CHAIN = ["--network", "regtest", "--no-platform", "--dapi", "https://127.0.0.1:1"];
const PASSPHRASE = "U-02 first passphrase 7!";
const NEW_PASSPHRASE = "U-02 reset passphrase 9?";
const FUNDER_PASSPHRASE = "U-02 funder passphrase";
const AMOUNT = "1.5";

// WebDriver key codes (W3C "keyboard actions").
const CONTROL = "";
const BACKSPACE = "";

let driver: ChildProcess | undefined;
/** The data roots and passphrase files: out of OUT, which CI uploads; removed at the end. */
let work: string | undefined;

function run(cmd: string, args: string[], input?: string): string {
  const r = spawnSync(cmd, args, { encoding: "utf8", input });
  if (r.status !== 0) throw new Error(`${cmd} ${args[0]} failed (${r.status}): ${r.stderr.slice(-600)}`);
  return r.stdout;
}

/** Presses a menu shortcut as a user would, through the X server (xdotool, XTEST): in the app the
 * menu bar owns the shortcuts, and WebDriver's keys reach only the page. The keys are released
 * again whatever happens: a release lost under load leaves the key down on the display,
 * repeating into every later field (and every later run on that display). */
function shortcut(keys: string) {
  try {
    run("xdotool", ["search", "--sync", "--onlyvisible", "--name", "^Dash Wallet", "windowfocus", "key", "--clearmodifiers", keys]);
  } finally {
    run("xdotool", ["keyup", keys]);
  }
}

const dwcli = (root: string, passFile: string, ...args: string[]) =>
  run(DWCLI, ["--datadir", root, ...CHAIN, "--passphrase-file", passFile, ...args]);

/** A wallet made by dwcli in its own data root: the phrase the app restores, and an address of it
 * that dashd funds. */
function fundedPhrase(root: string, passFile: string): { phrase: string; address: string; txid: string } {
  dwcli(root, passFile, "init-vault");
  const created = dwcli(root, passFile, "create");
  const phrase = /^mnemonic (.+)$/m.exec(created)?.[1]?.trim();
  if (!phrase || phrase.split(" ").length !== 12) throw new Error("dwcli create printed no 12-word phrase");
  const address = /address (\S+)/.exec(dwcli(root, passFile, "receive"))?.[1];
  if (!address) throw new Error("dwcli receive printed no address");
  const txid = run("sh", [REGTEST, "fund", address, AMOUNT]).trim().split("\n").pop()!;
  run("sh", [REGTEST, "mine", "2"]);
  return { phrase, address, txid };
}

/** Every file under `dir` (the data root, OUT) that holds one of `secrets` as plain text. */
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
  work = mkdtempSync(join(tmpdir(), "dw-onboarding-")); // mode 0700
  const appRoot = join(work, "app");
  const funderRoot = join(work, "funder");
  const funderPass = join(work, "funder-passphrase");
  writeFileSync(funderPass, `${FUNDER_PASSPHRASE}\n`, { mode: 0o600 });

  const funded = fundedPhrase(funderRoot, funderPass);
  console.log(`onboarding: dashd funded ${funded.address} with ${AMOUNT} DASH (${funded.txid.slice(0, 16)}…)`);

  const started = await startDriver(DRIVER, PORT, join(OUT, "tauri-driver.log"));
  driver = started.driver;
  const wd = new WebDriver(started.base, OUT, TIMEOUT_MS);
  const launch = () =>
    wd.start({
      alwaysMatch: {
        "tauri:options": { application: APP, args: ["--datadir", appRoot, ...CHAIN, "--sync", "--spv-peer", P2P, "--theme", "light"] },
      },
    });

  const steps: string[] = [];
  const step = (s: string) => {
    steps.push(s);
    console.log(`onboarding: ${s}`);
  };
  const texts = (css: string) => wd.script<string[]>(`return [...document.querySelectorAll(arguments[0])].map((e) => e.textContent.trim())`, [css]);
  const textOf = async (css: string) => wd.text(await wd.waitFor(css));
  const waitText = (css: string, want: RegExp, what: string) =>
    poll(TIMEOUT_MS, what, async () => {
      const el = await wd.find(css);
      const t = el ? await wd.text(el) : "";
      return want.test(t) ? t : undefined;
    });
  const isDisabled = (id: string) => wd.script<boolean>(`return !!document.querySelector(arguments[0])?.disabled`, [tid(id)]);
  /** Clicks the first element matching `css` whose text is `text`. */
  const clickText = async (css: string, text: string) => {
    for (const el of await wd.findAll(css)) {
      if ((await wd.text(el)).trim() === text) return void (await wd.click(el));
    }
    throw new Error(`no ${css} reads the expected text`);
  };
  /** Replaces a field's text by key presses (select all, delete, type), as a user would. */
  const retype = async (id: string, text: string) => {
    const el = await wd.waitFor(tid(id));
    await wd.type(el, `${CONTROL}a${CONTROL}${BACKSPACE}${text}`);
  };
  /** Types into a field and checks that it took every key, by length only (the text is a
   * secret). Under load a key can go missing; then it types the text again, up to three times. */
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
  const unlock = async (passphrase: string) => {
    await enter("lock-field", passphrase);
    await wd.press(tid("lock-submit"));
  };
  const lock = async () => {
    await wd.press(tid("toolbar-lock"));
    await wd.waitFor(tid("lock-field"), "the lock screen");
  };

  await launch();
  let created: string[] = [];
  let failure: unknown = null;
  try {
    // ---- first run: Welcome, nothing to cancel (IOS-002)
    await wd.waitFor(tid("onboarding-welcome"), "Welcome on a fresh data root");
    if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");
    if (await wd.find(tid("onboarding-cancel"))) throw new Error("first run offers Cancel");
    await wd.screenshot("01-welcome");
    step("fresh data root: Welcome, no Cancel");

    // ---- Create ▸ passphrase: a mismatch is refused, then set (IOS-010)
    await wd.press(tid("onboarding-create"));
    await enter("onboarding-passphrase-new", PASSPHRASE);
    await enter("onboarding-passphrase-repeat", `${PASSPHRASE}x`);
    await wd.press(tid("onboarding-passphrase-submit"));
    await waitText(tid("onboarding-passphrase-error"), /./, "the mismatch message");
    await wd.screenshot("02-passphrase-mismatch");
    await enter("onboarding-passphrase-new", PASSPHRASE);
    await enter("onboarding-passphrase-repeat", PASSPHRASE);
    await wd.press(tid("onboarding-passphrase-submit"));
    step("Choose passphrase: mismatch refused, passphrase set");

    // ---- the phrase, then verify with a wrong chip first (IOS-002, IOS-004); no screenshots
    await wd.waitFor(tid("onboarding-phrase-words"), "the recovery phrase");
    created = await texts(`${tid("onboarding-phrase-words")} .phrase-word`);
    if (created.length !== 12) throw new Error(`the phrase step shows ${created.length} words`);
    if (!(await isDisabled("onboarding-phrase-continue"))) throw new Error("Continue is enabled before the phrase is written down");
    await wd.press(tid("onboarding-written-down"));
    await wd.press(tid("onboarding-phrase-continue"));
    await wd.waitFor(tid("onboarding-verifyPhrase"), "the verify step");
    const positions = (await texts(`${tid("verify-slot")} .verify-slot-label`)).map((l) => Number(l.replace("Word ", "")) - 1);
    if (positions.length !== 4) throw new Error(`verify asks for ${positions.length} words`);
    const chips = await texts(tid("verify-chip"));
    const wrong = chips.find((c) => c !== created[positions[0]!]);
    if (!wrong) throw new Error("no wrong chip to tap");
    await clickText(tid("verify-chip"), wrong);
    await wd.waitFor(tid("verify-mistake"), "the wrong-word message");
    for (const p of positions) await clickText(tid("verify-chip"), created[p]!);
    await wd.waitFor(tid("onboarding-done"), "Done after the last chip");
    await wd.screenshot("03-created");
    step("phrase shown, verify: wrong chip refused, four words in order, wallet created");
    const title = await wd.call<string>("GET", wd.s("/title"));
    if (created.some((w) => title.split(/\W+/).includes(w))) throw new Error("a phrase word is in the window title");

    await wd.press(tid("onboarding-open"));
    await wd.waitFor(tid("hero"), "the Overview of the created wallet");
    await wd.screenshot("04-overview-created");
    step("Open wallet: Overview");

    // ---- Show Recovery Phrase (IOS-003): the passphrase, then the same words
    shortcut("ctrl+comma");
    await wd.press(tid("settings-recovery-phrase"));
    await enter("recovery-passphrase-field", PASSPHRASE);
    await wd.press(tid("recovery-passphrase-submit"));
    await wd.waitFor(tid("recovery-words"), "the revealed phrase");
    const revealed = await texts(`${tid("recovery-words")} .phrase-word`);
    if (revealed.join(" ") !== created.join(" ")) throw new Error("Show Recovery Phrase shows another phrase");
    await wd.press(tid("recovery-done"));
    await wd.waitGone(tid("recovery-phrase"), "the recovery sheet");
    step("Show Recovery Phrase: passphrase, the created phrase");

    // ---- Add Wallet ▸ Restore with live checks (IOS-007)
    await wd.press(tid("settings-add-wallet"));
    await wd.waitFor(tid("onboarding-cancel"), "Add Wallet with Cancel");
    await wd.press(tid("onboarding-restore"));
    const words = funded.phrase.split(" ");
    await retype("restore-input", `${words[0]} zzzz `);
    await waitText(tid("restore-problem"), /Word 2/, "the unknown-word message");
    const last = words[11]!;
    await retype("restore-input", `${words.slice(0, 11).join(" ")} ${last.slice(0, -1)}`);
    await wd.waitFor(tid("restore-suggestion"), "suggestions for the last word");
    await clickText(tid("restore-suggestion"), last);
    await wd.waitFor(tid("restore-valid"), "the valid-phrase note");
    await wd.press(tid("restore-continue"));
    await wd.waitFor(tid("onboarding-restoreOptions"), "the restore options");
    if (!(await wd.script<boolean>(`return document.querySelector(arguments[0]).checked`, [tid("restore-scan-genesis")])))
      throw new Error("the scan does not start at genesis by default");
    await wd.screenshot("05-restore-options");
    await wd.press(tid("restore-finish"));
    await wd.waitFor(tid("onboarding-done"), "Done after the restore");
    await wd.press(tid("onboarding-open"));
    step("Add Wallet ▸ Restore: unknown word flagged, suggestion completes, scan from genesis");

    // ---- its balance arrives through SPV from dashd (GL)
    await wd.waitFor(tid("hero"), "the Overview of the restored wallet");
    const restoredName = await wd.script<string>(
      `const s = document.querySelector('[data-testid="toolbar-wallet-picker"]'); return s ? s.selectedOptions[0].textContent : ""`,
    );
    const createdName = (await texts(`${tid("toolbar-wallet-picker")} option`)).find((n) => n !== restoredName);
    if (!restoredName || !createdName) throw new Error("the toolbar does not offer both wallets");
    const balance = await poll(SYNC_TIMEOUT_MS, `the restored balance (${AMOUNT} DASH) through SPV`, async () => {
      const shown = await wd.script<string>(`return document.querySelector('[data-testid="hero-amount"] .amount')?.title || ""`);
      return shown.includes(AMOUNT) ? shown : undefined;
    });
    await wd.screenshot("06-restored-balance");
    step(`restored wallet: ${balance} through SPV from dashd`);

    // ---- lock screen: wrong passphrase, mixing only, the unlock sheet (IOS-012, IOS-013)
    await lock();
    await unlock("not the passphrase");
    await waitText(tid("lock-error"), /incorrect.*\d+ attempts? remaining/, "attempts remaining");
    await wd.screenshot("07-wrong-passphrase");
    await wd.press(tid("lock-mixing-only"));
    await unlock(PASSPHRASE);
    await wd.waitGone(tid("lock-screen"), "the lock screen");
    await wd.press(tid("status-unlock"));
    await wd.screenshot("08-unlock-sheet");
    await enter("unlock-sheet-field", PASSPHRASE);
    await wd.press(tid("unlock-sheet-submit"));
    await wd.waitGone(tid("unlock-sheet"), "the unlock sheet");
    await wd.waitGone(tid("status-unlock"), "the mixing-only status");
    step("lock: wrong passphrase (attempts remaining), mixing only, unlock sheet to full");

    // ---- Forgot passphrase with the restored phrase (IOS-014)
    await lock();
    await wd.press(tid("lock-forgot"));
    await wd.waitFor(tid("forgot-wallet"), "the wallet choice");
    await clickText(tid("forgot-wallet"), restoredName);
    const consequences = await textOf(tid("forgot-consequences"));
    if (!consequences.includes(createdName)) throw new Error("What happens does not name the other wallet");
    await wd.screenshot("09-forgot-consequences");
    await wd.press(tid("forgot-accept"));
    await retype("forgot-phrase-input", funded.phrase);
    await wd.waitFor(tid("forgot-phrase-valid"), "the valid-phrase note");
    await wd.press(tid("forgot-continue"));
    await enter("forgot-new-new", NEW_PASSPHRASE);
    await enter("forgot-new-repeat", NEW_PASSPHRASE);
    await wd.press(tid("forgot-new-submit"));
    await wd.waitFor(tid("forgot-done"), "the reset");
    if (!(await textOf(tid("forgot-watch-only"))).includes(createdName)) throw new Error("Done does not list the watch-only wallet");
    await wd.screenshot("10-forgot-done");
    await wd.press(tid("forgot-close"));
    await wd.waitFor(tid("hero"), "the shell after the reset");
    step(`Forgot passphrase: ${restoredName} keeps its keys, ${createdName} watch-only`);

    await lock();
    await unlock(PASSPHRASE);
    await waitText(tid("lock-error"), /incorrect.*7 attempts remaining/, "the old passphrase refused");
    await unlock(NEW_PASSPHRASE);
    await wd.waitGone(tid("lock-screen"), "the lock screen");
    step("the old passphrase fails, the new one unlocks");

    // ---- three failures: the wait, kept across a relaunch (IOS-012). Each waits for its own
    // message, so the next is not typed while the last is still being checked.
    await lock();
    for (const [i, want] of [/7 attempts remaining/, /6 attempts remaining/, /try again in/].entries()) {
      await unlock(`wrong ${i + 1}`);
      await waitText(tid("lock-error"), want, `failure ${i + 1}`);
    }
    const waitStarted = Date.now();
    if (!(await isDisabled("lock-field"))) throw new Error("the field takes a passphrase during the wait");
    await wd.screenshot("11-lock-wait");
    await wd.end();
    await launch();
    await waitText(tid("lock-error"), /try again in/, "the wait after the relaunch");
    if (!(await isDisabled("lock-field"))) throw new Error("the relaunched lock screen takes a passphrase during the wait");
    await wd.screenshot("12-lock-wait-relaunched");
    step("three failures: the wait, kept across a relaunch");

    await poll(Math.max(0, 75_000 - (Date.now() - waitStarted)) + TIMEOUT_MS, "the end of the wait", async () =>
      (await isDisabled("lock-field")) ? undefined : true,
    );
    await unlock(NEW_PASSPHRASE);
    await wd.waitGone(tid("lock-screen"), "the lock screen");
    await wd.waitFor(tid("hero"), "the Overview after the wait");
    await wd.screenshot("13-unlocked-after-wait");
    step("after the wait: the new passphrase unlocks");
  } catch (e) {
    failure = e;
    // Never of a phrase screen, which would put the phrase in OUT: checked before and after,
    // and dropped if one came up meanwhile.
    const onPhrase = () =>
      wd.script<boolean>(`return !!document.querySelector('.phrase-grid, .verify-chips, .phrase-input')`).catch(() => true);
    if (!(await onPhrase())) {
      await wd.screenshot("failure").catch(() => undefined);
      if (await onPhrase()) rmSync(join(OUT, "failure.png"), { force: true });
    }
  } finally {
    await wd.end().catch(() => undefined);
  }

  // Whatever the outcome (CI uploads OUT either way), once the app and the driver have exited
  // and their output is on disk. The screenshots are kept off phrase screens above: text drawn
  // into a PNG is not found here.
  stopDriver(driver);
  driver = undefined;
  await started.closed;
  const secrets = [funded.phrase, created.join(" "), PASSPHRASE, NEW_PASSPHRASE].filter((s) => s !== "");
  const found = [...leaks(appRoot, secrets), ...leaks(OUT, secrets)];
  if (found.length > 0) {
    // Not uploaded.
    for (const f of found) if (f.startsWith(OUT)) rmSync(f.replace(/ \(\d+\)$/, ""), { force: true });
    const leak = `secrets in plain text: ${found.join(", ")}`;
    throw new Error(failure ? `${failure instanceof Error ? failure.message : String(failure)}; ${leak}` : leak);
  }
  if (failure) throw failure;
  step("no phrase or passphrase in the data root, the logs or the other files of OUT");

  writeFileSync(join(OUT, "result.json"), JSON.stringify({ ok: true, txid: funded.txid, steps }, null, 2));
  console.log(`onboarding: PASS (${steps.length} steps); screenshots in ${OUT}`);
}

main()
  .catch((e) => {
    console.error(`onboarding: FAIL: ${e instanceof Error ? e.message : String(e)}`);
    process.exitCode = 1;
  })
  .finally(() => {
    stopDriver(driver);
    try {
      if (work) rmSync(work, { recursive: true, force: true, maxRetries: 3 });
    } catch (e) {
      console.warn(`onboarding: could not remove ${work}: ${e instanceof Error ? e.message : String(e)}`);
    }
  });
