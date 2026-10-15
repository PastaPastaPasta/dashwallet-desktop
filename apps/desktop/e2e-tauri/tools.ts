// U-05 acceptance (T1): the Tools tabs on the real engine against a dashd regtest chain, with
// dashd as the judge. tauri-driver drives the production renderer; no fixture is involved.
//
// Flow: dashd mines 101 blocks → dwcli makes an encrypted vault and a wallet → dashd pays it
// 10 DASH and mines it → the app starts SPV against dashd alone and is unlocked →
//   Information shows dashd's height; Console answers `getblockcount` with it and refuses
//   `getblock`; Peers lists dashd, its one peer;
//   Sign/Verify signs with the wallet's address and dashd's `verifymessage` accepts it;
//   Address Book adds a dashd address;
//   Coin Selection lists the funding coin, locks and unlocks it, and pays with it: Pay picks the
//   address from the book, Review, Create Unsigned → Tools ▸ PSBT → Sign Tx → Broadcast Tx →
//   dashd's mempool holds the transaction paying that address → one block;
//   Repair rescans from the first block to the end.
// Tools is opened with its menu shortcut (Ctrl+Shift+I), pressed through xdotool as U-04 does.
// The OS file dialogs are not driven (WebDriver cannot reach a GTK dialog): Playwright covers
// them on the fixture (D2). Screenshots, the app's log, dashd's log and result.json go to
// $DW_E2E_OUT.
//
//   DW_APP=…/dash-wallet DWCLI=…/dwcli TAURI_DRIVER=…/tauri-driver DASHD=…/dashd node e2e-tauri/tools.ts
// `scripts/e2e-tauri-tools.sh` builds the binaries and runs it under a private session bus.

import { spawnSync, type ChildProcess } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { Dashd, freePort, paysTo, type RawTx } from "./dashd.ts";
import { APP, CHAIN, dwcli, env, poll, startDriver, stopDriver, tid, TIMEOUT_MS, WebDriver } from "./driver.ts";

const OUT = resolve(env("DW_E2E_OUT", "test-results/tauri-tools"));
const DASHD = resolve(env("DASHD"));
const PORT = Number(env("DW_E2E_PORT", "4774"));
const PASSPHRASE = "e2e tools passphrase";
const FUNDING = "10";
const PAYMENT = "1.25";
const PAYMENT_DUFFS = 125_000_000;
const MESSAGE = "U-05 says hello";
/** SPV sync, a scan or a sighting on a loaded host (as regtest-send.ts allows). */
const LOAD_WAIT_MS = Number(env("DW_E2E_LOAD_WAIT_MS", "600000"));

let driver: ChildProcess | undefined;
let node: Dashd | undefined;
/** Holds the vault, its passphrase file and dashd's chain: out of OUT; removed at the end. */
let work: string | undefined;

async function main() {
  mkdirSync(OUT, { recursive: true });
  work = mkdtempSync(join(tmpdir(), "dw-tools-")); // mode 0700
  const dataRoot = join(work, "wallet");
  const passFile = join(work, "passphrase");
  writeFileSync(passFile, `${PASSPHRASE}\n`, { mode: 0o600 });
  const steps: string[] = [];
  const evidence: Record<string, unknown> = {};
  const step = (s: string) => {
    steps.push(s);
    console.log(`tools: ${s}`);
  };

  const nodeDir = join(work, "dashd");
  mkdirSync(nodeDir);
  node = new Dashd(DASHD, nodeDir, await freePort(), await freePort(), join(OUT, "dashd.log"));
  const chain = node;
  await poll(300_000, "dashd's RPC", async () => (chain.tryCli("getblockchaininfo").ok ? true : undefined));
  evidence.dashd = chain.json<{ subversion: string }>("getnetworkinfo").subversion;
  chain.cli("createwallet", "miner");
  chain.mine(101);

  dwcli(dataRoot, passFile, "init-vault");
  const walletId = /wallet_id (\S+)/.exec(dwcli(dataRoot, passFile, "create"))?.[1];
  if (!walletId) throw new Error("dwcli create printed no wallet id");
  const address = /address (\S+)/.exec(dwcli(dataRoot, passFile, "receive"))?.[1];
  if (!address) throw new Error("dwcli receive printed no address");
  evidence.fundingTxid = chain.cli("-rpcwallet=miner", "sendtoaddress", address, FUNDING);
  chain.mine(1);
  step(`dashd ${evidence.dashd} regtest at ${chain.height()}; wallet ${walletId.slice(0, 16)}… funded with ${FUNDING} DASH at ${address}`);

  const started = await startDriver(PORT, OUT);
  driver = started.driver;
  const wd = new WebDriver(started.base, OUT);
  const args = ["--datadir", dataRoot, ...CHAIN, "--sync", "--spv-peer", `127.0.0.1:${chain.p2pPort}`, "--theme", "light"];
  await wd.start({ alwaysMatch: { "tauri:options": { application: APP, args } } });
  const textOf = (css: string) => wd.script<string>(`return document.querySelector('${css}')?.textContent ?? ""`);
  const valueOf = (css: string) => wd.script<string>(`return document.querySelector('${css}')?.value ?? ""`);
  const disabled = (css: string) => wd.script<boolean>(`return !!document.querySelector('${css}')?.disabled`);
  const tools = async (tab: string) => {
    await wd.press(tid(`tools-tabs-${tab}`));
    await wd.waitFor(tid(`tools-${tab}`), `Tools ▸ ${tab}`);
  };
  /** The grant sheet, if the payment setting asks for the passphrase. */
  const authorizeIfAsked = async () => {
    const asked = await poll(TIMEOUT_MS, "the grant sheet or its answer", async () => {
      if (await wd.find(tid("grant-sheet-field"))) return "asked";
      if ((await textOf(tid("psbt-message"))).includes("Signed")) return "not asked";
      return undefined;
    });
    if (asked === "asked") {
      await wd.fill(tid("grant-sheet-field"), PASSPHRASE);
      await wd.press(tid("grant-sheet-submit"));
      await wd.waitGone(tid("grant-sheet"), "the grant sheet");
    }
    return asked;
  };

  try {
    await wd.fill(tid("lock-field"), PASSPHRASE);
    await wd.press(tid("lock-submit"));
    await wd.waitGone(tid("lock-screen"), "the lock screen");
    if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");

    // Pay shows the funding once SPV saw its block: the wallet is caught up.
    await wd.press(tid("sidebar-pay"));
    await wd.press(tid("pay-tabs-address"));
    await poll(LOAD_WAIT_MS, "the funding in the balance", async () =>
      (await textOf(tid("pay-address-form"))).includes(`Available: ${FUNDING}.00000000`) ? true : undefined,
    );
    step("unlocked the real vault; SPV synced from dashd and counts the funding");

    // Information, Console and Peers: dashd's chain and dashd itself.
    await openTools(wd);
    await tools("information");
    const height = String(chain.height());
    await poll(LOAD_WAIT_MS, "the tip height in Information", async () => ((await textOf(tid("information-block-chain"))).includes(height) ? true : undefined));
    evidence.information = await textOf(tid("information-block-chain"));
    await wd.screenshot("1-information");
    step(`Information: current block height ${height}, as dashd`);

    await tools("console");
    await wd.fill(tid("console-line"), `getblockcount${ENTER}`);
    await poll(TIMEOUT_MS, "getblockcount's reply", async () => ((await textOf(tid("console-log"))).includes(height) ? true : undefined));
    await wd.fill(tid("console-line"), `getblock 1${ENTER}`);
    await poll(TIMEOUT_MS, "getblock's refusal", async () => ((await textOf(tid("console-log"))).includes("Not available in SPV mode") ? true : undefined));
    evidence.console = await textOf(tid("console-log"));
    await wd.screenshot("2-console");
    step(`Console: getblockcount answers ${height}; getblock is not available in SPV mode`);

    await tools("peers");
    const peer = `127.0.0.1:${chain.p2pPort}`;
    await poll(TIMEOUT_MS, "dashd in Peers", async () => ((await textOf(tid("peers-table"))).includes(peer) ? true : undefined));
    // dash-spv reports a peer's address and connection time only: the other columns show "—".
    evidence.peers = await textOf(tid(`peer-${peer}`));
    if ((await wd.findAll("[data-testid^='peer-127.']")).length !== 1) throw new Error("Peers lists more than dashd");
    await wd.screenshot("3-peers");
    step(`Peers: dashd at ${peer}, outbound (${evidence.peers})`);

    // Sign/Verify: a signature dashd accepts.
    await tools("signVerify");
    await wd.fill(tid("sign-address"), address);
    await wd.fill(tid("sign-message"), MESSAGE);
    await wd.press(tid("sign-sign"));
    await wd.waitFor(tid("sign-signature"), "the signature");
    const signature = await valueOf(tid("sign-signature"));
    evidence.signature = signature;
    evidence.dashdVerifies = chain.cli("verifymessage", address, signature, MESSAGE);
    if (evidence.dashdVerifies !== "true") throw new Error(`dashd's verifymessage says ${evidence.dashdVerifies}`);
    await wd.fill(tid("verify-address"), address);
    await wd.fill(tid("verify-message"), MESSAGE);
    await wd.fill(tid("verify-signature"), signature);
    await wd.press(tid("verify-verify"));
    await poll(TIMEOUT_MS, "Verify's outcome", async () => ((await textOf(tid("verify-result"))).includes("Message verified.") ? true : undefined));
    await wd.screenshot("4-sign-verify");
    step("Sign/Verify: the wallet signed; dashd's verifymessage and Verify accept it");

    // Address Book: the payee.
    const payee = chain.cli("-rpcwallet=miner", "getnewaddress");
    evidence.payee = payee;
    await tools("addressBook");
    await wd.press(tid("address-book-new"));
    await wd.fill(tid("address-book-label"), "Miner");
    await wd.fill(tid("address-book-address"), payee);
    await wd.press(tid("address-book-save"));
    await wd.waitFor(tid(`address-book-entry-${payee}`), "the new entry");
    await wd.screenshot("5-address-book");
    step(`Address Book: ${payee} saved as Miner`);

    // Coin Selection: the funding coin, a lock and an unlock, then Pay with it.
    await tools("coinSelection");
    const menu = await wd.waitFor("summary[data-testid^='coin-menu-']", "the funding coin");
    const key = (await wd.script<string>(`return document.querySelector("summary[data-testid^='coin-menu-']").dataset.testid`)).replace("coin-menu-", "");
    if (!key.startsWith(String(evidence.fundingTxid))) throw new Error(`Coin Selection lists ${key}, not the funding ${evidence.fundingTxid}`);
    await wd.click(menu);
    await wd.press(tid(`coin-menu-${key}-lock-unspent`));
    await wd.waitFor(tid("coins-locked"), "the lock");
    await wd.screenshot("6-coin-locked");
    await wd.press(tid(`coin-menu-${key}`));
    await wd.press(tid(`coin-menu-${key}-unlock-unspent`));
    await wd.waitGone(tid("coins-locked"), "the lock");
    await wd.press(`${tid("coin-table")} tbody input[type=checkbox]`);
    await wd.waitFor(tid("coin-summary"), "the selection's summary");
    evidence.coinSummary = await textOf(tid("coin-summary"));
    step(`Coin Selection: ${key} locked, unlocked and selected (${evidence.coinSummary})`);

    await wd.press(tid("coins-pay"));
    await wd.waitFor(tid("pay-address-form"), "Pay");
    if (!(await wd.script<boolean>(`return document.querySelector('${tid("coin-choice-selected")}').checked`))) throw new Error("Pay did not open on the selected coins");
    await wd.press(tid("pay-address-book"));
    await wd.press(tid(`address-book-choose-${payee}`));
    if ((await valueOf(tid("pay-address"))) !== payee) throw new Error("the picker did not fill the address");
    await wd.fill(tid("pay-amount"), PAYMENT);
    await poll(TIMEOUT_MS, "the engine's estimate", async () => ((await disabled(tid("pay-continue"))) ? undefined : true));
    await wd.press(tid("pay-continue"));
    await wd.press(tid("pay-confirm-unsigned"));
    await wd.waitFor(tid("psbt-lines"), "the unsigned PSBT in Tools ▸ PSBT");
    evidence.unsigned = await textOf(tid("psbt-lines"));
    if (!new RegExp(`Sends ${PAYMENT.replace(".", "\\.")}0+ \\S+ to ${payee}`).test(String(evidence.unsigned))) throw new Error(`the PSBT reads ${evidence.unsigned}`);
    await wd.screenshot("7-psbt-unsigned");
    step(`Pay with these coins → the book's address, ${PAYMENT} DASH → Create Unsigned → PSBT: ${evidence.unsigned}`);

    await wd.press(tid("psbt-sign"));
    evidence.signPassphrase = await authorizeIfAsked();
    await poll(TIMEOUT_MS, "the signature", async () => ((await textOf(tid("psbt-message"))).includes("Signed transaction successfully") ? true : undefined));
    evidence.signed = await textOf(tid("psbt-lines"));
    if (String(evidence.signed).includes("unsigned inputs")) throw new Error(`the signed PSBT still reads ${evidence.signed}`);
    const before = new Set(chain.json<string[]>("getrawmempool"));
    await wd.press(tid("psbt-broadcast"));
    const txid = await poll(LOAD_WAIT_MS, "the transaction in dashd's mempool", async () => chain.json<string[]>("getrawmempool").find((t) => !before.has(t)));
    const tx = chain.json<RawTx>("getrawtransaction", txid, "true");
    evidence.txid = txid;
    evidence.paid = paysTo(tx, payee);
    if (evidence.paid !== PAYMENT_DUFFS) throw new Error(`the transaction pays ${payee} ${evidence.paid} duffs, not ${PAYMENT_DUFFS}`);
    // dashd is the judge; the app says sent, or (SPV hears no answer to a broadcast) that it was
    // handed to the network and its outcome is unknown, as Pay's result does (U-03).
    evidence.broadcast = await poll(TIMEOUT_MS, "Broadcast's outcome", async () => {
      if ((await textOf(tid("psbt-message"))).includes(txid)) return "sent";
      if ((await textOf(tid("psbt-error"))).includes("whether it was accepted is unknown")) return "unknown";
      return undefined;
    });
    if (!(await disabled(tid("psbt-broadcast")))) throw new Error("Broadcast Tx is offered again");
    evidence.block = chain.mine(1)[0];
    await wd.screenshot("8-psbt-broadcast");
    step(`PSBT: signed (passphrase ${evidence.signPassphrase}), broadcast (${evidence.broadcast}); dashd's mempool holds ${txid} paying ${PAYMENT} DASH; mined in ${evidence.block}`);

    // Repair: a rescan from the first block runs to its end.
    await openTools(wd);
    await tools("repair");
    await wd.press(tid("repair-rescan-genesis"));
    await poll(LOAD_WAIT_MS, "the rescan to finish", async () =>
      (await wd.find(tid("repair-progress"))) || (await disabled(tid("repair-rescan-genesis"))) ? undefined : true,
    );
    const problem = await wd.find(`${tid("repair-outcome")}.form-problem`);
    evidence.repair = await textOf(tid("tools-repair"));
    await wd.screenshot("9-repair");
    if (problem) throw new Error(`the rescan failed: ${await textOf(tid("repair-outcome"))}`);
    step("Repair: a rescan from the first block ran to its end");
  } catch (e) {
    await wd.screenshot("failure").catch(() => undefined);
    throw e;
  } finally {
    await wd.end().catch(() => undefined);
  }
  writeFileSync(join(OUT, "result.json"), JSON.stringify({ ok: true, steps, evidence }, null, 2));
  console.log(`tools: PASS (${steps.length} steps); evidence in ${OUT}`);
}

/** WebDriver's Enter key. */
const ENTER = "\uE007";

function run(cmd: string, args: string[]): string {
  const r = spawnSync(cmd, args, { encoding: "utf8" });
  if (r.status !== 0) throw new Error(`${cmd} ${args[0]} failed (${r.status}): ${r.stderr.slice(-600)}`);
  return r.stdout;
}

/** Tools has no sidebar row: its menu shortcut opens it, pressed through the X server (the menu
 * bar owns it; WebDriver's keys reach only the page). The keys are always released again. */
async function openTools(wd: WebDriver) {
  try {
    run("xdotool", ["search", "--sync", "--onlyvisible", "--name", "^Dash Wallet", "windowfocus", "key", "--clearmodifiers", "ctrl+shift+i"]);
  } finally {
    run("xdotool", ["keyup", "ctrl+shift+i"]);
  }
  await wd.waitFor(tid("page-tools"), "the Tools page");
}

main()
  .catch((e) => {
    console.error(`tools: FAIL: ${e instanceof Error ? e.message : String(e)}`);
    process.exitCode = 1;
  })
  .finally(async () => {
    stopDriver(driver);
    await node?.stop();
    try {
      if (work) rmSync(work, { recursive: true, force: true, maxRetries: 3 });
    } catch (e) {
      console.warn(`tools: could not remove ${work}: ${e instanceof Error ? e.message : String(e)}`);
    }
  });
