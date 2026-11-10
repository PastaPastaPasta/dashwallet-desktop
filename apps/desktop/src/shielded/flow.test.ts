// The shielded flow: each move runs under the quote's platformOp grant, the sheet can close while the
// call runs, a lock that was not applied is finished with one duff of grant, and the refusals have
// their words.

import { afterEach, describe, expect, it, vi } from "vitest";
import type { ShieldedSpendOutcome } from "../api/generated/ShieldedSpendOutcome";
import type { ShieldOutcome } from "../api/generated/ShieldOutcome";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { fakeVault } from "../tools/testApi";
import { onShieldedSettled, resetShielded, SHIELDED_COPY, shieldedCanChange, shieldedErrorText, shieldedFor, ShieldedFlow } from "./flow";

const TO = "tdash1zq0k8y";
const SPEND: ShieldedSpendOutcome = { credits: "10000000000", balance: "114980000000", to: TO, state: "done", unresolved: null };
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
    shieldFromCore: vi.fn(async (_w: string, d: number, grant: string) => (calls.push(`add ${d} ${grant}`), APPLIED)),
    resumeShield: vi.fn(async (_w: string, txid: string, grant: string) => (calls.push(`resume ${txid} ${grant}`), APPLIED)),
    shieldedSend: vi.fn(
      (_w: string, to: string, c: string, memo: string | null, grant: string) =>
        new Promise<ShieldedSpendOutcome>((resolve) => {
          calls.push(`send ${to} ${c} ${memo} ${grant}`);
          answer = resolve;
        }),
    ),
    unshieldToCore: vi.fn(async (_w: string, c: string, to: string | null, grant: string, _confirm: boolean) => (calls.push(`out ${c} ${to} ${grant}`), SPEND)),
  };
  const flow = new ShieldedFlow(api, WALLET_A);
  return { flow, api, vault, calls, answer: (o: ShieldedSpendOutcome) => answer!(o) };
}

afterEach(() => resetShielded());

describe("the shielded flow", () => {
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

  it("sends an add to Shielded that may be out to Tools ▸ Repair, and a spend to Activity", () => {
    const unknown = appError("platform.broadcast_unknown", { kind: "artifact", artifact: "shield/0/funding" });
    expect(shieldedErrorText(unknown, credits, duffs, "add")).toBe(SHIELDED_COPY.fundingUnknown);
    expect(SHIELDED_COPY.fundingUnknown).toContain("Open Tools ▸ Repair");
    for (const kind of ["send", "out"] as const) expect(shieldedErrorText(unknown, credits, duffs, kind)).toBe("It may have been sent. Check Activity before trying again.");
  });

  it("DEC1058_syncingNeverReadsAsPlatformDown", () => {
    const syncing = (code: string, on: boolean) => shieldedErrorText(appError(code as never, null as never), credits, duffs, "add", on);
    const SAID = "Shielded is still syncing; try again when it finishes.";
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
