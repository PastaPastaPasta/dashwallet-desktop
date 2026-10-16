// U-06 acceptance (ROADMAP U-06, D12): mixing starts and stops from the UI on the real engine.
// tauri-driver drives the production renderer on a dashd regtest chain with no masternodes, so a
// running mixer waits for them (S5 "Paused", Core's "No Masternodes detected."); the rounds
// themselves are the regtest `coinjoin` suite's (dwd_coinjoin_client.py), on the same engine calls.
//
// Flow: dashd regtest, 101 blocks to its miner → dwcli makes an encrypted vault and a wallet →
// dashd pays it 10 DASH and mines it → the app syncs from dashd alone and is unlocked → CoinJoin
// page: Not Started once SPV sees the funds → Advanced (8 rounds) → Start Mixing → the engine
// reports a running mixer (Starting…, then Paused or Mixing) → the vault locks: mixing stops with
// `vaultLocked` (S10) → unlock for mixing only → Start Mixing → Stop Mixing, confirmed → Not
// Started. Screenshots, the app's log, dashd's log and result.json (the evidence) go to $DW_E2E_OUT.
//
//   DW_APP=…/dash-wallet DWCLI=…/dwcli TAURI_DRIVER=…/tauri-driver DASHD=…/dashd \
//     node e2e-tauri/regtest-coinjoin.ts
// `scripts/e2e-regtest-coinjoin.sh` builds the binaries, finds dashd and runs it under a private
// session bus. It needs a private X display. Not in CI: it needs dashd.

import type { ChildProcess } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { Dashd, freePort } from "./dashd.ts";
import { APP, CHAIN, dwcli, env, poll, startDriver, stopDriver, tid, TIMEOUT_MS, WebDriver } from "./driver.ts";

const OUT = resolve(env("DW_E2E_OUT", "test-results/tauri-regtest-coinjoin"));
const DASHD = resolve(env("DASHD"));
const PORT = Number(env("DW_E2E_PORT", "4774"));
const PASSPHRASE = "e2e regtest passphrase";
const FUNDING = "10";
/** SPV sync on a loaded host (as regtest-send.ts allows). */
const LOAD_WAIT_MS = Number(env("DW_E2E_LOAD_WAIT_MS", "600000"));
/** States of a running mixer (docs/design/U-06-coinjoin.md S4–S7). */
const RUNNING = ["starting", "paused", "mixing", "finished"];

let driver: ChildProcess | undefined;
let node: Dashd | undefined;
let work: string | undefined;

async function main() {
  mkdirSync(OUT, { recursive: true });
  work = mkdtempSync(join(tmpdir(), "dw-regtest-coinjoin-")); // mode 0700
  const dataRoot = join(work, "wallet");
  const passFile = join(work, "passphrase");
  writeFileSync(passFile, `${PASSPHRASE}\n`, { mode: 0o600 });
  const steps: string[] = [];
  const evidence: Record<string, unknown> = {};
  const step = (s: string) => {
    steps.push(s);
    console.log(`regtest-coinjoin: ${s}`);
  };

  const nodeDir = join(work, "dashd");
  mkdirSync(nodeDir);
  node = new Dashd(DASHD, nodeDir, await freePort(), await freePort(), join(OUT, "dashd.log"));
  const chain = node;
  await poll(300_000, "dashd's RPC", async () => (chain.tryCli("getblockchaininfo").ok ? true : undefined));
  evidence.dashd = chain.json<{ subversion: string }>("getnetworkinfo").subversion;
  chain.cli("createwallet", "miner");
  chain.mine(101);
  step(`dashd ${evidence.dashd} regtest: 101 blocks mined to its miner wallet, no masternodes`);

  dwcli(dataRoot, passFile, "init-vault");
  const walletId = /wallet_id (\S+)/.exec(dwcli(dataRoot, passFile, "create"))?.[1];
  if (!walletId) throw new Error("dwcli create printed no wallet id");
  const address = /address (\S+)/.exec(dwcli(dataRoot, passFile, "receive"))?.[1];
  if (!address) throw new Error("dwcli receive printed no address");
  evidence.fundingTxid = chain.cli("-rpcwallet=miner", "sendtoaddress", address, FUNDING);
  chain.mine(1);
  step(`wallet ${walletId.slice(0, 16)}… funded with ${FUNDING} DASH (mined at ${chain.height()})`);

  const started = await startDriver(PORT, OUT);
  driver = started.driver;
  const wd = new WebDriver(started.base, OUT);
  const args = ["--datadir", dataRoot, ...CHAIN, "--sync", "--spv-peer", `127.0.0.1:${chain.p2pPort}`, "--theme", "light"];
  await wd.start({ alwaysMatch: { "tauri:options": { application: APP, args } } });
  const attr = (id: string, name: string) => wd.script<string | null>(`return document.querySelector('${tid(id)}')?.getAttribute('${name}') ?? null`);
  const textOf = (id: string) => wd.script<string>(`return document.querySelector('${tid(id)}')?.textContent ?? ""`);
  const stateIs = (id: string, ...states: string[]) => async () => {
    const s = await attr(id, "data-state");
    return s !== null && states.includes(s) ? s : undefined;
  };
  const unlock = async (mixingOnly: boolean) => {
    await wd.fill(tid("lock-field"), PASSPHRASE);
    if (mixingOnly) await wd.press(tid("lock-mixing-only"));
    await wd.press(tid("lock-submit"));
    await wd.waitGone(tid("lock-screen"), "the lock screen");
  };
  try {
    await unlock(false);
    if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");
    step("unlocked the real vault; SPV syncs from dashd alone");

    await wd.press(tid("sidebar-coinJoin"));
    await wd.waitFor(tid("coinjoin-status"), "the CoinJoin page");
    // Too little to mix until SPV has seen the funding (S12), then Not Started (S14).
    await poll(LOAD_WAIT_MS, "the funded wallet's mixing status", stateIs("coinjoin-status", "off"));
    await wd.screenshot("1-not-started");
    step(`CoinJoin page: ${await textOf("coinjoin-status-title")}, ${await textOf("coinjoin-status-mixed")}`);

    await wd.press(`${tid("coinjoin-level-advanced")} input`);
    await poll(TIMEOUT_MS, "Advanced to be saved", async () =>
      (await wd.script<boolean>(`return document.querySelector('${tid("coinjoin-level-advanced")} input').checked`)) ? true : undefined,
    );
    // The radio follows the settings the write's refetch read back from the engine, and the
    // status's rounds are the engine's.
    await wd.press(`${tid("coinjoin-details")} summary`);
    evidence.amountAndRounds = await poll(TIMEOUT_MS, "8 rounds in the engine's status", async () => {
      const t = await textOf("coinjoin-detail-amount");
      return t.includes("8 Rounds") ? t : undefined;
    });
    step(`level: Advanced; the engine's status says "${evidence.amountAndRounds}"`);

    // Start: the engine runs a mixer, which waits for masternodes.
    await wd.press(tid("coinjoin-action"));
    evidence.firstRunning = await poll(TIMEOUT_MS, "a running mixer", stateIs("coinjoin-status", ...RUNNING));
    evidence.running = await poll(LOAD_WAIT_MS, "the mixer's first step", stateIs("coinjoin-status", "paused", "mixing", "finished"));
    evidence.runningLine = await textOf("coinjoin-status-line");
    await wd.screenshot("2-mixing-started");
    step(`Start Mixing: ${evidence.firstRunning} → ${evidence.running} ("${evidence.runningLine}")`);
    await wd.press(tid("sidebar-home"));
    await poll(TIMEOUT_MS, "the Home card", stateIs("coinjoin-card", ...RUNNING));
    evidence.homeCard = await textOf("coinjoin-card");
    await wd.screenshot("3-home-card");
    step(`Home card: "${evidence.homeCard}"`);

    // A lock stops mixing (S10); mixing-only unlock lets it start again (D6).
    await wd.press(tid("toolbar-lock"));
    await wd.waitFor(tid("lock-screen"), "the lock screen");
    // The shell stays under the lock screen: wait for the mixing loop's halt (its next tick, after
    // any step in progress) before unlocking, or it would never see the vault locked.
    await poll(LOAD_WAIT_MS, "mixing to stop on the lock", stateIs("coinjoin-card", "locked"));
    await unlock(true);
    await poll(TIMEOUT_MS, "the stopped card", stateIs("coinjoin-card", "locked"));
    evidence.lockedLine = await textOf("coinjoin-card-line");
    await wd.screenshot("4-stopped-by-lock");
    step(`locked, unlocked for mixing only: the Home card says "${evidence.lockedLine}"`);

    await wd.press(tid("coinjoin-card-open"));
    await wd.waitFor(tid("coinjoin-status"), "the CoinJoin page");
    evidence.lockLine = await textOf("coinjoin-lock-line");
    await wd.press(tid("coinjoin-action"));
    evidence.restarted = await poll(TIMEOUT_MS, "mixing again", stateIs("coinjoin-status", ...RUNNING));
    step(`Start Mixing unlocked for mixing only ("${evidence.lockLine}"): ${evidence.restarted}`);

    // Start's own call may still be out (the button is busy): Stop once it can be pressed.
    await poll(TIMEOUT_MS, "Stop Mixing", async () =>
      (await wd.script<boolean>(`const b = document.querySelector('${tid("coinjoin-action")}'); return !!b && !b.disabled && b.textContent === "Stop Mixing"`))
        ? true
        : undefined,
    );
    await wd.press(tid("coinjoin-action"));
    await wd.press(tid("coinjoin-stop-confirm"));
    await poll(LOAD_WAIT_MS, "mixing to stop", stateIs("coinjoin-status", "off"));
    await wd.screenshot("5-stopped");
    step(`Stop Mixing, confirmed: ${await textOf("coinjoin-status-title")}`);
  } catch (e) {
    await wd.screenshot("failure").catch(() => undefined);
    throw e;
  } finally {
    await wd.end().catch(() => undefined);
  }
  writeFileSync(join(OUT, "result.json"), JSON.stringify({ ok: true, steps, evidence }, null, 2));
  console.log(`regtest-coinjoin: PASS (${steps.length} steps); evidence in ${OUT}`);
}

main()
  .catch((e) => {
    console.error(`regtest-coinjoin: FAIL: ${e instanceof Error ? e.message : String(e)}`);
    process.exitCode = 1;
  })
  .finally(async () => {
    stopDriver(driver);
    await node?.stop();
    try {
      if (work) rmSync(work, { recursive: true, force: true, maxRetries: 3 });
    } catch (e) {
      console.warn(`regtest-coinjoin: could not remove ${work}: ${e instanceof Error ? e.message : String(e)}`);
    }
  });
