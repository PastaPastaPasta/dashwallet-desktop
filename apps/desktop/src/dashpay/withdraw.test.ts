// The withdraw flow (DP6-02): Max sends `all` so the engine keeps the fee in reserve, an explicit
// amount in credits, the form → confirm → running → done path under the quote's grant, refusals,
// and My Profile's recent withdrawals with their status and polling.

import { afterEach, describe, expect, it, vi } from "vitest";
import type { Withdrawal } from "../api/generated/Withdrawal";
import type { WithdrawOutcome } from "../api/generated/WithdrawOutcome";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { fakeVault } from "../tools/testApi";
import {
  amountCanChange,
  amountKey,
  onWithdrawSettled,
  PENDING_POLL_MS,
  pollWhilePending,
  RECENT_SHOWN,
  resetWithdrawals,
  statusText,
  withdrawalRows,
  withdrawAmount,
  withdrawErrorText,
  withdrawFor,
  WithdrawFlow,
} from "./withdraw";

const TO = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";
const COSTS = { creditsPerDuff: 1000 };
const amount = (duffs: number) => `${duffs / 1e8} tDASH`;
const credits = (c: string) => amount(Number(BigInt(c) / 1000n));

function setup(lock = { encrypted: false }) {
  const vault = fakeVault(lock);
  const calls: string[] = [];
  let answer: ((o: WithdrawOutcome) => void) | null = null;
  const api = {
    ...vault.api,
    creditsWithdraw: vi.fn(
      (_walletId: string, identity: string, to: string, a: { kind: string; credits?: string }, grant: string) =>
        new Promise<WithdrawOutcome>((resolve) => {
          calls.push(`withdraw ${identity} ${to} ${a.kind === "all" ? "all" : a.credits} ${grant}`);
          answer = resolve;
        }),
    ),
  };
  const flow = new WithdrawFlow(api, WALLET_A);
  return { flow, api, vault, calls, answer: (o: WithdrawOutcome) => answer!(o) };
}

const OUTCOME: WithdrawOutcome = { credits: "184099000000", expectedDuffs: 184_098_810, remainingCredits: "1000000", withdrawal: "Wd" };
const GRANT = { maxDuffs: 0, maxCredits: "184499000000" };

const doc = (id: string, status: Withdrawal["status"], patch: Partial<Withdrawal> = {}): Withdrawal => ({
  id,
  credits: "50000000000",
  expectedDuffs: 49_999_810,
  to: TO,
  status,
  createdAt: 1_000_000,
  updatedAt: 2_000_000,
  ...patch,
});

afterEach(() => resetWithdrawals());

describe("the withdraw sheet (DP6-02)", () => {
  it("D1, D10: Max is `all` for the engine to reserve the fee; an amount is credits at the table's rate", () => {
    // Mutant "Max without the reserve" in the UI: sending the balance as credits instead of `all`.
    expect(withdrawAmount(true, 5_000_000, COSTS)).toEqual({ kind: "all" });
    expect(withdrawAmount(true, null, undefined)).toEqual({ kind: "all" });
    expect(withdrawAmount(false, 5_000_000, COSTS)).toEqual({ kind: "credits", credits: "5000000000" });
    // Nothing to send without an amount, a positive one, or the rate.
    expect(withdrawAmount(false, null, COSTS)).toBeNull();
    expect(withdrawAmount(false, 0, COSTS)).toBeNull();
    expect(withdrawAmount(false, 5_000_000, undefined)).toBeNull();
    expect([amountKey({ kind: "all" }), amountKey({ kind: "credits", credits: "7" })]).toEqual(["all", "7"]);
  });

  it("form → confirm → running → done under the quote's platformOp grant; one call at a time", async () => {
    const { flow, vault, calls, answer } = setup();
    const settled = vi.fn();
    const stop = onWithdrawSettled(settled);
    flow.begin("A");
    expect([flow.open, flow.step, flow.max, flow.to]).toEqual([true, "form", true, null]);
    flow.setText("0.5");
    expect(flow.max).toBe(false);
    flow.setMax(true);
    flow.next({ kind: "all" }, TO);
    expect(flow.step).toBe("confirm");
    const going = flow.withdraw(GRANT);
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    expect(vault.calls.authorize[0]).toMatchObject({ purpose: { kind: "platformOp", maxDuffs: 0, maxCredits: "184499000000" }, walletId: WALLET_A });
    await flow.withdraw(GRANT);
    expect(calls).toEqual([`withdraw A ${TO} all grant-1`]);
    // Closed while it runs, reopened to its result, which the page offers meanwhile.
    flow.close();
    expect([flow.open, flow.unseenFor("A")]).toEqual([false, false]);
    answer(OUTCOME);
    await going;
    await Promise.resolve();
    expect(settled).toHaveBeenCalledTimes(1);
    expect(flow.unseenFor("A")).toBe(true);
    expect(flow.unseenFor("B")).toBe(false);
    flow.begin("A");
    expect([flow.step, flow.outcome]).toEqual(["done", OUTCOME]);
    flow.close();
    flow.begin("A");
    expect([flow.step, flow.max, flow.destination]).toEqual(["form", true, null]);
    stop();
  });

  it("a refusal keeps its error; Back when another amount or address may pass", async () => {
    const { flow, api } = setup();
    const settled = vi.fn();
    const stop = onWithdrawSettled(settled);
    api.creditsWithdraw.mockRejectedValueOnce(appError("platform.broadcast_unknown", { kind: "artifact", artifact: "Wd" }));
    flow.begin("A");
    flow.next({ kind: "credits", credits: "5000000000" }, TO);
    await flow.withdraw(GRANT);
    await Promise.resolve();
    expect(flow.step).toBe("refused");
    // An unknown outcome is never retried from here: a second withdrawal could double it (D7).
    expect(amountCanChange(flow.error)).toBe(false);
    expect(settled).toHaveBeenCalledTimes(1);
    for (const c of ["credits.withdrawal_out_of_range", "platform.insufficient_credits", "platform.grant_exceeded", "invalid_argument"] as const)
      expect(amountCanChange(appError(c)), c).toBe(true);
    flow.back();
    expect([flow.step, flow.error]).toEqual(["form", null]);
    stop();
  });

  it("a cancelled passphrase returns to the confirm step and withdraws nothing", async () => {
    const { flow, calls } = setup({ encrypted: true });
    const settled = vi.fn();
    const stop = onWithdrawSettled(settled);
    flow.begin("A");
    flow.next({ kind: "all" }, TO);
    const going = flow.withdraw(GRANT);
    await vi.waitFor(() => expect(flow.grants.pending).not.toBeNull());
    flow.grants.cancel();
    await going;
    await Promise.resolve();
    expect([flow.step, calls]).toEqual(["confirm", []]);
    expect(settled).not.toHaveBeenCalled();
    stop();
  });

  it("refusal copy by code (§3), in Dash", () => {
    expect(withdrawErrorText(appError("credits.withdrawal_out_of_range", { kind: "creditRange", min: "1190000", max: "50000000000000" }), credits)).toBe(
      "Withdraw between 0.0000119 tDASH and 500 tDASH.",
    );
    expect(withdrawErrorText(appError("credits.withdrawal_out_of_range"), credits)).toBe("Dash Platform does not take a withdrawal of this amount.");
    expect(withdrawErrorText(appError("platform.insufficient_credits", { kind: "credits", needed: "401190000", available: "4000000" }), credits)).toBe(
      "Not enough credits: 0.0040119 tDASH needed with the fee, 0.00004 tDASH available.",
    );
    expect(withdrawErrorText(appError("identity.keys_missing", { kind: "keysMissing", purpose: "transfer" }), credits)).toBe(
      "This identity has no transfer key in this wallet, so it cannot withdraw.",
    );
    expect(withdrawErrorText(appError("platform.broadcast_unknown"), credits)).toBe("It may have been sent. Check your withdrawals in My Profile before trying again.");
    expect(withdrawErrorText(appError("platform.trust_mismatch"), credits)).toBe("This identity's data could not be verified against the network. Nothing was withdrawn.");
    expect(withdrawErrorText(appError("invalid_argument"), credits)).toBe("Enter a Dash address of this network.");
    expect(withdrawErrorText(appError("platform.grant_invalid"), credits)).toBe("It stopped with platform.grant_invalid.");
  });

  it("withdrawFor keeps one flow per wallet", () => {
    const { api } = setup();
    expect(withdrawFor(api, WALLET_A)).toBe(withdrawFor(api, WALLET_A));
    expect(withdrawFor(api, "other")).not.toBe(withdrawFor(api, WALLET_A));
  });
});

describe("My Profile's recent withdrawals (D11)", () => {
  it("§3: Platform's statuses; an expired payout is re-signed, so it stays pending", () => {
    expect((["queued", "pooled", "broadcasted", "expired"] as const).map((s) => statusText(s))).toEqual(Array(4).fill({ text: "Pending", pending: true }));
    expect(statusText("complete")).toEqual({ text: "Sent", pending: false });
    expect(statusText("failed")).toEqual({ text: "Failed: the credits were not returned", pending: false });
    expect(statusText("unknown")).toEqual({ text: "Status unknown", pending: false });
  });

  it("shows the newest five with their payout, else the credits", () => {
    const list = Array.from({ length: 7 }, (_, i) => doc(`w${i}`, "complete"));
    const rows = withdrawalRows(list, COSTS, amount);
    expect(rows.map((r) => r.id)).toEqual(["w0", "w1", "w2", "w3", "w4"]);
    expect(rows.length).toBe(RECENT_SHOWN);
    expect(rows[0]).toEqual({ id: "w0", title: "Withdrawal of 0.4999981 tDASH", status: "Sent", pending: false, to: TO, at: 2000 });
    expect(withdrawalRows([doc("x", "queued", { expectedDuffs: null, updatedAt: null })], COSTS, amount)[0]).toMatchObject({ title: "Withdrawal of 0.5 tDASH", at: 1000 });
    expect(withdrawalRows([doc("x", "queued", { expectedDuffs: null, createdAt: null, updatedAt: null })], undefined, amount)[0]).toMatchObject({
      title: "Withdrawal of 50,000,000,000 credits",
      at: null,
    });
    expect(withdrawalRows(undefined, COSTS, amount)).toEqual([]);
  });

  it("reads again every minute while a shown row is pending, else only on signals", () => {
    expect(pollWhilePending([doc("a", "complete"), doc("b", "broadcasted")])).toBe(PENDING_POLL_MS);
    expect(pollWhilePending([doc("a", "complete"), doc("b", "failed")])).toBe(false);
    expect(pollWhilePending(undefined)).toBe(false);
    // A pending row past the five shown does not keep the page polling.
    const old = [...Array.from({ length: 5 }, (_, i) => doc(`c${i}`, "complete")), doc("p", "queued")];
    expect(pollWhilePending(old)).toBe(false);
  });
});
