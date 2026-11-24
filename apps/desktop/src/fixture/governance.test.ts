// The fixture's governance (governance.ts, MG-09) through the generated commands, as the Governance
// tab calls them: the sync switch and its refusals, votes with the one-hour rule and a store
// refusal, and own proposals from create to submit in MG-06's states.

import { clearMocks } from "@tauri-apps/api/mocks";
import { listen } from "@tauri-apps/api/event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { AppEvent } from "../api/generated/commands";
import type { ProposalDraft } from "../api/generated/ProposalDraft";
import { FEE_NETWORK, MN_A, MN_B, MN_C, PROPOSAL_FEE } from "./governance";
import { installFixture } from "./index";
import { CLUB, KEY_VOTING } from "./masternodes";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID as W, type ScenarioName } from "./scenarios";
import { FIXTURE_NOW } from "./wallet";

const PASS = { kind: "passphrase", passphrase: FIXTURE_PASSPHRASE } as const;
const COIN = 100_000_000;

const install = (scenario: ScenarioName) => installFixture({ scenario });
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  install("governance");
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

async function events() {
  const seen: AppEvent[] = [];
  await listen<AppEvent>(cmd.ENGINE_EVENT, (e) => void seen.push(e.payload));
  return seen;
}

const hashOf = async (name: string) => (await cmd.governanceProposals({ scope: "all", text: name }))[0].hash;

async function draft(): Promise<ProposalDraft> {
  const [sb] = await cmd.governanceUpcomingSuperblocks(12);
  return {
    name: "desktop-wallet-docs",
    url: "https://www.dashcentral.org/p/desktop-wallet-docs",
    paymentAddress: "yPay8vK2pMxRt5nW9cJ3hL6dF4sA1bZ7Wm",
    paymentAmount: 50 * COIN,
    paymentCount: 2,
    firstSuperblockHeight: sb.height,
    coins: [],
  };
}

describe("fixture governance", () => {
  it("is off by default, refuses reads before a sync, and turns on with an event", async () => {
    install("funded");
    const seen = await events();
    expect(await cmd.governanceSyncEnabled()).toBe(false);
    expect((await cmd.governanceSyncState()).phase).toBe("disabled");
    await expect(cmd.governanceProposals({ scope: "current", text: null })).rejects.toMatchObject({
      code: "governance.not_synced",
      data: { kind: "governancePhase", phase: "disabled" },
    });
    // The clock needs only the tip; the vote-derived counts wait for a sync (mg-engine §1).
    const info = await cmd.governanceInfo();
    expect(info.clock.nextSuperblock).toBeGreaterThan(info.clock.tipHeight);
    expect(info.proposals).toBeNull();
    await cmd.governanceSetSyncEnabled(true);
    expect(seen.filter((e) => e.kind === "governance")).toHaveLength(1);
    expect((await cmd.governanceInfo()).proposals).toMatchObject({ total: 5 });
    expect(await cmd.governanceProposals({ scope: "current", text: null })).toHaveLength(5);
    expect(await cmd.governanceProposals({ scope: "all", text: null })).toHaveLength(6);
  });

  it("lists the wallet's voting masternodes with the votes seen", async () => {
    const hash = await hashOf("dash-incubator-nov");
    const nodes = await cmd.governanceVotingMasternodes(W, hash);
    expect(nodes.map((n) => [n.proTxHash, n.valid, n.currentVote])).toEqual([
      [MN_A, true, "yes"],
      [MN_B, true, null],
      [MN_C, false, null],
    ]);
    await expect(cmd.governanceVotingMasternodes(W, "zz")).rejects.toMatchObject({ code: "invalid_argument" });
  });

  it("votes under a governance grant, refusing per masternode as the engine does", async () => {
    const hash = await hashOf("dash-incubator-nov");
    const before = (await cmd.governanceProposal(hash)).row.tally.no.estimate;
    // A spend grant is the wrong purpose.
    const spend = await cmd.authorize({ kind: "spend", maxDuffs: 1 }, W, PASS);
    await expect(cmd.governanceVote(W, hash, "no", [MN_B], spend.id)).rejects.toMatchObject({ code: "platform.grant_invalid" });
    const grant = await cmd.authorize({ kind: "governance" }, W, PASS);
    const casts = await cmd.governanceVote(W, hash, "no", [MN_A, MN_B, MN_C], grant.id);
    expect(casts.map((c) => c.rejected)).toEqual(["too_often", null, "masternode_not_found"]);
    expect((await cmd.governanceProposal(hash)).row.tally.no.estimate).toBe(before + 4);
    // Single use.
    await expect(cmd.governanceVote(W, hash, "no", [MN_B], grant.id)).rejects.toMatchObject({ code: "platform.grant_invalid" });
    // The evonode voted just now: a change waits an hour.
    const again = await cmd.authorize({ kind: "governance" }, W, PASS);
    expect((await cmd.governanceVote(W, hash, "yes", [MN_B], again.id))[0].rejected).toBe("too_often");
  });

  it("offers a tracked masternode once its added voting key matches, after the keychain's, and votes with it", async () => {
    const hash = await hashOf("dashcon-2027-venue");
    const offered = async () => (await cmd.governanceVotingMasternodes(W, hash)).map((n) => [n.proTxHash, n.key, n.weight]);
    expect(await offered()).toEqual([
      [MN_A, { kind: "keychain", index: 0 }, 1],
      [MN_B, { kind: "keychain", index: 1 }, 4],
      [MN_C, { kind: "keychain", index: 2 }, 1],
    ]);
    expect(await cmd.mnAttachKey(CLUB, "voting", KEY_VOTING)).toBe("matched");
    expect((await offered()).at(-1)).toEqual([CLUB, { kind: "attached" }, 4]);
    const grant = await cmd.authorize({ kind: "governance" }, W, PASS);
    expect((await cmd.governanceVote(W, hash, "yes", [CLUB], grant.id))[0].rejected).toBeNull();
    expect((await cmd.governanceVotingMasternodes(W, hash)).find((n) => n.proTxHash === CLUB)?.currentVote).toBe("yes");
    expect(await cmd.mnDetachKey(CLUB, "voting")).toBe(true);
    expect((await offered()).map(([h]) => h)).toEqual([MN_A, MN_B, MN_C]);
  });

  it("checks an added key at vote time: an unresolved collateral is listed but refused, a rotated-away key isn't offered (Astra r1 4, 7)", async () => {
    const f = install("governance-added-key");
    const hash = await hashOf("dashcon-2027-venue");
    const vote = async () => (await cmd.governanceVote(W, hash, "yes", [CLUB], (await cmd.authorize({ kind: "governance" }, W, PASS)).id))[0].rejected;
    const club = async () => (await cmd.governanceVotingMasternodes(W, hash)).find((n) => n.proTxHash === CLUB);
    f.world.governance.collateralUnresolved.push(CLUB);
    expect((await club())?.collateralResolved).toBe(false);
    expect(await vote()).toBe("collateral_unresolved");
    f.world.governance.collateralUnresolved = [];
    // The masternode's owner changed its voting key: the key added here no longer matches the list.
    f.world.masternodes.expected[CLUB] = { ...f.world.masternodes.expected[CLUB], voting: "rotated" };
    expect(await club()).toBeUndefined();
    expect(await vote()).toBe("no_voting_key");
    expect((await cmd.mnList(W)).masternodes.find((r) => r.proTxHash === CLUB)?.roles).toMatchObject({ voting: false, attached: [{ role: "voting", check: "key_mismatch" }] });
  });

  it("lets a keychain voter vote whatever stale key is attached to it, as choose_key tries the keychain first", async () => {
    const f = install("governance");
    const club = f.world.masternodes.rows.find((r) => r.proTxHash === CLUB)!;
    f.world.masternodes.rows.push({ ...structuredClone(club), proTxHash: MN_B, tracked: true });
    f.world.masternodes.attached[MN_B] = [{ role: "voting", keyId: "stale", check: "matched" }];
    const hash = await hashOf("dashcon-2027-venue");
    const grant = await cmd.authorize({ kind: "governance" }, W, PASS);
    expect((await cmd.governanceVote(W, hash, "yes", [MN_B], grant.id))[0].rejected).toBeNull();
  });

  it("refuses a vote whose voting key changed (governance-vote-refused)", async () => {
    install("governance-vote-refused");
    const hash = await hashOf("dashcon-2027-venue");
    const grant = await cmd.authorize({ kind: "governance" }, W, PASS);
    const casts = await cmd.governanceVote(W, hash, "yes", [MN_A, MN_B], grant.id);
    expect(casts.map((c) => c.rejected)).toEqual([null, "wrong_voting_key"]);
  });

  it("previews and creates a proposal under a spend grant capped at the fee", async () => {
    const d = await draft();
    const preview = await cmd.governancePreviewProposal(W, d);
    expect(preview).toMatchObject({ fee: PROPOSAL_FEE, networkFee: FEE_NETWORK });
    expect(JSON.parse(preview.dataJson)).toMatchObject({ name: d.name, payment_amount: 50, type: 1 });
    await expect(cmd.governancePreviewProposal(W, { ...d, name: "Bad Name!", paymentCount: 0 })).rejects.toMatchObject({
      code: "governance.invalid_proposal",
      data: { kind: "proposalFields", fields: ["name", "payment_count"] },
    });
    const short = await cmd.authorize({ kind: "spend", maxDuffs: PROPOSAL_FEE }, W, PASS);
    await expect(cmd.governanceCreateProposal(W, d, short.id)).rejects.toMatchObject({ code: "send.grant_exceeded" });
    const grant = await cmd.authorize({ kind: "spend", maxDuffs: preview.fee + preview.networkFee }, W, PASS);
    const created = await cmd.governanceCreateProposal(W, d, grant.id);
    expect(created).toMatchObject({ hash: preview.hash, state: { kind: "feePaid", confirmations: 0 } });
    // Newest first, before the scenario's own confirming one.
    expect((await cmd.governanceMyProposals(W)).map((p) => p.name)).toEqual([d.name, "dash-docs-refresh"]);
  });

  it("refuses a draft named as a proposal in progress, as the engine does (issue #148)", async () => {
    const d = await draft();
    const pay = async () => (await cmd.authorize({ kind: "spend", maxDuffs: PROPOSAL_FEE + FEE_NETWORK }, W, PASS)).id;
    const created = await cmd.governanceCreateProposal(W, d, await pay());
    await expect(cmd.governanceCreateProposal(W, { ...d, url: `${d.url}-again` }, await pay())).rejects.toMatchObject({
      code: "governance.proposal_in_progress",
      data: { kind: "proposalHash", hash: created.hash },
    });
    expect((await cmd.governanceMyProposals(W)).filter((p) => p.name === d.name)).toHaveLength(1);
    await expect(cmd.governanceCreateProposal(W, { ...d, name: d.name.toUpperCase() }, await pay())).rejects.toMatchObject({
      code: "governance.proposal_in_progress",
      data: { kind: "proposalHash", hash: created.hash },
    });
    expect((await cmd.governanceMyProposals(W)).filter((p) => p.name.toLowerCase() === d.name)).toHaveLength(1);
    const f = install("governance");
    f.world.governance.creating = [d.name];
    await expect(cmd.governanceCreateProposal(W, d, await pay())).rejects.toMatchObject({ code: "governance.proposal_in_progress", data: null });
    expect((await cmd.governanceMyProposals(W)).some((p) => p.name === d.name)).toBe(false);
    await expect(cmd.governanceCreateProposal(W, { ...d, name: "desktop-wallet-docs-2" }, await pay())).resolves.toMatchObject({ name: "desktop-wallet-docs-2" });
  });

  it("frees the name of a public proposal past its end, as the engine does (issue #188)", async () => {
    const f = install("governance");
    const d = await draft();
    const pay = async () => (await cmd.authorize({ kind: "spend", maxDuffs: PROPOSAL_FEE + FEE_NETWORK }, W, PASS)).id;
    const created = await cmd.governanceCreateProposal(W, d, await pay());
    const row = f.world.governance.mine[W].find((p) => p.hash === created.hash)!;
    row.state = { kind: "public", status: "passing" };
    // Another schedule, so the fixture's same-draft check is not what refuses or admits it.
    const again = { ...d, paymentCount: 3 };
    await expect(cmd.governanceCreateProposal(W, again, await pay())).rejects.toMatchObject({
      code: "governance.proposal_in_progress",
      data: { kind: "proposalHash", hash: created.hash },
    });
    row.endEpoch = Math.floor(FIXTURE_NOW.getTime() / 1000);
    await expect(cmd.governanceCreateProposal(W, again, await pay())).resolves.toMatchObject({ name: d.name });
    expect((await cmd.governanceMyProposals(W)).filter((p) => p.name === d.name)).toHaveLength(2);
  });

  it("submits per MG-06 D24 and refuses past submitting", async () => {
    install("governance-mine");
    const mine = await cmd.governanceMyProposals(W);
    const byName = (n: string) => mine.find((p) => p.name === n)!.hash;
    expect((await cmd.governanceSubmitProposal(W, byName("wallet-audit-2027"))).state).toEqual({ kind: "feePaid", confirmations: 0 });
    expect((await cmd.governanceSubmitProposal(W, byName("community-meetups"))).state).toEqual({ kind: "confirming", confirmations: 1 });
    await expect(cmd.governanceSubmitProposal(W, byName("podcast-season-3"))).rejects.toMatchObject({
      code: "governance.submit_refused",
      data: { kind: "reason", reason: "fee_failed" },
    });
    await expect(cmd.governanceSubmitProposal(W, byName("dash-incubator-nov"))).rejects.toMatchObject({ data: { reason: "public" } });
  });

  it("keeps a failed resend as fee_unsent with its error (governance-submit-fails)", async () => {
    install("governance-submit-fails");
    const hash = (await cmd.governanceMyProposals(W)).find((p) => p.name === "desktop-wallet-i18n")!.hash;
    await expect(cmd.governanceSubmitProposal(W, hash)).rejects.toMatchObject({ code: "send.broadcast_unknown" });
    const after = (await cmd.governanceMyProposals(W)).find((p) => p.hash === hash)!;
    expect(after.state).toEqual({ kind: "feeUnsent" });
    expect(after.lastError).toMatch(/^send: broadcast outcome unknown/);
  });
});
