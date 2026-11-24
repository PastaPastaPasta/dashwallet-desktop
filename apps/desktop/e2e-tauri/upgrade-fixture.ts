// Writes an upgrade-path fixture with a released app (rust/crates/dw-app/src/upgrade_path_tests.rs reads it).
// After this writer seeds settings.json (the pre-release notice dismissed), the release binary is the
// only thing that writes the data root: tauri-driver drives it and every change goes through its own
// IPC, as its renderer would send it. dashd regtest is the chain and the
// judge of what the app reports.
//
// Phase 1, syncing from dashd: an encrypted vault, two wallets from BIP39 test vectors, dashd pays
// three addresses, the app pays dashd 1.25 tDASH, a transaction label, an address-book entry, a
// receive request and the settings listed in the README are set to non-defaults; from 0.2.0 also a
// locked outpoint and a watch-only wallet that dashd pays. The app is closed.
// Phase 2, offline: the app opens the data root again, is unlocked, and its answers to the read
// commands are recorded in expected.json, after they are checked against dashd, with a message
// signed by every funded address of the wallets with keys (from 0.2.0), verified by dashd. It is closed
// and the data root is copied to $DW_UPGRADE_OUT/data, by way of a staging directory beside it; an
// existing $DW_UPGRADE_OUT is refused before anything starts.
//
// Each launch picks free ports for tauri-driver and WebKitWebDriver, refuses either that already
// answers, and checks that the driver it spawned owns both listeners before it asks for a session.
//
// Run it only through `scripts/upgrade-fixture.sh <tag> <sha256>`, which downloads and verifies the
// release and starts this under a fresh session bus and Xvfb display; run directly, it refuses.

import { spawnSync, type ChildProcess } from "node:child_process";
import { cpSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readdirSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { connect } from "node:net";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { accountTpub } from "./bip32.ts";
import { Dashd, freePort, paysTo, type RawTx } from "./dashd.ts";
import { ownedProbe, processGroup, ssListing } from "./listeners.ts";
import { type Coin, outpointProblem, type TxOut } from "./outpoint.ts";
import { APP, CHAIN, driverGone, driverReady, env, poll, spawnDriver, stopDriver, WebDriver } from "./driver.ts";

if (process.env.DW_UPGRADE_PRIVATE_SESSION !== "1" || process.env.GDK_BACKEND !== "x11" || process.env.WAYLAND_DISPLAY) {
  console.error("upgrade-fixture: run it through scripts/upgrade-fixture.sh, which gives it a private X display and session bus");
  process.exit(2);
}

const VERSION = env("DW_UPGRADE_VERSION");
const OUT = resolve(env("DW_UPGRADE_OUT"));
const LOGS = resolve(env("DW_E2E_OUT", "test-results/upgrade-fixture"));
const DASHD = resolve(env("DASHD"));
const LOAD_WAIT_MS = Number(env("DW_E2E_LOAD_WAIT_MS", "600000"));

const PASSPHRASE = "upgrade fixture passphrase";
const WALLETS = [
  { name: "Fixture main", mnemonic: "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about", bip39: null },
  { name: "Fixture second", mnemonic: "legal winner thank year wave sausage worth useful legal winner thank yellow", bip39: "TREZOR" },
];
const MESSAGE = "dashwallet-desktop upgrade fixture";
const DUFFS = 100_000_000;

type Call = { cmd: string; args: Record<string, unknown>; result: unknown };

let driver: ChildProcess | undefined;
let node: Dashd | undefined;
let work: string | undefined;
/** The staging directory this run created, removed unless it was renamed into place. */
let staging: string | undefined;

const present = (path: string) => {
  try {
    lstatSync(path);
    return true;
  } catch {
    return false;
  }
};

function step(s: string) {
  console.log(`upgrade-fixture: ${s}`);
}

const answers = (port: number) =>
  Promise.all(
    ["127.0.0.1", "::1"].map(
      (host) =>
        new Promise<boolean>((ok) => {
          const s = connect({ host, port });
          s.once("connect", () => (s.destroy(), ok(true)));
          s.once("error", () => ok(false));
        }),
    ),
  ).then((r) => r.some(Boolean));

/** tauri-driver and WebKitWebDriver on two free ports, refused if either answered before the
 * spawn, and accepted only when every listener on both ports is in the spawned driver's group. */
async function startOwnDriver() {
  const port = await freePort();
  let native = await freePort();
  while (native === port) native = await freePort();
  for (const p of [port, native]) if (await answers(p)) throw new Error(`port ${p} already answers: no driver is started beside it`);
  driver = spawnDriver(port, LOGS, native);
  const group = driver.pid;
  await poll(30_000, "both drivers to listen", ownedProbe(ssListing, [port, native], group, processGroup));
  const base = await driverReady(port);
  step(`tauri-driver on ${port}, WebKitWebDriver on ${native}, both owned by group ${group}`);
  return { driver, base };
}

async function launch(dataRoot: string, args: string[]) {
  const started = await startOwnDriver();
  const wd = new WebDriver(started.base, LOGS);
  await wd.start({ alwaysMatch: { "tauri:options": { application: APP, args: ["--datadir", dataRoot, ...CHAIN, ...args, "--theme", "light"] } } });
  await wd.call("POST", wd.s("/timeouts"), { script: LOAD_WAIT_MS });
  await poll(LOAD_WAIT_MS, "the app's IPC", async () => ((await wd.script<boolean>("return !!window.__TAURI_INTERNALS__")) ? true : undefined));
  if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture backend");
  const invoke = async <T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> => {
    const r = await wd.scriptAsync<{ ok?: T; err?: unknown }>(
      `const done = arguments[arguments.length - 1];
       window.__TAURI_INTERNALS__.invoke(arguments[0], arguments[1]).then((ok) => done({ ok: ok ?? null }), (err) => done({ err }));`,
      [cmd, args],
    );
    if ("err" in r) throw new Error(`${cmd}: ${JSON.stringify(r.err)}`);
    return r.ok as T;
  };
  await poll(LOAD_WAIT_MS, "the engine", async () => (await invoke<unknown>("active_network").catch(() => null)) ?? undefined);
  return { wd, invoke };
}

/** Quits the app as SIGTERM does (its engine shutdown), then
 * checks that the session ended cleanly: no `.session-open` marker is left. */
async function close(wd: WebDriver, dataRoot: string) {
  const group = driver?.pid;
  const pids = spawnSync("pgrep", ["-g", String(group), "-f", `^${APP}`], { encoding: "utf8" }).stdout.split("\n").filter(Boolean).map(Number);
  if (pids.length !== 1) throw new Error(`found ${pids.length} app processes`);
  const pid = pids[0]!;
  process.kill(pid, "SIGTERM");
  await poll(LOAD_WAIT_MS, "the app to quit", async () => {
    try {
      process.kill(pid, 0);
      return undefined;
    } catch {
      return true;
    }
  });
  await wd.end().catch(() => undefined);
  stopDriver(driver);
  await driverGone(driver);
  driver = undefined;
  const marker = join(dataRoot, "regtest", ".session-open");
  if (existsSync(marker)) throw new Error("the app left .session-open: it did not shut down cleanly");
}

async function main() {
  if (present(OUT)) throw new Error(`${OUT} exists: remove it or choose another destination`);
  const stage = join(dirname(OUT), `.${basename(OUT)}.staging-${process.pid}`);
  if (present(stage)) throw new Error(`${stage} exists`);
  mkdirSync(LOGS, { recursive: true });
  work = mkdtempSync(join(tmpdir(), "dw-upgrade-fixture-"));
  const dataRoot = join(work, "data");
  mkdirSync(dataRoot, { mode: 0o700 });
  writeFileSync(join(dataRoot, "settings.json"), '{"preReleaseNoticeDismissed":true}\n');

  const nodeDir = join(work, "dashd");
  mkdirSync(nodeDir);
  node = new Dashd(DASHD, nodeDir, await freePort(), await freePort(), join(LOGS, "dashd.log"));
  const chain = node;
  await poll(300_000, "dashd's RPC", async () => (chain.tryCli("getblockchaininfo").ok ? true : undefined));
  const dashdVersion = chain.json<{ subversion: string }>("getnetworkinfo").subversion;
  chain.cli("createwallet", "miner");
  chain.mine(101);
  step(`dashd ${dashdVersion}: 101 blocks`);

  // ---------------------------------------------------------------- phase 1: the release writes
  let { wd, invoke } = await launch(dataRoot, ["--sync", "--spv-peer", `127.0.0.1:${chain.p2pPort}`]);
  const launched = await invoke<{ appVersion: string; engineVersion: string }>("launch_info");
  if (`v${launched.appVersion}` !== VERSION) throw new Error(`the binary is ${launched.appVersion}, not ${VERSION}`);
  const v010 = launched.appVersion === "0.1.0";
  await invoke("create_vault", { passphrase: PASSPHRASE });
  const ids: string[] = [];
  for (const w of WALLETS)
    ids.push(await invoke<string>("import_wallet", { mnemonic: w.mnemonic, bip39Passphrase: w.bip39, options: { name: w.name, birthHeight: 0, coreCompat: false, lookahead: null } }));
  const [main, second] = ids as [string, string];
  step(`vault and wallets ${ids.map((i) => i.slice(0, 12)).join(", ")}`);

  const addr = async (id: string, label?: string) =>
    (await invoke<{ address: string }>(label ? "next_receive_address" : "current_receive_address", label ? { walletId: id, label } : { walletId: id })).address;
  const mainFirst = await addr(main);
  const mainSavings = await addr(main, "Savings");
  const secondFirst = await addr(second);
  const funding = {
    mainFirst: chain.cli("-rpcwallet=miner", "sendtoaddress", mainFirst, "10"),
    mainSavings: chain.cli("-rpcwallet=miner", "sendtoaddress", mainSavings, "2.5"),
    secondFirst: chain.cli("-rpcwallet=miner", "sendtoaddress", secondFirst, "3"),
  };
  chain.mine(1);
  const balanceOf = (id: string) => invoke<{ confirmed: number } | null>("balance", { walletId: id });
  await poll(LOAD_WAIT_MS, "the funding in both balances", async () =>
    (await balanceOf(main))?.confirmed === 12.5 * DUFFS && (await balanceOf(second))?.confirmed === 3 * DUFFS ? true : undefined,
  );
  step("funded: 12.5 tDASH to the main wallet, 3 tDASH to the second");

  const payee = chain.cli("-rpcwallet=miner", "getnewaddress");
  const draft = await invoke<number>("new_tx_draft", { walletId: main });
  await invoke("draft_set_recipients", { draft, recipients: [{ address: payee, amount: 1.25 * DUFFS, subtractFeeFromAmount: false, label: "Fixture payee", message: null }] });
  const spend = await invoke<{ id: string }>("authorize", { purpose: { kind: "spend", maxDuffs: 2 * DUFFS }, walletId: main, credential: { kind: "passphrase", passphrase: PASSPHRASE } });
  const prepared = await invoke<{ handle: number }>("draft_prepare", { draft, grantId: spend.id });
  // dash-spv takes a broadcast as accepted once its one peer mines it: the block comes while it waits.
  const before = new Set(chain.json<string[]>("getrawmempool"));
  await wd.script(
    `window.__dwBroadcast = null;
     window.__TAURI_INTERNALS__.invoke("draft_broadcast", arguments[0]).then((ok) => (window.__dwBroadcast = { ok }), (err) => (window.__dwBroadcast = { err }));`,
    [{ draft, prepared: prepared.handle }],
  );
  const sent = await poll(LOAD_WAIT_MS, "the payment in dashd's mempool", async () => chain.json<string[]>("getrawmempool").find((t) => !before.has(t)));
  if (paysTo(chain.json<RawTx>("getrawtransaction", sent, "true"), payee) !== 1.25 * DUFFS) throw new Error("the payment does not pay 1.25 tDASH");
  const fee = Math.round(chain.json<{ fees: { base: number } }>("getmempoolentry", sent).fees.base * DUFFS);
  chain.mine(1);
  const outcome = await poll(LOAD_WAIT_MS, "the broadcast's outcome", async () => (await wd.script<{ ok?: { txid: string }; err?: { code: string } } | null>("return window.__dwBroadcast")) ?? undefined);
  if (outcome.ok ? outcome.ok.txid !== sent : outcome.err?.code !== "send.broadcast_unknown") throw new Error(`draft_broadcast: ${JSON.stringify(outcome)}`);
  chain.mine(1);
  await poll(LOAD_WAIT_MS, "the payment confirmed in history", async () => {
    const d = await invoke<{ status: { confirmations: number } }>("tx_detail", { walletId: main, txid: sent }).catch(() => null);
    return d && d.status.confirmations >= 2 ? true : undefined;
  });
  step(`paid ${payee} 1.25 tDASH in ${sent}, fee ${fee} duffs`);

  let watchOnly: { name: string; xpub: string; walletId: string; funding: string } | undefined;
  let locked: { txid: string; vout: number }[] = [];
  if (!v010) {
    const coins = await invoke<{ outpoint: { txid: string; vout: number }; isChange: boolean }[]>("utxos", {
      walletId: main,
      filter: { includeLocked: true, fullyMixedOnly: false, minConfirmations: null },
    });
    locked = coins.filter((c) => !c.isChange).map((c) => c.outpoint);
    if (locked.length !== 1) throw new Error(`expected one non-change coin left in the main wallet, found ${locked.length}`);
    await invoke("lock_outpoints", { walletId: main, outpoints: locked });
    const vector = WALLETS[1]!;
    const own = (await invoke<{ xpub: string }>("account_xpub", { walletId: second, account: 0 })).xpub;
    if (accountTpub(vector.mnemonic, vector.bip39, 0) !== own) throw new Error("the writer's BIP32 derivation disagrees with the release's account 0");
    // BIP39's own test vector for the main mnemonic carries the passphrase TREZOR: another seed, no wallet here.
    const xpub = accountTpub(WALLETS[0]!.mnemonic, "TREZOR", 0);
    const name = "Fixture watch-only";
    const walletId = await invoke<string>("import_watch_only", { xpub, options: { name, birthHeight: 0, lookahead: null } });
    ids.push(walletId);
    const to = await addr(walletId);
    const watchFunding = chain.cli("-rpcwallet=miner", "sendtoaddress", to, "0.75");
    chain.mine(1);
    await poll(LOAD_WAIT_MS, "the watch-only wallet's funding", async () => ((await balanceOf(walletId))?.confirmed === 0.75 * DUFFS ? true : undefined));
    watchOnly = { name, xpub, walletId, funding: watchFunding };
    step(`locked ${locked.map((o) => `${o.txid}:${o.vout}`).join(", ")}; watch-only ${walletId.slice(0, 12)} (abandon … about with TREZOR, account 0) holds 0.75 tDASH`);
  }

  await invoke("set_tx_label", { walletId: main, txid: funding.mainFirst, label: "Opening deposit" });
  const contact = chain.cli("-rpcwallet=miner", "getnewaddress");
  await invoke("save_address_book_entry", { walletId: main, address: contact, label: "Fixture contact", purpose: "send", replace: false });
  await invoke("create_receive_request", { walletId: main, amount: DUFFS / 2, label: "Invoice 7", message: "fixture request", includeUsername: null });
  // Off is what a later build reads for an unset row: on, for the main wallet only, shows a lost one.
  await invoke("set_username_in_requests", { walletId: main, on: !v010 });
  await invoke("set_dust_protection", { threshold: 12_345 });
  await invoke("set_backup_policy", { keep: 7 });
  await invoke(v010 ? "set_network_choice" : "switch_network", { network: "testnet" });
  const cj = await invoke<Record<string, unknown>>("coinjoin_settings");
  await invoke("set_coinjoin_settings", { settings: { ...cj, multiSession: true, maxSessions: 3, rounds: 6, targetAmountDash: 25 } });
  await invoke("set_ui_settings", {
    settings: { theme: "dark", unit: "milliDash", decimalDigits: 4, autohideBalance: true, autoLockSecs: 900, subtractFeeByDefault: true, showMasternodes: true, masternodesItem: "on", preReleaseNoticeDismissed: true },
  });
  const height = chain.height();
  await poll(LOAD_WAIT_MS, "the wallets at dashd's tip", async () => {
    const scanned = await Promise.all(ids.map((id) => invoke<number | null>("wallet_scan_height", { walletId: id })));
    return scanned.every((h) => h === height) ? true : undefined;
  });
  await invoke("stop_spv").catch(() => undefined);
  await close(wd, dataRoot);
  step(`settings saved; the app closed at height ${height}`);

  // ---------------------------------------------------------------- phase 2: what the release shows
  ({ wd, invoke } = await launch(dataRoot, ["--no-sync"]));
  await invoke("unlock", { passphrase: PASSPHRASE });
  const calls: Call[] = [];
  const record = async <T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> => {
    const result = await invoke<T>(cmd, args);
    calls.push({ cmd, args, result });
    return result;
  };
  for (const cmd of ["lock_status", "ui_settings", "get_network_choice", "active_network", "existing_networks", "payment_authentication", "quick_unlock_policy", "coinjoin_settings", "dust_protection", "backup_policy", "list_wallets", "wallet_load_states"])
    await record(cmd);
  const history: Record<string, { txid: string; amount: number; category: string; blockHeight: number | null }[]> = {};
  type Signature = { walletId: string; address: string; message: string; signature: string };
  const signatures: Signature[] = [];
  const fundedSignatures: Signature[] = [];
  for (const walletId of ids) {
    for (const cmd of ["wallet_info", "balance", "wallet_scan_height", "current_receive_address", "receive_requests", "username_in_requests", "locked_outpoints", "coinjoin_salt"]) await record(cmd, { walletId });
    await record("account_xpub", { walletId, account: 0 });
    await record("addresses", { walletId, filter: { chain: null, used: null } });
    await record("address_book", { walletId, purpose: null, search: null });
    await record("utxos", { walletId, filter: { includeLocked: true, fullyMixedOnly: false, minConfirmations: null } });
    const rows = (await record<{ rows: { txid: string; amount: number; category: string; blockHeight: number | null }[] }>("history_page", {
      walletId,
      request: { sort: "oldestFirst", cursor: null, limit: 100, categories: [], text: null },
    })).rows;
    history[walletId] = rows;
    for (const txid of [...new Set(rows.map((r) => r.txid))]) {
      await record("tx_detail", { walletId, txid });
      await record("tx_label", { walletId, txid });
    }
    if (walletId === watchOnly?.walletId) continue;
    const signed = async (address: string) => {
      const grant = await invoke<{ id: string }>("authorize", { purpose: { kind: "signMessage" }, walletId, credential: { kind: "passphrase", passphrase: PASSPHRASE } });
      const signature = await invoke<string>("sign_message", { walletId, address, message: MESSAGE, grantId: grant.id });
      if (chain.cli("verifymessage", address, signature, MESSAGE) !== "true") throw new Error(`dashd does not verify the signature by ${address}`);
      return { walletId, address, message: MESSAGE, signature };
    };
    signatures.push(await signed((await invoke<{ address: string }>("current_receive_address", { walletId })).address));
    if (v010) continue;
    const coins = calls.findLast((c) => c.cmd === "utxos" && c.args.walletId === walletId)!.result as Coin[];
    for (const coin of coins) {
      const out = chain.cli("gettxout", coin.outpoint.txid, String(coin.outpoint.vout), "true");
      const problem = outpointProblem(coin, out ? (JSON.parse(out) as TxOut) : null);
      if (problem) throw new Error(problem);
    }
    for (const address of [...new Set(coins.map((c) => c.address))].sort()) fundedSignatures.push(await signed(address));
  }
  await close(wd, dataRoot);

  const answer = <T>(cmd: string, walletId: string) => calls.find((c) => c.cmd === cmd && c.args.walletId === walletId)!.result as T;
  const balances = ids.map((id) => answer<{ confirmed: number }>("balance", id).confirmed);
  const want = [12.5 * DUFFS - 1.25 * DUFFS - fee, 3 * DUFFS, ...(watchOnly ? [0.75 * DUFFS] : [])];
  if (balances.join() !== want.join()) throw new Error(`balances ${balances.join(", ")} disagree with dashd's payments and fee (${want.join(", ")})`);
  if (!v010) {
    const flags = ids.map((id) => answer<boolean>("username_in_requests", id));
    if (flags.join() !== "true,false,false") throw new Error(`username in requests reads ${flags.join(", ")}, set for the main wallet only`);
    const lockedNow = answer<{ txid: string; vout: number }[]>("locked_outpoints", main);
    if (JSON.stringify(lockedNow) !== JSON.stringify(locked)) throw new Error(`locked outpoints ${JSON.stringify(lockedNow)}, locked ${JSON.stringify(locked)}`);
  }
  const txids = new Set(Object.values(history).flat().map((r) => r.txid));
  for (const t of [...Object.values(funding), sent, ...(watchOnly ? [watchOnly.funding] : [])]) if (!txids.has(t)) throw new Error(`${t} is missing from the release's history`);
  step(`recorded ${calls.length} answers; balances ${balances.join(", ")} duffs agree with dashd; ${txids.size} transactions`);

  // ---------------------------------------------------------------- the fixture
  mkdirSync(dirname(OUT), { recursive: true });
  mkdirSync(stage);
  staging = stage;
  cpSync(dataRoot, join(stage, "data"), { recursive: true, filter: (src) => !src.endsWith("instance.lock") });
  const expected = {
    version: VERSION,
    appVersion: launched.appVersion,
    release: { asset: process.env.DW_UPGRADE_ASSET ?? null, sha256: process.env.DW_UPGRADE_ASSET_SHA256 ?? null },
    network: "regtest",
    passphrase: PASSPHRASE,
    dataRoot: "$DATA_ROOT",
    wallets: WALLETS.map((w, i) => ({ ...w, walletId: ids[i] })),
    dashd: {
      version: dashdVersion,
      height,
      funding,
      ...(v010 ? {} : { deposits: { [main]: 12.5 * DUFFS, [second]: 3 * DUFFS, ...(watchOnly ? { [watchOnly.walletId]: 0.75 * DUFFS } : {}) } }),
      payment: { txid: sent, to: payee, duffs: 1.25 * DUFFS, ...(v010 ? {} : { walletId: main, feeDuffs: fee }) },
    },
    ...(watchOnly ? { watchOnly } : {}),
    ...(v010 ? {} : { lockedOutpoints: { [main]: locked } }),
    signatures,
    ...(v010 ? {} : { fundedSignatures }),
    calls: JSON.parse(JSON.stringify(calls).replaceAll(JSON.stringify(dataRoot).slice(1, -1), "$DATA_ROOT")),
  };
  writeFileSync(join(stage, "expected.json"), `${JSON.stringify(expected, null, 2)}\n`);
  if (present(OUT)) throw new Error(`${OUT} appeared during the run; the capture is not kept`);
  renameSync(stage, OUT);
  staging = undefined;
  const files = (dir: string): string[] =>
    readdirSync(dir).flatMap((f) => {
      const p = join(dir, f);
      return statSync(p).isDirectory() ? files(p) : [p];
    });
  step(`fixture in ${OUT}: ${files(join(OUT, "data")).length} files`);
}

main()
  .catch((e) => {
    console.error(`upgrade-fixture: FAIL: ${e instanceof Error ? e.message : String(e)}`);
    process.exitCode = 1;
  })
  .finally(async () => {
    stopDriver(driver);
    await node?.stop();
    if (work) rmSync(work, { recursive: true, force: true, maxRetries: 3 });
    if (staging) rmSync(staging, { recursive: true, force: true, maxRetries: 3 });
  });
