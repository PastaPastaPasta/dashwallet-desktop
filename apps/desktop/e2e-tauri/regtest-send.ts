// U-03 acceptance (ROADMAP U-03, D19): a real send through the UI on a dashd regtest chain.
// tauri-driver drives the production renderer on the real engine; dashd is the chain and the
// judge. No fixture is involved.
//
// Flow: dashd regtest starts at genesis and mines 101 blocks to its `miner` wallet → dwcli makes
// an encrypted vault and a wallet → dashd pays the wallet's address 10 DASH and mines it → the
// app starts SPV against dashd alone, is unlocked and shows the balance → Pay ▸ To an address:
// a fresh dashd address, 1.25 DASH, Review, the passphrase, Send → dashd's mempool holds the
// transaction, paying that address 1.25 DASH → one block is mined (dash-spv accepts a broadcast
// from its one peer without an InstantSend quorum once it is mined) → the result sheet → Activity
// shows the transaction in a block → dashd sees it confirmed and the wallet's change.
// Screenshots, the app's log, dashd's log and result.json (the evidence) go to $DW_E2E_OUT.
//
//   DW_APP=…/dash-wallet DWCLI=…/dwcli TAURI_DRIVER=…/tauri-driver DASHD=…/dashd \
//     node e2e-tauri/regtest-send.ts
// `scripts/e2e-regtest-send.sh` builds the binaries, finds dashd and runs it under a private
// session bus. Like smoke.ts it needs a private X display. Not in CI: it needs dashd.

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { closeSync, mkdirSync, mkdtempSync, openSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { APP, CHAIN, dwcli, env, poll, startDriver, stopDriver, tid, TIMEOUT_MS, WebDriver } from "./driver.ts";

const OUT = resolve(env("DW_E2E_OUT", "test-results/tauri-regtest-send"));
const DASHD = resolve(env("DASHD"));
const DASH_CLI = join(dirname(DASHD), "dash-cli");
const PORT = Number(env("DW_E2E_PORT", "4754"));
const PASSPHRASE = "e2e regtest passphrase";
const FUNDING = "10";
const PAYMENT = "1.25";
const PAYMENT_DUFFS = 125_000_000;
/** SPV sync, a scan or a sighting on a loaded host (as dw-engine's regtest tests allow). */
const LOAD_WAIT_MS = Number(env("DW_E2E_LOAD_WAIT_MS", "600000"));

/** A free loopback port. */
function freePort(): Promise<number> {
  return new Promise((ok, fail) => {
    const s = createServer();
    s.once("error", fail);
    s.listen(0, "127.0.0.1", () => {
      const { port } = s.address() as { port: number };
      s.close(() => ok(port));
    });
  });
}

/** A regtest dashd at genesis with RPC through dash-cli; `stop` ends it. */
class Dashd {
  child: ChildProcess;
  datadir: string;
  rpcPort: number;
  p2pPort: number;

  constructor(datadir: string, p2pPort: number, rpcPort: number) {
    this.datadir = datadir;
    this.p2pPort = p2pPort;
    this.rpcPort = rpcPort;
    // Straight into the file: a pipe is read by this process's event loop, which `spawnSync`
    // (dash-cli, dwcli) blocks, and dashd stops at a full pipe (mining froze at block 25).
    const log = openSync(join(OUT, "dashd.log"), "w");
    // As regtest/README.md's node, on private ports.
    this.child = spawn(
      DASHD,
      [
        "-regtest",
        `-datadir=${datadir}`,
        "-server",
        "-printtoconsole",
        "-txindex=1",
        "-blockfilterindex=1",
        "-peerblockfilters=1",
        "-peerbloomfilters=1",
        "-listen=1",
        "-listenonion=0",
        `-port=${p2pPort}`,
        `-bind=127.0.0.1:${p2pPort}`,
        `-rpcport=${rpcPort}`,
        `-rpcbind=127.0.0.1:${rpcPort}`,
        "-rpcallowip=127.0.0.1",
        "-rpcuser=dw",
        "-rpcpassword=dw",
        "-fallbackfee=0.00001",
      ],
      { stdio: ["ignore", log, log] },
    );
    closeSync(log);
  }

  tryCli(...args: string[]): { ok: boolean; out: string } {
    const r = spawnSync(DASH_CLI, ["-regtest", `-datadir=${this.datadir}`, `-rpcport=${this.rpcPort}`, "-rpcuser=dw", "-rpcpassword=dw", ...args], {
      encoding: "utf8",
    });
    return { ok: r.status === 0, out: (r.status === 0 ? r.stdout : r.stderr).trim() };
  }

  cli(...args: string[]): string {
    const r = this.tryCli(...args);
    if (!r.ok) throw new Error(`dash-cli ${args.join(" ")}: ${r.out}`);
    return r.out;
  }

  json = <T>(...args: string[]) => JSON.parse(this.cli(...args)) as T;
  height = () => Number(this.cli("getblockcount"));

  mine(blocks: number): string[] {
    return this.json<string[]>("generatetoaddress", String(blocks), this.cli("-rpcwallet=miner", "getnewaddress"));
  }

  /** Stops dashd: `stop` over RPC, SIGKILL after 30 s. */
  stop(): Promise<void> {
    if (this.child.exitCode !== null) return Promise.resolve();
    const exited = new Promise<void>((ok) => this.child.once("exit", () => ok()));
    this.tryCli("stop");
    const kill = setTimeout(() => this.child.kill("SIGKILL"), 30_000);
    return exited.finally(() => clearTimeout(kill));
  }
}

interface RawTx {
  txid: string;
  confirmations?: number;
  blockhash?: string;
  vout: { value: number; scriptPubKey: { address?: string; addresses?: string[] } }[];
}

const paysTo = (tx: RawTx, address: string) =>
  tx.vout.filter((o) => o.scriptPubKey.address === address || o.scriptPubKey.addresses?.includes(address)).reduce((n, o) => n + Math.round(o.value * 1e8), 0);

let driver: ChildProcess | undefined;
let node: Dashd | undefined;
/** Holds the vault, its passphrase file and dashd's chain: out of OUT; removed at the end. */
let work: string | undefined;

async function main() {
  mkdirSync(OUT, { recursive: true });
  work = mkdtempSync(join(tmpdir(), "dw-regtest-send-")); // mode 0700
  const dataRoot = join(work, "wallet");
  const passFile = join(work, "passphrase");
  writeFileSync(passFile, `${PASSPHRASE}\n`, { mode: 0o600 });
  const steps: string[] = [];
  const evidence: Record<string, unknown> = {};
  const step = (s: string) => {
    steps.push(s);
    console.log(`regtest-send: ${s}`);
  };

  // The chain: dashd at genesis, its miner wallet funded.
  const nodeDir = join(work, "dashd");
  mkdirSync(nodeDir);
  node = new Dashd(nodeDir, await freePort(), await freePort());
  const chain = node;
  await poll(300_000, "dashd's RPC", async () => (chain.tryCli("getblockchaininfo").ok ? true : undefined));
  evidence.dashd = chain.json<{ subversion: string }>("getnetworkinfo").subversion;
  chain.cli("createwallet", "miner");
  chain.mine(101);
  step(`dashd ${evidence.dashd} regtest: 101 blocks mined to its miner wallet`);

  // The wallet: dwcli makes the vault and the wallet, dashd pays its address.
  dwcli(dataRoot, passFile, "init-vault");
  const walletId = /wallet_id (\S+)/.exec(dwcli(dataRoot, passFile, "create"))?.[1];
  if (!walletId) throw new Error("dwcli create printed no wallet id");
  const address = /address (\S+)/.exec(dwcli(dataRoot, passFile, "receive"))?.[1];
  if (!address) throw new Error("dwcli receive printed no address");
  evidence.fundingTxid = chain.cli("-rpcwallet=miner", "sendtoaddress", address, FUNDING);
  chain.mine(1);
  step(`wallet ${walletId.slice(0, 16)}… funded with ${FUNDING} DASH at ${address} (mined at ${chain.height()})`);

  const started = await startDriver(PORT, OUT);
  driver = started.driver;
  const wd = new WebDriver(started.base, OUT);
  const args = ["--datadir", dataRoot, ...CHAIN, "--sync", "--spv-peer", `127.0.0.1:${chain.p2pPort}`, "--theme", "light"];
  await wd.start({ alwaysMatch: { "tauri:options": { application: APP, args } } });
  const disabled = (id: string) => wd.script<boolean>(`return !!document.querySelector('${tid(id)}')?.disabled`);
  const textOf = (css: string) => wd.script<string>(`return document.querySelector('${css}')?.textContent ?? ""`);
  try {
    await wd.fill(tid("lock-field"), PASSPHRASE);
    await wd.press(tid("lock-submit"));
    await wd.waitGone(tid("lock-screen"), "the lock screen");
    if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");
    step("unlocked the real vault; SPV syncs from dashd alone");

    // Pay shows what the engine counts as spendable: the funding, once SPV saw its block.
    await wd.press(tid("sidebar-pay"));
    await wd.press(tid("pay-tabs-address"));
    await poll(LOAD_WAIT_MS, "the funding in the balance", async () =>
      (await textOf(tid("pay-address-form"))).includes(`Available: ${FUNDING}.00000000`) ? true : undefined,
    );
    await wd.screenshot("1-funded");
    step(`balance: ${FUNDING}.00000000 DASH available`);

    const payee = chain.cli("-rpcwallet=miner", "getnewaddress");
    evidence.payee = payee;
    await wd.fill(tid("pay-address"), payee);
    await wd.fill(tid("pay-amount"), PAYMENT);
    await wd.fill(tid("pay-label"), "Regtest payee");
    await poll(TIMEOUT_MS, "the engine's estimate", async () => ((await disabled("pay-continue")) ? undefined : true));
    evidence.estimatedFee = await textOf(tid("pay-fee-line"));
    await wd.screenshot("2-pay-form");
    step(`Pay ▸ To an address: ${PAYMENT} DASH to ${payee}, ${evidence.estimatedFee}`);

    await wd.press(tid("pay-continue"));
    await wd.fill(tid("pay-confirm-passphrase"), PASSPHRASE);
    evidence.confirmTotal = await textOf(tid("pay-confirm-total"));
    await wd.screenshot("3-confirm");
    step(`Review: confirm sheet, total ${evidence.confirmTotal}; passphrase entered`);

    const before = new Set(chain.json<string[]>("getrawmempool"));
    await wd.press(tid("pay-confirm-send"));
    // The payment reaches dashd's mempool; then a block, which dash-spv takes as acceptance.
    const txid = await poll(LOAD_WAIT_MS, "the payment in dashd's mempool", async () =>
      chain.json<string[]>("getrawmempool").find((t) => !before.has(t)),
    );
    const pending = chain.json<RawTx>("getrawtransaction", txid, "true");
    evidence.txid = txid;
    evidence.mempoolPaid = paysTo(pending, payee);
    if (evidence.mempoolPaid !== PAYMENT_DUFFS) throw new Error(`the transaction pays ${payee} ${evidence.mempoolPaid} duffs, not ${PAYMENT_DUFFS}`);
    step(`dashd's mempool holds ${txid}, paying ${payee} ${PAYMENT} DASH`);

    evidence.block = chain.mine(1)[0];
    await wd.waitFor(`${tid("pay-result-sent")}, ${tid("pay-result-unknown")}`, "the payment's result", LOAD_WAIT_MS);
    evidence.result = (await wd.find(tid("pay-result-sent"))) ? "pay-result-sent" : "pay-result-unknown";
    await wd.screenshot("4-result");
    // Under load the broadcast's own wait can end before SPV sees the block: "status unknown",
    // which the block settles (dw-engine's e0_04_bl76_regtest allows the same).
    if (evidence.result === "pay-result-sent" && !(await textOf(tid("pay-result-txid"))).includes(txid))
      throw new Error("the result sheet shows another transaction id");
    step(`result: ${evidence.result === "pay-result-sent" ? "Payment sent, with dashd's txid" : "status unknown (the block settles it)"}; block ${evidence.block}`);

    // Activity: the transaction, in a block. Show in Activity opens its detail sheet (U-03 D10).
    if (evidence.result === "pay-result-sent") await wd.press(tid("pay-result-show"));
    else {
      await wd.press(tid("pay-result-close"));
      await wd.press(tid("sidebar-activity"));
      await wd.press(`[data-txid="${txid}"]`);
    }
    await poll(LOAD_WAIT_MS, "the transaction in a block", async () => ((await textOf(tid("tx-detail-facts"))).includes("Block") ? true : undefined));
    evidence.activity = await textOf(tid("tx-detail-facts"));
    await wd.screenshot("5-activity-detail");
    step("Activity: the transaction's detail shows its block");

    const mined = chain.json<RawTx>("getrawtransaction", txid, "true");
    evidence.confirmations = mined.confirmations;
    if (mined.blockhash !== evidence.block) throw new Error(`${txid} is in ${mined.blockhash}, not the mined block ${evidence.block}`);
    evidence.minerReceived = chain.json<{ amount: number }>("-rpcwallet=miner", "gettransaction", txid).amount;
    step(`dashd: ${txid} confirmed in ${mined.blockhash}; the miner wallet received ${evidence.minerReceived} DASH`);
  } catch (e) {
    await wd.screenshot("failure").catch(() => undefined);
    throw e;
  } finally {
    await wd.end().catch(() => undefined);
  }
  writeFileSync(join(OUT, "result.json"), JSON.stringify({ ok: true, steps, evidence }, null, 2));
  console.log(`regtest-send: PASS (${steps.length} steps); evidence in ${OUT}`);
}

main()
  .catch((e) => {
    console.error(`regtest-send: FAIL: ${e instanceof Error ? e.message : String(e)}`);
    process.exitCode = 1;
  })
  .finally(async () => {
    stopDriver(driver);
    // dashd stops before its datadir goes.
    await node?.stop();
    try {
      if (work) rmSync(work, { recursive: true, force: true, maxRetries: 3 });
    } catch (e) {
      console.warn(`regtest-send: could not remove ${work}: ${e instanceof Error ? e.message : String(e)}`);
    }
  });
