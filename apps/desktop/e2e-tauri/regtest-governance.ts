// MG-09 acceptance (ROADMAP MG-09, GL): from the UI on regtest, vote on a proposal and create one.
// tauri-driver drives the production renderer on the real engine. The chain is the regtest suite
// `governance_ui` (regtest/functional/dwd_governance_ui.py): 3 masternodes, one of them voting
// with this wallet's DIP3 voting key 0, a proposal to vote on, and this wallet holding one
// 1,000 DASH coin. That suite starts this file and is the judge: Core must hold the vote and list
// the created proposal.
//
// Flow: unlock → the sidebar shows Masternodes for the collateral-sized coin (D3) → Governance
// is off: Turn on (MG-02's setting, D6) → the list syncs and shows the suite's proposal → Vote…:
// the masternode is listed with key 0, Yes, passphrase, Vote Yes → "1 of 1 vote sent" → Create
// proposal: details, payments, review (the engine's preview), passphrase, Pay → Mine says Fee paid
// → one block: the wallet submits (Confirming) → five more: fully valid. Screenshots, the app's
// log and result.json (the evidence) go to $DW_E2E_OUT.
//
//   DW_APP=…/dash-wallet DWCLI=…/dwcli TAURI_DRIVER=…/tauri-driver DW_E2E_DATADIR=… \
//   DW_E2E_PASSPHRASE=… DW_E2E_WALLET=… DW_E2E_SPV_PEER=127.0.0.1:… DW_E2E_RPC=http://u:p@127.0.0.1:… \
//   DW_E2E_MINER=<address> DW_E2E_PROPOSAL=<name> DW_E2E_VOTER=<proTxHash> node e2e-tauri/regtest-governance.ts
// `scripts/e2e-regtest-governance.sh` builds the binaries and runs the suite. Not in CI: it needs
// dashd and a private X display.

import type { ChildProcess } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { APP, CHAIN, env, poll, startDriver, stopDriver, tid, TIMEOUT_MS, WebDriver } from "./driver.ts";

const OUT = resolve(env("DW_E2E_OUT", "test-results/tauri-regtest-governance"));
const DATADIR = resolve(env("DW_E2E_DATADIR"));
const PASSPHRASE = env("DW_E2E_PASSPHRASE");
const WALLET = env("DW_E2E_WALLET");
const PEER = env("DW_E2E_SPV_PEER");
const RPC = new URL(env("DW_E2E_RPC"));
const MINER = env("DW_E2E_MINER");
const PROPOSAL = env("DW_E2E_PROPOSAL");
const VOTER = env("DW_E2E_VOTER");
const PORT = Number(env("DW_E2E_PORT", "4784"));
/** SPV and governance sync on a loaded host (as regtest-send.ts allows). */
const LOAD_WAIT_MS = Number(env("DW_E2E_LOAD_WAIT_MS", "600000"));
/** The proposal this run creates; the suite reads it back from result.json. */
const CREATED = "dwd_ui_p1";

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
const mine = (blocks: number) => rpc<string[]>("generatetoaddress", blocks, MINER);

async function main() {
  mkdirSync(OUT, { recursive: true });
  const steps: string[] = [];
  const evidence: Record<string, unknown> = {};
  const step = (s: string) => {
    steps.push(s);
    console.log(`regtest-governance: ${s}`);
  };

  const started = await startDriver(PORT, OUT);
  driver = started.driver;
  const wd = new WebDriver(started.base, OUT);
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
  /** Chooses an option of a <select> as its change event reports it to React. */
  const choose = (id: string, value: string) =>
    wd.script(
      `const el = document.querySelector(arguments[0]);
       Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set.call(el, arguments[1]);
       el.dispatchEvent(new Event("change", { bubbles: true }));`,
      [tid(id), value],
    );
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

    // No setting: Masternodes shows for the 1,000 DASH coin once SPV has seen it (D3).
    await wd.waitFor(tid("sidebar-masternodes"), "Masternodes in the sidebar (the collateral-sized coin)", LOAD_WAIT_MS);
    await wd.press(tid("sidebar-masternodes"));
    await wd.waitFor(tid("page-masternodes"), "the Masternodes page");
    // Governance data is off for a new network: Turn on writes MG-02's setting (D6).
    const off = await poll(TIMEOUT_MS, "the governance state", async () =>
      (await exists("governance-disabled")) ? true : (await exists("governance-clock")) || (await exists("governance-syncing")) ? false : undefined,
    );
    if (off) {
      await wd.screenshot("1-governance-off");
      await wd.press(tid("governance-turn-on"));
    }
    step(`Masternodes shown for the collateral-sized coin; governance ${off ? "was off: Turn on" : "already on"}`);

    const row = `proposal-row-${PROPOSAL}`;
    await wd.waitFor(tid(row), `${PROPOSAL} in the synced list`, LOAD_WAIT_MS);
    evidence.clock = await textOf("governance-clock-title");
    evidence.listRow = await textOf(row);
    await wd.screenshot("2-list");
    step(`the list is synced: "${evidence.clock}"; ${PROPOSAL}: "${evidence.listRow}"`);

    // Vote: the masternode with this wallet's voting key, Yes.
    await wd.press(tid(row));
    await poll(LOAD_WAIT_MS, "Vote… enabled (a voting key in the wallet)", async () => ((await enabled("proposal-vote")) ? true : undefined));
    await wd.press(tid("proposal-vote"));
    const node = `vote-node-${VOTER.slice(0, 8)}`;
    await wd.waitFor(tid(node), "the masternode in the vote sheet");
    evidence.voteRow = await textOf(node);
    await wd.press(tid("vote-outcome-yes"));
    await enter("vote-passphrase", PASSPHRASE);
    await wd.screenshot("3-vote-sheet");
    await wd.press(tid("vote-send"));
    evidence.voteResult = await poll(TIMEOUT_MS, "the vote's result", textIs("vote-result-title", "1 of 1 vote sent", "0 of 1 vote sent"));
    evidence.voteResultRow = await textOf(`vote-result-${VOTER.slice(0, 8)}`);
    await wd.screenshot("4-vote-result");
    if (evidence.voteResult !== "1 of 1 vote sent") throw new Error(`the vote was not sent: ${evidence.voteResultRow}`);
    step(`Vote… Yes with ${evidence.voteRow}: "${evidence.voteResult}" (${evidence.voteResultRow})`);
    await wd.press(tid("vote-done"));
    await wd.waitGone(tid("vote-sheet"), "the vote sheet");

    // Create: three steps, then the fee.
    await wd.press(tid("governance-create"));
    await enter("create-name", CREATED);
    await enter("create-url", "https://example.org/dwd-ui-p1");
    await wd.press(tid("create-continue"));
    await wd.waitFor(tid("create-amount"), "the payments step");
    await choose("create-count", "2");
    await enter("create-amount", "2.5");
    await poll(TIMEOUT_MS, "the amount to settle", async () => ((await enabled("create-continue")) ? true : undefined));
    evidence.firstPayment = await wd.script<string>(`const s = document.querySelector(arguments[0]); return s.options[s.selectedIndex].text`, [tid("create-first")]);
    await wd.screenshot("5-create-payments");
    await wd.press(tid("create-continue"));
    await wd.waitFor(tid("create-review"), "the review (the engine's preview)");
    evidence.reviewFee = await textOf("create-review-fee");
    await enter("create-passphrase", PASSPHRASE);
    await wd.screenshot("6-create-review");
    await wd.press(tid("create-pay"));
    await wd.waitGone(tid("create-sheet"), "the create sheet");
    const chip = `mine-chip-${CREATED}`;
    await wd.waitFor(tid(chip), "the new proposal in Mine");
    evidence.afterPay = await poll(TIMEOUT_MS, "Fee paid", textIs(chip, "Fee paid"));
    await wd.screenshot("7-mine-fee-paid");
    const created = (await invoke<{ name: string; hash: string; feeTxid: string }[]>("governance_my_proposals", { walletId: WALLET })).find((p) => p.name === CREATED);
    if (!created) throw new Error("governance_my_proposals has no created proposal");
    Object.assign(evidence, { createdHash: created.hash, feeTxid: created.feeTxid });
    step(`Create proposal ${CREATED}, first payment ${evidence.firstPayment}, fee "${evidence.reviewFee}": Mine says ${evidence.afterPay}; fee ${created.feeTxid}`);

    // The fee confirms: the wallet submits it on its own (MG-06), then it gathers six confirmations.
    await poll(TIMEOUT_MS, "the fee in dashd's mempool", async () => ((await rpc<string[]>("getrawmempool")).includes(created.feeTxid) ? true : undefined));
    await mine(1);
    evidence.afterOne = await poll(LOAD_WAIT_MS, "the wallet to submit it", textIs(chip, "Confirming"));
    await wd.screenshot("8-mine-confirming");
    await mine(5);
    evidence.afterSix = await poll(LOAD_WAIT_MS, "six confirmations", async () => {
      const meta = (await textOf(`mine-row-${CREATED}`)).trim();
      const c = (await textOf(chip)).trim();
      return c !== "Confirming" || meta.includes("6 of 6") ? `${c}: ${meta}` : undefined;
    });
    await wd.screenshot("9-mine-six");
    step(`one block: ${evidence.afterOne}; six: ${evidence.afterSix}`);
    // Core relays it once valid; the list's next update makes it public in Mine. Recorded, not
    // required: the suite judges the proposal on Core.
    evidence.public = await poll(180_000, "the proposal public in Mine", async () => {
      const c = (await textOf(chip)).trim();
      return c !== "Confirming" ? c : undefined;
    }).catch(() => null);
    await wd.screenshot("10-mine-later");
    step(`later: Mine says ${evidence.public ?? "Confirming (not public within 3 min)"}`);
  } catch (e) {
    await wd.screenshot("failure").catch(() => undefined);
    throw e;
  } finally {
    await wd.end().catch(() => undefined);
  }
  writeFileSync(resolve(OUT, "result.json"), JSON.stringify({ ok: true, created: CREATED, steps, evidence }, null, 2));
  console.log(`regtest-governance: PASS (${steps.length} steps); evidence in ${OUT}`);
}

main()
  .catch((e) => {
    console.error(`regtest-governance: FAIL: ${e instanceof Error ? e.message : String(e)}`);
    process.exitCode = 1;
  })
  .finally(() => stopDriver(driver));
