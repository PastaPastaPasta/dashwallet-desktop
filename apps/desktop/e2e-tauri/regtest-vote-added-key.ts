// From the UI on regtest: track a masternode this wallet did not derive, add its voting key, and
// vote with it (MG-10 §1.3–1.4, MG-00 D13 amended for 1.0). tauri-driver drives the production
// renderer on the real engine. The chain is the regtest suite `governance_added_key_ui`
// (regtest/functional/dwd_governance_added_key_ui.py): 3 masternodes, one of them voting with a key
// Core generated for this run, and a proposal to vote on. That suite starts this file and is the
// judge: Core must hold the vote.
//
// Flow: unlock → Show Masternodes → Governance on, the list syncs and shows the proposal → My
// masternodes ▸ More ▸ Track a masternode…: find it by ProTx hash, Track → Add keys opens: the
// voting key, typed only into the masked field → "Matches this masternode's voting key." → the
// tracked row says the key matches → Governance ▸ the proposal ▸ Vote…: the masternode is listed
// with "key you added", Yes, passphrase, Vote Yes → "1 of 1 vote sent". Screenshots (none while the
// key is in a field), the app's log and ui.json (what the UI showed) go to $DW_E2E_OUT; none may
// hold the key. The suite writes result.json once Core has judged. tauri-driver and WebKitWebDriver
// listen on free ports, and the run talks to them only once both listeners are in the group it
// spawned; that group's id goes to $DW_E2E_OUT/driver.pid, so the suite can stop it on a timeout.
//
//   DW_APP=…/dash-wallet DWCLI=…/dwcli TAURI_DRIVER=…/tauri-driver DW_E2E_DATADIR=… \
//   DW_E2E_PASSPHRASE=… DW_E2E_SPV_PEER=127.0.0.1:… DW_E2E_RPC=http://u:p@127.0.0.1:… \
//   DW_E2E_PROPOSAL=<name> DW_E2E_VOTER=<proTxHash> DW_E2E_VOTING_KEY=<WIF> node e2e-tauri/regtest-vote-added-key.ts
// `scripts/e2e-regtest-vote-added-key.sh` builds the binaries and runs the suite. Not in CI: it
// needs dashd and a private X display.

import type { ChildProcess } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { freePort } from "./dashd.ts";
import { APP, CHAIN, driverGone, driverReady, env, fillSecret, leaks, poll, spawnDriver, stopDriver, tid, TIMEOUT_MS, WebDriver } from "./driver.ts";
import { ownedProbe, processGroup, ssListing } from "./listeners.ts";

const OUT = resolve(env("DW_E2E_OUT", "test-results/tauri-regtest-vote-added-key"));
const DATADIR = resolve(env("DW_E2E_DATADIR"));
const PASSPHRASE = env("DW_E2E_PASSPHRASE");
const PEER = env("DW_E2E_SPV_PEER");
const RPC = new URL(env("DW_E2E_RPC"));
const PROPOSAL = env("DW_E2E_PROPOSAL");
const VOTER = env("DW_E2E_VOTER");
const VOTING_KEY = env("DW_E2E_VOTING_KEY");
// Not passed on to tauri-driver, WebKitWebDriver or the app.
delete process.env.DW_E2E_VOTING_KEY;
/** SPV, the masternode list and governance sync on a loaded host (as regtest-send.ts allows). */
const LOAD_WAIT_MS = Number(env("DW_E2E_LOAD_WAIT_MS", "600000"));
const SHORT = VOTER.slice(0, 8);

let driver: ChildProcess | undefined;

async function main() {
  mkdirSync(OUT, { recursive: true });
  const steps: string[] = [];
  const evidence: Record<string, unknown> = {};
  const step = (s: string) => {
    steps.push(s);
    console.log(`regtest-vote-added-key: ${s}`);
  };

  const port = await freePort();
  let native = await freePort();
  while (native === port) native = await freePort();
  driver = spawnDriver(port, OUT, native);
  writeFileSync(resolve(OUT, "driver.pid"), `${driver.pid}\n`);
  await poll(30_000, "both drivers to listen", ownedProbe(ssListing, [port, native], driver.pid, processGroup));
  const wd = new WebDriver(await driverReady(port), OUT);
  // Regtest has no DAPI: masternodes' ProRegTxs, which a vote's collateral comes from, are read from
  // dashd (MG-02 D80, MG-09 D28).
  const args = ["--datadir", DATADIR, ...CHAIN, "--quorum-url", "http://127.0.0.1:1", "--collateral-rpc", RPC.href, "--sync", "--spv-peer", PEER, "--theme", "light"];
  await wd.start({ alwaysMatch: { "tauri:options": { application: APP, args } } });
  const textOf = (id: string) => wd.script<string>(`return document.querySelector(arguments[0])?.textContent ?? ""`, [tid(id)]);
  const exists = (id: string) => wd.script<boolean>(`return !!document.querySelector(arguments[0])`, [tid(id)]);
  const enabled = (id: string) => wd.script<boolean>(`const b = document.querySelector(arguments[0]); return !!b && !b.disabled`, [tid(id)]);
  const textIs = (id: string, ...want: string[]) => async () => {
    const t = (await textOf(id)).trim();
    return want.includes(t) ? t : undefined;
  };
  /** Types into an input until it holds the whole text (WebKitWebDriver can drop characters). */
  const enter = async (id: string, text: string) => {
    for (let i = 0; i < 3; i++) {
      await wd.clear(await wd.waitFor(tid(id)));
      await wd.fill(tid(id), text);
      if ((await wd.script<number>(`return document.querySelector(arguments[0])?.value.length ?? -1`, [tid(id)])) === text.length) return;
    }
    throw new Error(`${id} did not take ${text.length} characters`);
  };
  const invoke = async <T>(command: string, a: Record<string, unknown> = {}): Promise<T> => {
    const r = await wd.scriptAsync<{ ok?: T; err?: string }>(
      `const done = arguments[arguments.length - 1];
       window.__TAURI_INTERNALS__.invoke(arguments[0], arguments[1]).then((ok) => done({ ok }), (e) => done({ err: e?.code ?? String(e) }));`,
      [command, a],
    );
    if (r.err !== undefined) throw new Error(`${command}: ${r.err}`);
    return r.ok as T;
  };
  /** A capture is never taken while the key field holds anything, masked or not. */
  const keyFieldFilled = () => wd.script<boolean>(`return !!document.querySelector(arguments[0])?.value`, [tid("mn-add-key-input")]);
  /** Types the voting key into the Add key sheet's masked field (fillSecret checks it) and submits. */
  const addVotingKey = async () => {
    for (let i = 0; i < 3; i++) {
      await wd.clear(await wd.waitFor(tid("mn-add-key-input")));
      await fillSecret(wd, tid("mn-add-key-input"), VOTING_KEY);
      if ((await wd.script<number>(`return document.querySelector(arguments[0])?.value.length ?? -1`, [tid("mn-add-key-input")])) === VOTING_KEY.length)
        return wd.press(tid("mn-add-key-submit"));
    }
    await wd.clear(await wd.waitFor(tid("mn-add-key-input")));
    throw new Error(`mn-add-key-input did not take ${VOTING_KEY.length} characters`);
  };
  try {
    await enter("lock-field", PASSPHRASE);
    await wd.press(tid("lock-submit"));
    await wd.waitGone(tid("lock-screen"), "the lock screen");
    if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");
    step("unlocked the real vault; SPV syncs from the suite's controller node");

    const prefs = await invoke<Record<string, unknown>>("ui_settings");
    await invoke("set_ui_settings", { settings: { ...prefs, masternodesItem: "on" } });
    await wd.script("location.reload()");
    await wd.waitFor(tid("sidebar-masternodes"), "Masternodes in the sidebar (Show Masternodes on)", LOAD_WAIT_MS);
    await wd.press(tid("sidebar-masternodes"));
    await wd.waitFor(tid("page-masternodes"), "the Masternodes page");
    await wd.press(tid("masternodes-tabs-governance"));
    const off = await poll(TIMEOUT_MS, "the governance state", async () =>
      (await exists("governance-disabled")) ? true : (await exists("governance-clock")) || (await exists("governance-syncing")) ? false : undefined,
    );
    if (off) await wd.press(tid("governance-turn-on"));
    const row = `proposal-row-${PROPOSAL}`;
    await wd.waitFor(tid(row), `${PROPOSAL} in the synced list`, LOAD_WAIT_MS);
    step(`governance ${off ? "was off: Turn on" : "already on"}; ${PROPOSAL} is listed`);

    // Track: find the masternode by ProTx hash once the masternode list has synced.
    await wd.press(tid("masternodes-tabs-myMasternodes"));
    await wd.press(tid("mn-page-more"));
    await wd.press(tid("mn-page-more-track"));
    await enter("mn-track-query", VOTER);
    const result = `mn-track-result-${VOTER}`;
    await poll(LOAD_WAIT_MS, "the masternode in Track's results", async () => {
      if (await exists(result)) return true;
      if (await enabled("mn-track-find")) {
        await wd.press(tid("mn-track-find"));
        await new Promise((r) => setTimeout(r, 2000));
      }
      return undefined;
    });
    evidence.trackResult = await textOf(result);
    if (!(await enabled("mn-track-submit"))) throw new Error(`Track is off for the masternode: ${evidence.trackResult}`);
    await wd.screenshot("1-track-found");
    step(`Track found ${SHORT}…: "${evidence.trackResult}"`);
    await wd.press(tid("mn-track-submit"));
    await wd.waitGone(tid("mn-track-sheet"), "the track sheet");

    // Add keys follows Track. It asks for the full unlock first when the vault isn't fully unlocked.
    await wd.waitFor(tid("mn-add-key-sheet"), "Add keys after Track");
    if (await poll(TIMEOUT_MS, "the key field or Add keys' Unlock", async () =>
      (await exists("mn-add-key-input")) ? false : (await exists("mn-add-key-unlock")) ? true : undefined,
    )) {
      await wd.press(tid("mn-add-key-unlock"));
      await fillSecret(wd, tid("unlock-sheet-field"), PASSPHRASE);
      await wd.press(tid("unlock-sheet-submit"));
      await wd.waitGone(tid("unlock-sheet"), "the unlock sheet");
      await wd.press(tid("mn-more"));
      await wd.press(tid("mn-more-add-keys"));
      step("Add keys asked for the full unlock: unlocked");
    }
    evidence.addKeyRole = await textOf("mn-add-key-role");
    if (!String(evidence.addKeyRole).includes("Voting key")) throw new Error(`Add keys offers ${evidence.addKeyRole}`);
    await addVotingKey();
    evidence.addKeyResult = await poll(TIMEOUT_MS, "Add key's result", async () => {
      if (await exists("mn-add-key-result")) return (await textOf("mn-add-key-result")).trim();
      const err = (await textOf("mn-add-key-error")).trim();
      if (err) throw new Error(`Add key: ${err}`);
      return undefined;
    });
    await wd.screenshot("2-add-key-result");
    if (evidence.addKeyResult !== "Matches this masternode's voting key.") throw new Error(`Add key: ${evidence.addKeyResult}`);
    step(`Add keys (${evidence.addKeyRole}): "${evidence.addKeyResult}"`);
    await wd.press(tid("mn-add-key-done"));
    await wd.waitGone(tid("mn-add-key-sheet"), "the add key sheet");

    await wd.waitFor(tid(`mn-row-${VOTER}`), "the tracked masternode's row");
    // The pane reads the key's check again; a match with the synced list is what lets it vote (D29).
    let keyLine = "";
    evidence.keyAdded = await poll(TIMEOUT_MS, "Keys you added to say the voting key matches", async () => {
      keyLine = (await textOf("mn-key-added-voting")).trim();
      return keyLine.includes("· matches") ? keyLine : undefined;
    }).catch(() => {
      throw new Error(`Keys you added says "${keyLine}", not that the voting key matches`);
    });
    evidence.trackedRow = await textOf(`mn-row-${VOTER}`);
    await wd.screenshot("3-tracked-row");
    step(`tracked row "${evidence.trackedRow}"; ${evidence.keyAdded}`);

    // Vote with it.
    await wd.press(tid("masternodes-tabs-governance"));
    await wd.waitFor(tid(row), `${PROPOSAL} in the list`);
    await wd.press(tid(row));
    await poll(LOAD_WAIT_MS, "Vote… enabled (a voting key in the wallet)", async () => ((await enabled("proposal-vote")) ? true : undefined));
    await wd.press(tid("proposal-vote"));
    // The row turns votable once governance has the masternode's collateral too, which it reads
    // apart from the list the key was matched against; a row votable only after the sheet opened
    // starts unticked.
    const node = `vote-node-${SHORT}`;
    let seen = "";
    const voter = await poll(LOAD_WAIT_MS, "the tracked masternode votable with the key you added", async () => {
      const r = await wd.script<{ text: string; enabled: boolean; checked: boolean } | null>(
        `const li = document.querySelector(arguments[0]); const box = li?.querySelector("input[type=checkbox]");
         return li && box ? { text: li.textContent ?? "", enabled: !box.disabled, checked: box.checked } : null`,
        [tid(node)],
      );
      seen = r?.text.trim() ?? "";
      return r?.enabled && r.text.includes("key you added") ? r : undefined;
    }).catch(() => {
      throw new Error(`the vote sheet's row: "${seen}"`);
    });
    if (!voter.checked) await wd.script(`document.querySelector(arguments[0]).querySelector("input[type=checkbox]").click()`, [tid(node)]);
    evidence.voteRow = voter.text.trim();
    await wd.press(tid("vote-outcome-yes"));
    await enter("vote-passphrase", PASSPHRASE);
    await wd.screenshot("4-vote-sheet");
    await wd.press(tid("vote-send"));
    evidence.voteResult = await poll(TIMEOUT_MS, "the vote's result", textIs("vote-result-title", "1 of 1 vote sent", "0 of 1 vote sent"));
    evidence.voteResultRow = await textOf(`vote-result-${SHORT}`);
    await wd.screenshot("5-vote-result");
    if (evidence.voteResult !== "1 of 1 vote sent") throw new Error(`the vote was not sent: ${evidence.voteResultRow}`);
    step(`Vote… Yes with "${evidence.voteRow}": "${evidence.voteResult}" (${evidence.voteResultRow})`);
  } catch (e) {
    if (!(await keyFieldFilled().catch(() => true))) await wd.screenshot("failure").catch(() => undefined);
    throw e;
  } finally {
    await wd.end().catch(() => undefined);
  }
  writeFileSync(resolve(OUT, "ui.json"), JSON.stringify({ steps, evidence }, null, 2));
  console.log(`regtest-vote-added-key: the UI's ${steps.length} steps done; Core judges the vote`);
}

main()
  .catch((e) => {
    console.error(`regtest-vote-added-key: FAIL: ${e instanceof Error ? e.message : String(e)}`);
    process.exitCode = 1;
  })
  .finally(async () => {
    stopDriver(driver);
    await driverGone(driver);
    const found = leaks(OUT, [VOTING_KEY]);
    if (found.length) {
      console.error(`regtest-vote-added-key: FAIL: the voting key in plain text: ${found.join(", ")}`);
      process.exitCode = 1;
    }
  });
