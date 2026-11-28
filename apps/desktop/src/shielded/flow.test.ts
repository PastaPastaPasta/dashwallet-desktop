// The shielded flow: each move runs under the quote's platformOp grant, the sheet can close while the
// call runs, a lock that was not applied is finished with one duff of grant, and the refusals have
// their words.

import { afterEach, describe, expect, it, vi } from "vitest";
import type { ShieldedSpendOutcome } from "../api/generated/ShieldedSpendOutcome";
import type { ShieldOutcome } from "../api/generated/ShieldOutcome";
import type { FundAmount } from "../api/generated/FundAmount";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { fakeVault } from "../tools/testApi";
import { onShieldedSettled, resetShielded, SHIELDED_COPY, shieldedCanChange, shieldedErrorText, shieldedFor, ShieldedFlow } from "./flow";

const duffsOf = (a: FundAmount) => (a.kind === "duffs" ? a.duffs : 0);

const TO = "tdash1zq0k8y";
const SPEND: ShieldedSpendOutcome = { credits: "10000000000", balance: "114980000000", to: TO, state: "done", unresolved: null, pendingReason: null };
const APPLIED: ShieldOutcome = { txid: "lock1", state: { kind: "applied", balance: "130000000000" } };
const GRANT = { maxDuffs: 0, maxCredits: "10020000000" };
const credits = (c: string) => `${Number(BigInt(c) / 1000n) / 1e8} tDASH`;
const duffs = (d: number) => `${d / 1e8} tDASH`;

function setup(lock = { encrypted: false }) {
  const vault = fakeVault(lock);
  const calls: string[] = [];
  let answer: ((o: ShieldedSpendOutcome) => void) | null = null;
  const api = {
    ...vault.api,
    shieldFromCore: vi.fn(async (_w: string, a: FundAmount, grant: string) => (calls.push(`add ${duffsOf(a)} ${grant}`), APPLIED)),
    resumeShield: vi.fn(async (_w: string, txid: string, grant: string) => (calls.push(`resume ${txid} ${grant}`), APPLIED)),
    shieldedSend: vi.fn(
      (_w: string, to: string, c: string, memo: string | null, grant: string) =>
        new Promise<ShieldedSpendOutcome>((resolve) => {
          calls.push(`send ${to} ${c} ${memo} ${grant}`);
          answer = resolve;
        }),
    ),
    unshieldToCore: vi.fn(async (_w: string, c: string, to: string | null, grant: string, _confirm: boolean) => (calls.push(`out ${c} ${to} ${grant}`), SPEND)),
    unshieldToPlatform: vi.fn(async (_w: string, c: string, to: string | null, grant: string, confirm: boolean) => (calls.push(`toPlatform ${c} ${to} ${grant} ${confirm}`), SPEND)),
    platformAddressesSync: vi.fn(async (_w: string) => (calls.push("platformSync"), { credits: "0", movableCredits: "0", duffs: 0, addresses: [], syncedAt: null, syncing: false, sendsActive: true, sendsOff: null })),
  };
  const flow = new ShieldedFlow(api, WALLET_A);
  return { flow, api, vault, calls, answer: (o: ShieldedSpendOutcome) => answer!(o) };
}

afterEach(() => resetShielded());

describe("the shielded flow", () => {
  it("says it is preparing once a shield's call has been pending for a second, and only for a shield", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const { flow, api, answer } = setup();
      let shielded: ((o: ShieldOutcome) => void) | null = null;
      api.shieldFromCore.mockImplementation(() => new Promise<ShieldOutcome>((resolve) => (shielded = resolve)));
      flow.begin("add");
      flow.next({ kind: "add", duffs: 100_000 });
      const going = flow.run({ maxDuffs: 101_000, maxCredits: "0" });
      await vi.advanceTimersByTimeAsync(0);
      expect(flow.step).toBe("running");
      await vi.advanceTimersByTimeAsync(999);
      expect(flow.preparing.shown).toBe(false);
      await vi.advanceTimersByTimeAsync(1);
      expect(flow.preparing.shown).toBe(true);
      shielded!(APPLIED);
      await going;
      expect([flow.step, flow.preparing.shown]).toEqual(["done", false]);

      flow.begin("send");
      flow.next({ kind: "send", credits: "10000000000", to: TO, memo: null });
      const sending = flow.run(GRANT);
      await vi.advanceTimersByTimeAsync(5_000);
      expect([flow.step, flow.preparing.shown]).toEqual(["running", false]);
      answer(SPEND);
      await sending;
    } finally {
      vi.useRealTimers();
    }
  });

  it("adds to Shielded under a duffs grant and reads again", async () => {
    const { flow, vault, calls } = setup();
    const settled = vi.fn();
    const stop = onShieldedSettled(settled);
    flow.begin("add");
    flow.next({ kind: "add", duffs: 100_000 });
    await flow.run({ maxDuffs: 101_000, maxCredits: "0" });
    expect(vault.calls.authorize[0]).toMatchObject({ purpose: { kind: "platformOp", maxDuffs: 101_000, maxCredits: "0" }, walletId: WALLET_A });
    expect(calls).toEqual(["add 100000 grant-1"]);
    expect([flow.step, flow.result]).toEqual(["done", { kind: "shield", outcome: APPLIED }]);
    await Promise.resolve();
    expect(settled).toHaveBeenCalledTimes(1);
    stop();
  });

  it("a Core Max (Max from the Dash wallet) is sent with the confirmed coin set, never as an amount with the fee on top", async () => {
    const { flow, api } = setup();
    flow.begin("add");
    flow.next({ kind: "add", duffs: 199_998_956, all: true });
    await flow.run({ maxDuffs: 200_000_000, maxCredits: "0" }, false, "confirmed-set");
    expect(api.shieldFromCore.mock.calls).toEqual([[WALLET_A, { kind: "quotedAll", coinSet: "confirmed-set" }, "grant-1"]]);
  });

  it("a Core Max's refusals say what Max needs; an amount's keep their own words", () => {
    const text = (e: unknown, max: boolean) => shieldedErrorText(e, credits, duffs, "add", false, max);
    const locked = appError("shielded.coins_locked");
    expect(text(locked, true)).toBe("Max can't run: some coins in your Dash wallet are locked. Enter an amount instead.");
    expect(text(locked, false)).not.toContain("Max");
    expect(text(appError("shielded.too_many_inputs", { kind: "number", value: 61 }), true)).toBe("Max needs at most 50 coins; this wallet has 61. Enter an amount instead.");
    expect(text(appError("shielded.mixing_active"), true)).toBe("Max can't run while mixing. Stop mixing first, or enter an amount.");
    expect(text(appError("shielded.move_running"), true)).toBe("Another move of your coins is running. Try Max again when it's done.");
    expect(text(appError("shielded.coins_changed"), true)).toBe("Your Dash wallet changed since this was worked out. Review the amount again.");
    expect(shieldedCanChange(appError("shielded.coins_changed"))).toBe(true);
  });

  it("a Core Max below the smallest amount says what Max can move, and what it leaves out only when something is", () => {
    const text = (e: unknown, max: boolean) => shieldedErrorText(e, credits, duffs, "add", false, max);
    const below = (leftDuffs: number) => appError("shielded.below_minimum", { kind: "maxMinimum", min: 10_001, duffs: 3_852, leftDuffs, arrivingDuffs: 0, arrivingClears: false, arrivingBlock: null, otherArrivingDuffs: 0 });
    expect(text(below(10_000_100), true)).toBe("Max can move only 0.00003852 tDASH, which is less than the smallest amount, 0.00010001 tDASH. Max doesn't include mixed coins, payments from contacts or coins in other accounts.");
    expect(text(below(0), true)).toBe("Max can move only 0.00003852 tDASH, which is less than the smallest amount, 0.00010001 tDASH.");
    const typed = appError("shielded.below_minimum", { kind: "number", value: 10_001 });
    expect(text(typed, false)).toBe("Too small to add: the smallest amount is 0.00010001 tDASH.");
    expect(text(typed, true)).toBe(text(typed, false));
    expect(shieldedCanChange(below(0))).toBe(true);
  });

  it("distinguishes held, too many and other-account arriving coins", () => {
    const refusal = (arrivingDuffs: number, arrivingBlock: "held" | "inputCap" | null, otherArrivingDuffs = 0) =>
      appError("shielded.below_minimum", { kind: "maxMinimum", min: 56_001, duffs: 0, leftDuffs: 0, arrivingDuffs, arrivingClears: false, arrivingBlock, otherArrivingDuffs });
    const text = (e: unknown) => shieldedErrorText(e, credits, duffs, "add", false, true);
    expect(text(refusal(200_000, "held"))).toBe("Max can't run: some coins in your Dash wallet are locked. Enter an amount instead.");
    expect(text(refusal(200_000, "inputCap"))).toBe("Your Dash wallet's coins are still arriving (0.002 tDASH), but Max can't move them as they are, even once they've confirmed.");
    expect(text(refusal(0, null, 200_000))).toBe("Your Dash wallet's coins are still arriving (0.002 tDASH), but Max can't move them as they are, even once they've confirmed.");
  });

  it("a Core Max of nothing says the coins are arriving (and whether Max can move them once they've confirmed), that Max leaves them all out, or that the wallet has nothing to move, never 0.00000000 or the minimum", () => {
    const text = (e: unknown, max: boolean) => shieldedErrorText(e, credits, duffs, "add", false, max);
    const zero = (leftDuffs: number, arrivingDuffs: number, arrivingClears = arrivingDuffs > 0) => appError("shielded.below_minimum", { kind: "maxMinimum", min: 10_001, duffs: 0, leftDuffs, arrivingDuffs, arrivingClears, arrivingBlock: null, otherArrivingDuffs: 0 });
    expect(text(zero(7_000, 200_000), true)).toBe("Your Dash wallet's coins are still arriving (0.002 tDASH). Max can move them once they've confirmed.");
    expect(text(zero(7_000, 5_000, false), true)).toBe("Your Dash wallet's coins are still arriving (0.00005 tDASH), but even once they've confirmed, that's less than the smallest amount this transfer can move.");
    expect(text(zero(200_000, 0), true)).toBe("Max has nothing to move: it doesn't include mixed coins, payments from contacts or coins in other accounts.");
    expect(text(zero(0, 0), true)).toBe("There's nothing in your Dash wallet to move.");
    for (const e of [zero(7_000, 200_000), zero(7_000, 5_000, false), zero(200_000, 0), zero(0, 0)]) expect(text(e, true)).not.toMatch(/0\.00000000|Too small/);
  });

  it("sends with its memo and moves out to the wallet's own address (null)", async () => {
    const { flow, calls, answer } = setup();
    flow.begin("send");
    flow.next({ kind: "send", credits: "10000000000", to: TO, memo: "Lunch" });
    const going = flow.run(GRANT);
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    // Closed while it proves, one call at a time, reopened to its result.
    flow.close();
    await flow.run(GRANT);
    expect(calls).toEqual([`send ${TO} 10000000000 Lunch grant-1`]);
    expect(flow.unseen).toBe(false);
    answer(SPEND);
    await going;
    expect([flow.unseen, flow.result]).toEqual([true, { kind: "spend", outcome: SPEND }]);
    flow.begin("send");
    expect(flow.step).toBe("done");
    flow.close();
    flow.begin("out");
    flow.next({ kind: "out", credits: "10000000000" });
    await flow.run(GRANT);
    expect(calls.at(-1)).toBe("out 10000000000 null grant-2");
  });

  it("moves to the wallet's own Platform receive address (null), passing the user's confirmation", async () => {
    const { flow, calls } = setup();
    flow.begin("toPlatform");
    flow.next({ kind: "toPlatform", credits: "10000000000" });
    await flow.run(GRANT);
    expect(calls).toEqual(["toPlatform 10000000000 null grant-1 false", "platformSync"]);
    expect([flow.step, flow.result]).toEqual(["done", { kind: "spend", outcome: SPEND }]);
    flow.close();
    flow.begin("toPlatform");
    flow.next({ kind: "toPlatform", credits: "10000000000" });
    await flow.run(GRANT, true);
    expect(calls.slice(-2)).toEqual(["toPlatform 10000000000 null grant-2 true", "platformSync"]);
  });

  it("reads the Platform addresses once a move to Platform is done, so Home's total follows it", async () => {
    const { flow, api, calls } = setup();
    const settled = vi.fn();
    const stop = onShieldedSettled(settled);
    flow.begin("toPlatform");
    flow.next({ kind: "toPlatform", credits: "10000000000" });
    await flow.run(GRANT);
    await vi.waitFor(() => expect(settled).toHaveBeenCalledTimes(2));
    expect(api.platformAddressesSync).toHaveBeenCalledWith(WALLET_A);
    // A pending move, or a move out to the Dash wallet, reads nothing more.
    api.unshieldToPlatform.mockResolvedValueOnce({ ...SPEND, state: "pending" });
    flow.close();
    flow.begin("toPlatform");
    flow.next({ kind: "toPlatform", credits: "10000000000" });
    await flow.run(GRANT);
    flow.close();
    flow.begin("out");
    flow.next({ kind: "out", credits: "10000000000" });
    await flow.run(GRANT);
    expect(calls.filter((c) => c === "platformSync")).toHaveLength(1);
    // A failed read leaves the done move as it was.
    api.platformAddressesSync.mockRejectedValueOnce(appError("platform.unavailable", null as never));
    flow.close();
    flow.begin("toPlatform");
    flow.next({ kind: "toPlatform", credits: "10000000000" });
    await flow.run(GRANT);
    await Promise.resolve();
    expect([flow.step, flow.result]).toEqual(["done", { kind: "spend", outcome: SPEND }]);
    stop();
  });

  it("finishes a lock that was not applied with one duff of grant and no confirm step", async () => {
    const { flow, vault, calls } = setup();
    await flow.resume("lock1");
    expect(vault.calls.authorize[0]).toMatchObject({ purpose: { kind: "platformOp", maxDuffs: 1, maxCredits: "0" } });
    expect(calls).toEqual(["resume lock1 grant-1"]);
    expect([flow.kind, flow.step]).toEqual(["resume", "done"]);
  });

  it("a cancelled passphrase ran nothing: back at the confirm step", async () => {
    const { flow, api } = setup({ encrypted: true });
    const settled = vi.fn();
    const stop = onShieldedSettled(settled);
    flow.begin("out");
    flow.next({ kind: "out", credits: "10000000000" });
    const going = flow.run(GRANT);
    await vi.waitFor(() => expect(flow.grants.pending).not.toBeNull());
    flow.grants.cancel();
    await going;
    await Promise.resolve();
    expect([flow.step, api.unshieldToCore.mock.calls.length, settled.mock.calls.length]).toEqual(["confirm", 0, 0]);
    stop();
  });

  it("a refusal keeps its error; Back when another amount or address may pass", async () => {
    const { flow, api } = setup();
    api.unshieldToCore.mockRejectedValueOnce(appError("shielded.insufficient_funds" as never));
    flow.begin("out");
    flow.next({ kind: "out", credits: "10000000000" });
    await flow.run(GRANT);
    expect([flow.step, shieldedCanChange(flow.error)]).toEqual(["refused", true]);
    flow.back();
    expect([flow.step, flow.error]).toEqual(["form", null]);
    flow.next({ kind: "out", credits: "10000000000" });
    api.unshieldToCore.mockRejectedValueOnce(appError("platform.broadcast_unknown", { kind: "artifact", artifact: "x" }));
    await flow.run(GRANT);
    expect([flow.step, shieldedCanChange(flow.error)]).toEqual(["refused", false]);
  });

  it("a cancelled Finish passphrase closes the sheet instead of leaving an empty form", async () => {
    const { flow, api } = setup({ encrypted: true });
    const going = flow.resume("lock1");
    await vi.waitFor(() => expect(flow.grants.pending).not.toBeNull());
    flow.grants.cancel();
    await going;
    expect([flow.open, api.resumeShield.mock.calls.length]).toEqual([false, 0]);
  });

  it("Finish starts clean over an unseen result", async () => {
    const { flow, answer } = setup();
    flow.begin("send");
    flow.next({ kind: "send", credits: "10000000000", to: TO, memo: null });
    const going = flow.run(GRANT);
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    flow.close();
    answer(SPEND);
    await going;
    expect([flow.unseen, flow.step]).toEqual([true, "done"]);
    await flow.resume("lock1");
    expect([flow.kind, flow.step, flow.error]).toEqual(["resume", "done", null]);
  });

  it("Pay's Shielded address opens a fresh send filled with it, and a call still going shows instead", async () => {
    const { flow, answer } = setup();
    flow.begin("out");
    flow.setText("3");
    flow.sendTo(TO, "0.2");
    expect([flow.open, flow.kind, flow.step, flow.to, flow.text, flow.memo]).toEqual([true, "send", "form", TO, "0.2", ""]);
    flow.next({ kind: "send", credits: "20000000000", to: TO, memo: null });
    const going = flow.run(GRANT);
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    flow.close();
    flow.sendTo("tdash1zother", "1");
    expect([flow.open, flow.step, flow.to, flow.text]).toEqual([true, "running", TO, "0.2"]);
    answer(SPEND);
    await going;
    flow.close();
    flow.sendTo("tdash1zother", "1");
    expect([flow.step, flow.to, flow.text]).toEqual(["form", "tdash1zother", "1"]);
  });

  it("keeps one flow per wallet while the app runs", () => {
    const { api } = setup();
    expect(shieldedFor(api, WALLET_A)).toBe(shieldedFor(api, WALLET_A));
    expect(shieldedFor(api, "other")).not.toBe(shieldedFor(api, WALLET_A));
  });
});

describe("shielded refusal copy", () => {
  const text = (code: string, data: unknown = null) => shieldedErrorText(appError(code as never, data as never), credits, duffs);
  it("gives not_active and pool_too_small their own words", () => {
    expect(text("rescan_interrupted")).toBe("The last rescan was interrupted. Finish it in Repair ▸ Rescan before sending.");
    expect(text("shielded.not_active", { kind: "protocol", protocolVersion: 11, activationVersion: 12 })).toBe("Shielded is not active on this network yet.");
    expect(text("shielded.pool_too_small", { kind: "poolNotes", notes: 120, required: 250 })).toBe(
      "Shielded withdrawals open once the network's shielded pool has enough activity. Try again later.",
    );
  });

  it("says what is missing", () => {
    expect(text("shielded.insufficient_funds", { kind: "credits", needed: "2000000", available: "1000000" })).toContain("0.00002 tDASH needed with the fee, 0.00001 tDASH available");
    expect(text("shielded.funding_insufficient", { kind: "duffs", needed: 200_000, available: 100_000 })).toContain("0.002 tDASH needed with the fee, 0.001 tDASH available");
    const minimum = (kind: "add" | "send", value: number) => shieldedErrorText(appError("shielded.below_minimum" as never, { kind: "number", value } as never), credits, duffs, kind);
    // In duffs for a shield, in credits for a spend.
    expect(minimum("add", 20_000)).toBe("Too small to add: the smallest amount is 0.0002 tDASH.");
    expect(minimum("send", 2_000_000_000)).toBe("Too small: the smallest amount is 0.02 tDASH.");
    expect(text("shielded.invalid_address", { kind: "reason", reason: "x" })).toBe("Enter a Shielded address of this network.");
    expect(text("shielded.shield_pending", { kind: "txid", txid: "t" })).toContain("Finish it");
    expect(text("platform.unavailable")).toBe("Dash Platform did not answer. Try again.");
  });

  it("says why Shielded stays closed, by the engine's reason", () => {
    expect(text("shielded.store_closed", { kind: "reason", reason: "in_use" })).toMatch(/^Another program, possibly another copy of Dash Wallet, is using your Shielded data/);
    expect(text("shielded.store_closed", { kind: "reason", reason: "damaged" })).toMatch(/^Dash Wallet couldn't read your Shielded data from before the upgrade/);
    expect(text("shielded.store_closed", { kind: "reason", reason: "no_upgrade_copy" })).toMatch(/^Dash Wallet has no backup of your Shielded data/);
    const failed = text("shielded.store_closed", { kind: "reason", reason: "failed" });
    expect(failed).toMatch(/^Dash Wallet couldn't move your Shielded data to this version, so it didn't open Shielded\. /);
    expect(text("shielded.store_closed", { kind: "reason", reason: "later" })).toBe(failed);
    expect(text("shielded.store_closed")).toBe(failed);
  });

  it("sends an add to Shielded that may be out to Tools ▸ Repair, and a spend to Activity", () => {
    const unknown = appError("platform.broadcast_unknown", { kind: "artifact", artifact: "shield/0/funding" });
    expect(shieldedErrorText(unknown, credits, duffs, "add")).toBe(SHIELDED_COPY.fundingUnknown);
    expect(SHIELDED_COPY.fundingUnknown).toContain("Open Tools ▸ Repair");
    for (const kind of ["send", "out"] as const) expect(shieldedErrorText(unknown, credits, duffs, kind)).toBe("It may have been sent. Check Activity before trying again.");
  });

  it("never invites a second move after an unshield to Platform that may be out", async () => {
    const { flow, api } = setup();
    api.unshieldToPlatform.mockRejectedValueOnce(appError("platform.broadcast_unknown", { kind: "artifact", artifact: "shielded unshield" }));
    flow.begin("toPlatform");
    flow.next({ kind: "toPlatform", credits: "10000000000" });
    await flow.run(GRANT);
    expect([flow.step, shieldedCanChange(flow.error)]).toEqual(["refused", false]);
    const words = shieldedErrorText(flow.error, credits, duffs, flow.kind);
    expect(words).toBe("This move may still go through. Until Dash Wallet knows, these Shielded funds stay held. Don't move them again until Activity shows this move as done or failed.");
    expect(words).not.toMatch(/try(ing)? again/i);
  });

  it("DEC1058_syncingNeverReadsAsPlatformDown", () => {
    const syncing = (code: string, on: boolean) => shieldedErrorText(appError(code as never, null as never), credits, duffs, "add", on);
    const SAID = "Shielded is still syncing. Try again when it finishes.";
    expect(syncing("shielded.not_synced", false)).toBe(SAID);
    for (const code of ["platform.unavailable", "platform.timeout", "platform.context_unavailable"]) expect(syncing(code, true)).toBe(SAID);
    // Not syncing: a real Platform outage keeps its own words.
    expect(syncing("platform.unavailable", false)).toBe("Dash Platform did not answer. Try again.");
  });
});

describe("DEC-1057: an unrecorded spend", () => {
  it("passes the user's confirmation to a send and a move, and not otherwise", async () => {
    const { flow, api } = setup();
    flow.begin("out");
    flow.next({ kind: "out", credits: "10000000000" });
    await flow.run(GRANT);
    expect(api.unshieldToCore.mock.calls[0][4]).toBe(false);
    flow.close();
    flow.begin("out");
    flow.next({ kind: "out", credits: "10000000000" });
    await flow.run(GRANT, true);
    expect(api.unshieldToCore.mock.calls[1][4]).toBe(true);
  });

  it("offers Back after that refusal, so a status that was stale can be read again and ask", () => {
    expect(shieldedCanChange(appError("shielded.unrecorded_spend"))).toBe(true);
  });

  it("says in words that an earlier payment may still go through when the engine refuses without it", () => {
    const text = shieldedErrorText(appError("shielded.unrecorded_spend"), credits, duffs, "send");
    expect(text).toBe("A shielded payment from before the restart may still go through. Confirm to send anyway.");
  });
});
