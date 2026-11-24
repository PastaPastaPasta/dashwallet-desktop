// MG-10 S3 acceptance (GL `--part update`): from the UI on regtest, change a masternode's payout and
// voting key with Change keys or payout (a ProUpRegTx signed with the wallet's owner key).
// tauri-driver drives the production renderer on the real engine. The chain is the regtest suite
// `update_registrar_ui` (regtest/functional/dwd_update_registrar_ui.py): the wallet registered
// masternode F with its keychain's owner key 0. That suite starts this file and is the judge: Core's
// `protx info` must show the new payout and voting key.
//
// Flow: unlock → Show Masternodes → F in My masternodes → Change keys or payout… (offered once the
// list is synced and the wallet holds F's registration) → voting: this wallet's next key; payout:
// another address (Core's) → the review says both change → passphrase, Send change → "Change sent"
// → the transaction in dashd's mempool, one block. Screenshots, the app's log and result.json (the
// evidence) go to $DW_E2E_OUT.
//
//   DW_APP=…/dash-wallet DWCLI=…/dwcli TAURI_DRIVER=…/tauri-driver DW_E2E_DATADIR=… \
//   DW_E2E_PASSPHRASE=… DW_E2E_WALLET=… DW_E2E_SPV_PEER=127.0.0.1:… DW_E2E_RPC=http://u:p@127.0.0.1:… \
//   DW_E2E_MINER=<address> DW_E2E_MASTERNODE=<proTxHash> DW_E2E_PAYOUT=<address> node e2e-tauri/regtest-update-registrar.ts
// `scripts/e2e-regtest-update-registrar.sh` builds the binaries and runs the suite. Not in CI: it
// needs dashd and a private X display.

import type { ChildProcess } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { APP, CHAIN, env, poll, startDriver, stopDriver, tid, TIMEOUT_MS, WebDriver } from "./driver.ts";

const OUT = resolve(env("DW_E2E_OUT", "test-results/tauri-regtest-update-registrar"));
const DATADIR = resolve(env("DW_E2E_DATADIR"));
const PASSPHRASE = env("DW_E2E_PASSPHRASE");
const WALLET = env("DW_E2E_WALLET");
const PEER = env("DW_E2E_SPV_PEER");
const RPC = new URL(env("DW_E2E_RPC"));
const MINER = env("DW_E2E_MINER");
const MASTERNODE = env("DW_E2E_MASTERNODE");
const PAYOUT = env("DW_E2E_PAYOUT");
const PORT = Number(env("DW_E2E_PORT", "4785"));
/** SPV and the masternode list's sync on a loaded host (as regtest-governance.ts allows). */
const LOAD_WAIT_MS = Number(env("DW_E2E_LOAD_WAIT_MS", "600000"));

let driver: ChildProcess | undefined;

/** dashd's JSON-RPC (fetch takes no credentials in the URL: they go in the header). */
async function rpc<T>(method: string, ...params: unknown[]): Promise<T> {
  const url = `${RPC.protocol}//${RPC.host}${RPC.pathname}`;
  const auth = Buffer.from(`${decodeURIComponent(RPC.username)}:${decodeURIComponent(RPC.password)}`).toString("base64");
  const res = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Basic ${auth}` },
    body: JSON.stringify({ jsonrpc: "1.0", id: method, method, params }),
  });
  const body = (await res.json()) as { result: T; error: { message: string } | null };
  if (body.error) throw new Error(`${method}: ${body.error.message}`);
  return body.result;
}

async function main() {
  mkdirSync(OUT, { recursive: true });
  const steps: string[] = [];
  const evidence: Record<string, unknown> = {};
  const step = (s: string) => {
    steps.push(s);
    console.log(`regtest-update-registrar: ${s}`);
  };

  const started = await startDriver(PORT, OUT);
  driver = started.driver;
  const wd = new WebDriver(started.base, OUT);
  const args = ["--datadir", DATADIR, ...CHAIN, "--quorum-url", "http://127.0.0.1:1", "--sync", "--spv-peer", PEER, "--theme", "light"];
  await wd.start({ alwaysMatch: { "tauri:options": { application: APP, args } } });
  const textOf = (id: string) => wd.script<string>(`return document.querySelector(arguments[0])?.textContent ?? ""`, [tid(id)]);
  const exists = (id: string) => wd.script<boolean>(`return !!document.querySelector(arguments[0])`, [tid(id)]);
  const enabled = (id: string) => wd.script<boolean>(`const b = document.querySelector(arguments[0]); return !!b && !b.disabled`, [tid(id)]);
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

    const row = `mn-row-${MASTERNODE}`;
    await wd.waitFor(tid(row), "F in My masternodes", LOAD_WAIT_MS);
    await wd.press(tid(row));
    // Offered once the list is synced and the wallet's records hold F's registration (EP 6).
    await poll(LOAD_WAIT_MS, "Change keys or payout… offered", async () => ((await exists("mn-change-keys")) ? true : undefined));
    evidence.pane = await textOf("mn-facts");
    await wd.screenshot("1-pane");
    step(`F's pane: "${evidence.pane}"`);

    await wd.press(tid("mn-change-keys"));
    await wd.waitFor(tid("mn-change-keys-sheet"), "the Change keys or payout sheet");
    await wd.press(tid("mn-change-voting-keychain"));
    await wd.press(tid("mn-change-payout-address"));
    await enter("mn-change-payout-input", PAYOUT);
    await poll(TIMEOUT_MS, "the review: voting and payout change", async () => {
      const voting = (await textOf("mn-change-review-voting")).trim();
      const payout = (await textOf("mn-change-review-payout")).trim();
      return voting.includes("Changes") && payout.includes("Changes") ? { voting, payout } : undefined;
    }).then((r) => (evidence.review = r));
    evidence.fee = await textOf("mn-change-review-fee");
    evidence.signer = await textOf("mn-change-signer");
    if (await exists("mn-change-pose")) throw new Error("the PoSe-ban warning shows, but the operator key is kept");
    if (!(await exists("mn-change-send"))) throw new Error("Send change is not on this build (held until the operation-release fix): nothing to drive");
    await enter("mn-change-passphrase", PASSPHRASE);
    await poll(TIMEOUT_MS, "Send change enabled", async () => ((await enabled("mn-change-send")) ? true : undefined));
    await wd.screenshot("2-review");
    step(`review: ${JSON.stringify(evidence.review)}; fee "${evidence.fee}"; ${evidence.signer}`);

    await wd.press(tid("mn-change-send"));
    const outcome = await poll(TIMEOUT_MS, "the update's outcome", async () =>
      (await exists("mn-change-keys-sent")) ? "sent" : (await exists("mn-change-unsent")) ? "not sent" : undefined,
    );
    await wd.screenshot("3-outcome");
    if (outcome !== "sent") throw new Error(`the update did not go out: ${await textOf("mn-change-unsent")}`);
    evidence.sent = await textOf("mn-change-keys-sent");
    const ops = await invoke<{ kind: string; proTxHash: string | null; txid: string | null; state: { kind: string } }[]>("mn_operations", { walletId: WALLET });
    const op = ops.find((o) => o.kind === "update_registrar" && o.proTxHash === MASTERNODE);
    if (!op?.txid) throw new Error(`mn_operations has no update of F: ${JSON.stringify(ops)}`);
    evidence.txid = op.txid;
    step(`Send change: "${evidence.sent}"; ${op.state.kind} ${op.txid}`);

    await poll(TIMEOUT_MS, "the update in dashd's mempool", async () => ((await rpc<string[]>("getrawmempool")).includes(op.txid!) ? true : undefined));
    await rpc<string[]>("generatetoaddress", 1, MINER);
    step("one block mined over it");
    await wd.press(tid("mn-change-keys-done"));
  } catch (e) {
    await wd.screenshot("failure").catch(() => undefined);
    throw e;
  } finally {
    await wd.end().catch(() => undefined);
  }
  writeFileSync(resolve(OUT, "result.json"), JSON.stringify({ ok: true, steps, evidence }, null, 2));
  console.log(`regtest-update-registrar: PASS (${steps.length} steps); evidence in ${OUT}`);
}

main()
  .catch((e) => {
    console.error(`regtest-update-registrar: FAIL: ${e instanceof Error ? e.message : String(e)}`);
    process.exitCode = 1;
  })
  .finally(() => stopDriver(driver));
