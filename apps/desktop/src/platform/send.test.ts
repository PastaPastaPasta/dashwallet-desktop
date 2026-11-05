// Pay from Platform: the amount the form asks for, the send of the quote's operation under a fresh
// platformOp grant, the retry of a held send, the Confirm that Activity overtakes, and the
// refusals' texts.

import { afterEach, describe, expect, it, vi } from "vitest";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import type { PlatformSendOutcome } from "../api/generated/PlatformSendOutcome";
import type { PlatformSendQuote } from "../api/generated/PlatformSendQuote";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { fakeVault } from "../tools/testApi";
import { onMoveSettled } from "./move";
import { canRetry, heldSend, PlatformSendFlow, recipientText, resetPlatformSends, retryAmount, retryAsks, SEND_COPY, sendAmount, sendCanRetry, sendErrorText, sendFor, sendRefusalText, settledBy, staleConfirm } from "./send";

const TO = "tdash1kq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h";
const QUOTE: PlatformSendQuote = {
  credits: "1019500000",
  amountCredits: "1000000000",
  feeCredits: "6500000",
  maxFeeCredits: "19500000",
  feeFromAmount: false,
  to: TO,
  grant: { maxDuffs: 0, maxCredits: "1019500000" },
  operation: "op-1",
  retryOf: null,
};
const OUTCOME: PlatformSendOutcome = { id: "ps-1", credits: "1006500000", amountCredits: "1000000000", feeFromAmount: false, to: TO, state: "done", mayLand: false, feeCredits: "6500000", estimated: false };
const FAILED: PlatformSendOutcome = { ...OUTCOME, credits: "1019500000", state: "failed", mayLand: true, feeCredits: null };
const credits = (c: string) => `${Number(BigInt(c) / 1000n) / 1e8} tDASH`;

function setup() {
  const vault = fakeVault({ encrypted: false });
  const calls: string[] = [];
  let answer: ((o: PlatformSendOutcome) => void) | null = null;
  let refuse: ((e: unknown) => void) | null = null;
  const api = {
    ...vault.api,
    platformSend: vi.fn(
      (_walletId: string, operation: string, grant: string) =>
        new Promise<PlatformSendOutcome>((resolve, reject) => {
          calls.push(`send ${operation} ${grant}`);
          answer = resolve;
          refuse = reject;
        }),
    ),
    platformSendQuote: vi.fn(async (_walletId: string, _to: string, _amount: { kind: string; credits?: string }, _retryOf: string | null): Promise<PlatformSendQuote> => ({ ...QUOTE, operation: "op-2", retryOf: "ps-1" })),
  };
  const flow = new PlatformSendFlow(api, WALLET_A);
  return { flow, api, vault, calls, answer: (o: PlatformSendOutcome) => answer!(o), refuse: (e: unknown) => refuse!(e) };
}

const row = (id: string, over: Partial<PlatformActivity> = {}): PlatformActivity => ({
  id,
  kind: "send",
  state: "failed",
  credits: "1019500000",
  address: null,
  to: TO,
  expectedDuffs: null,
  feeCredits: null,
  at: 1,
  mayLand: true,
  estimated: false,
  ...over,
});

afterEach(() => resetPlatformSends());

describe("Pay from Platform", () => {
  it("asks for Max as all, and an amount as its credits", () => {
    expect(sendAmount(true, undefined)).toEqual({ kind: "all" });
    expect(sendAmount(false, 1_000_000)).toEqual({ kind: "credits", credits: "1000000000" });
    expect([sendAmount(false, 0), sendAmount(false, null), sendAmount(false, undefined)]).toEqual([null, null, null]);
  });

  it("sends the quote's operation under a fresh platformOp grant, one call at a time, and re-reads the balances after", async () => {
    const { flow, vault, calls, answer } = setup();
    const settled = vi.fn();
    const stop = onMoveSettled(settled);
    const going = flow.send(QUOTE);
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    expect(vault.calls.authorize[0]).toMatchObject({ purpose: { kind: "platformOp", maxDuffs: 0, maxCredits: "1019500000" }, walletId: WALLET_A });
    await flow.send({ ...QUOTE, operation: "op-9" });
    expect(calls).toEqual(["send op-1 grant-1"]);
    answer(OUTCOME);
    await going;
    await Promise.resolve();
    expect([flow.step, flow.outcome, settled.mock.calls.length]).toEqual(["done", OUTCOME, 1]);
    flow.reset();
    expect([flow.step, flow.outcome]).toEqual(["confirm", null]);
    stop();
  });

  it("takes a fresh grant for every send, never a lease", async () => {
    const { flow, vault, answer } = setup();
    for (const [i, operation] of ["op-1", "op-2"].entries()) {
      const going = flow.send({ ...QUOTE, operation });
      await vi.waitFor(() => expect(flow.step).toBe("running"));
      answer(OUTCOME);
      await going;
      flow.reset();
      expect(vault.calls.authorize).toHaveLength(i + 1);
    }
  });

  it("spends the operation on any attempt that ran (done, refused), so the form quotes again before the next Confirm", async () => {
    const { flow, refuse, answer } = setup();
    expect([flow.isSpent("op-1"), flow.attempts]).toEqual([false, 0]);
    const first = flow.send(QUOTE);
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    answer(FAILED);
    await first;
    expect([flow.isSpent("op-1"), flow.isSpent("op-2"), flow.attempts, flow.step]).toEqual([true, false, 1, "done"]);
    flow.reset();
    const second = flow.send({ ...QUOTE, operation: "op-2" });
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    refuse(appError("platform.unavailable"));
    await second;
    expect([flow.isSpent("op-2"), flow.attempts, flow.step]).toEqual([true, 2, "refused"]);
  });

  it("a cancelled passphrase ran nothing: the operation is still good", async () => {
    const vault = fakeVault({ encrypted: true });
    const api = { ...vault.api, platformSend: vi.fn(), platformSendQuote: vi.fn() };
    const flow = new PlatformSendFlow(api, WALLET_A);
    const going = flow.send(QUOTE);
    await vi.waitFor(() => expect(flow.grants.pending).not.toBeNull());
    flow.grants.cancel();
    await going;
    expect([flow.isSpent("op-1"), flow.attempts, flow.step, api.platformSend.mock.calls.length]).toEqual([false, 0, "confirm", 0]);
  });

  it("keeps a refusal to show, and offers Back only when a new quote may pass or the operation expired", async () => {
    const { flow, refuse } = setup();
    const going = flow.send(QUOTE);
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    refuse(appError("credits.move_pending", { kind: "artifact", artifact: "pm-1" }));
    await going;
    expect(flow.step).toBe("refused");
    expect(sendCanRetry(flow.error)).toBe(false);
    expect(sendCanRetry(appError("platform.grant_exceeded"))).toBe(true);
    expect(sendCanRetry(appError("platform.insufficient_credits"))).toBe(true);
    expect(sendCanRetry(appError("invalid_argument", { kind: "reason", reason: "unknown or used operation" }))).toBe(true);
  });

  it("an unknown or used operation says the details expired, and Back re-quotes", async () => {
    const { flow, refuse } = setup();
    const going = flow.send(QUOTE);
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    const gone = appError("invalid_argument", { kind: "reason", reason: "unknown or used operation" });
    refuse(gone);
    await going;
    expect([flow.step, sendRefusalText(flow.error, credits), flow.isSpent("op-1")]).toEqual(["refused", "This payment's details expired. Review it again.", true]);
    expect(sendCanRetry(flow.error)).toBe(true);
  });

  describe("Retry this payment", () => {
    it("offers it for a failed send that may land, not for one that is done, pending or gone", () => {
      expect([canRetry(FAILED), canRetry(OUTCOME), canRetry({ state: "pending", mayLand: true }), canRetry({ state: "failed", mayLand: false }), canRetry(null)]).toEqual([true, false, false, false, false]);
    });

    it("asks the same amount again, or Max again", () => {
      expect(retryAmount({ feeFromAmount: false, amountCredits: "1000000000" })).toEqual({ kind: "credits", credits: "1000000000" });
      expect(retryAmount({ feeFromAmount: true, amountCredits: "40000000000" })).toEqual({ kind: "all" });
    });

    it("quotes the same recipient and amount from the outcome, and goes back to Confirm for the quote", async () => {
      const { flow, api, answer } = setup();
      const going = flow.send(QUOTE);
      await vi.waitFor(() => expect(flow.step).toBe("running"));
      answer(FAILED);
      await going;
      const got = await flow.retry("tdash1other");
      expect(api.platformSendQuote).toHaveBeenCalledWith(WALLET_A, TO, { kind: "credits", credits: "1000000000" }, "ps-1");
      expect(got).toMatchObject({ quote: { operation: "op-2", retryOf: "ps-1" }, amount: { kind: "credits", credits: "1000000000" } });
      expect([flow.step, flow.outcome, flow.isSpent("op-2")]).toEqual(["confirm", null, false]);
    });

    it("retries a Max send as Max, and a send with no recipient on the outcome with the form's", async () => {
      const { flow, api, answer } = setup();
      const going = flow.send(QUOTE);
      await vi.waitFor(() => expect(flow.step).toBe("running"));
      answer({ ...FAILED, feeFromAmount: true, amountCredits: "40000000000", to: null });
      await going;
      await flow.retry("tdash1form");
      expect(api.platformSendQuote).toHaveBeenCalledWith(WALLET_A, "tdash1form", { kind: "all" }, "ps-1");
    });

    it("shows the quote's refusal when the retry cannot be quoted, and sends nothing", async () => {
      const { flow, api, answer } = setup();
      const going = flow.send(QUOTE);
      await vi.waitFor(() => expect(flow.step).toBe("running"));
      answer(FAILED);
      await going;
      api.platformSendQuote.mockRejectedValueOnce(appError("credits.move_pending", { kind: "artifact", artifact: "pm-3" }));
      expect(await flow.retry(TO)).toBeNull();
      expect([flow.step, sendRefusalText(flow.error, credits)]).toEqual(["refused", SEND_COPY.held]);
      expect(api.platformSend).toHaveBeenCalledTimes(1);
    });

    it("names the outcome's id as retryOf, and offers the retry only when the engine answers it as that send's", async () => {
      const { flow, api, answer } = setup();
      const going = flow.send(QUOTE);
      await vi.waitFor(() => expect(flow.step).toBe("running"));
      answer(FAILED);
      await going;
      // The held send landed meanwhile: a quote that is not its retry is never offered (the sheet follows Activity).
      api.platformSendQuote.mockResolvedValueOnce({ ...QUOTE, operation: "op-2", retryOf: null });
      expect(await flow.retry(TO)).toBeNull();
      expect([flow.step, flow.noRetry, flow.error, flow.outcome]).toEqual(["done", true, null, FAILED]);
      api.platformSendQuote.mockResolvedValueOnce({ ...QUOTE, operation: "op-3", retryOf: "ps-other" });
      expect(await flow.retry(TO)).toBeNull();
      expect(api.platformSendQuote).toHaveBeenLastCalledWith(WALLET_A, TO, { kind: "credits", credits: "1000000000" }, "ps-1");
      expect(await flow.retry(TO)).toMatchObject({ quote: { retryOf: "ps-1" } });
      expect(flow.noRetry).toBe(false);
    });

    it("remembers which send the last attempt retried", async () => {
      const { flow, answer } = setup();
      const first = flow.send(QUOTE);
      await vi.waitFor(() => expect(flow.step).toBe("running"));
      answer(FAILED);
      await first;
      expect(flow.retryOf).toBeNull();
      const got = await flow.retry(TO);
      const second = flow.send(got!.quote);
      await vi.waitFor(() => expect(flow.step).toBe("running"));
      answer(OUTCOME);
      await second;
      expect([flow.retryOf, flow.outcome]).toEqual(["ps-1", OUTCOME]);
    });

    it("retries a held send from the form (no outcome): its credits first, Max when the engine says that is not its amount", async () => {
      const { flow, api } = setup();
      const held = row("ps-9", { credits: "40000000000" });
      expect(retryAsks(held)).toEqual([{ kind: "credits", credits: "40000000000" }, { kind: "all" }]);
      api.platformSendQuote.mockRejectedValueOnce(appError("invalid_argument", { kind: "reason", reason: "not that send's retry" }));
      api.platformSendQuote.mockResolvedValueOnce({ ...QUOTE, operation: "op-5", retryOf: "ps-9", feeFromAmount: true });
      const got = await flow.quoteRetry({ id: "ps-9", to: TO }, retryAsks(held));
      expect(api.platformSendQuote.mock.calls.map((c) => [c[2], c[3]])).toEqual([[{ kind: "credits", credits: "40000000000" }, "ps-9"], [{ kind: "all" }, "ps-9"]]);
      expect(got).toMatchObject({ amount: { kind: "all" }, quote: { retryOf: "ps-9" } });
    });

    it("a refusal other than invalid_argument stops the form's retry at the first ask", async () => {
      const { flow, api } = setup();
      api.platformSendQuote.mockRejectedValueOnce(appError("credits.move_pending", { kind: "artifact", artifact: "ps-9" }));
      expect(await flow.quoteRetry({ id: "ps-9", to: TO }, retryAsks(row("ps-9")))).toBeNull();
      expect(api.platformSendQuote).toHaveBeenCalledTimes(1);
      expect(flow.step).toBe("refused");
    });

    it("does nothing for a result that cannot be retried", async () => {
      const { flow, api } = setup();
      expect(await flow.retry(TO)).toBeNull();
      expect(api.platformSendQuote).not.toHaveBeenCalled();
    });
  });

  describe("a Confirm that Activity overtakes", () => {
    const retry = { retryOf: "ps-held" };
    it("goes back with 'went through' when the retried send is done", () => {
      expect(staleConfirm(retry, [row("ps-held")], [row("ps-held", { state: "done", mayLand: false })])).toBe("went");
    });

    it("goes back with 'went through' when the retried send was replaced: its identical retry went through", () => {
      expect(staleConfirm(retry, [row("ps-held")], [row("ps-held", { state: "replaced", mayLand: false })])).toBe("went");
    });

    it("goes back with 'can no longer go through' when it failed for good", () => {
      expect(staleConfirm(retry, [row("ps-held")], [row("ps-held", { state: "failed", mayLand: false })])).toBe("gone");
    });

    it("stays while the retried send may still land, or Activity has not answered or does not list it", () => {
      expect(staleConfirm(retry, [row("ps-held")], [row("ps-held")])).toBeNull();
      expect(staleConfirm(retry, [row("ps-held")], [row("ps-held", { state: "pending" })])).toBeNull();
      expect(staleConfirm(retry, [row("ps-held")], null)).toBeNull();
      expect(staleConfirm(retry, [row("ps-held")], undefined)).toBeNull();
      expect(staleConfirm(retry, [row("ps-held")], [])).toBeNull();
    });

    it("goes back when a hold appeared that was not there when a new send's Confirm opened", () => {
      const fresh = { retryOf: null };
      expect(staleConfirm(fresh, [], [row("ps-2")])).toBe("held");
      expect(staleConfirm(fresh, [], [row("pm-2", { kind: "moveToCore", state: "pending" })])).toBe("held");
      // One already listed when it opened (its nonces were seen taken by the quote's read) does not revoke it.
      expect(staleConfirm(fresh, [row("ps-2")], [row("ps-2")])).toBeNull();
      expect(staleConfirm(fresh, [], [row("ps-3", { state: "done", mayLand: false })])).toBeNull();
    });
  });

  describe("a result sheet that follows Activity", () => {
    const unsettled = { id: "ps-1", mayLand: true };
    it("says it went through when the shown payment is done or replaced", () => {
      expect(settledBy(unsettled, [row("ps-1", { state: "done", mayLand: false })])).toBe("went");
      expect(settledBy(unsettled, [row("ps-1", { state: "replaced", mayLand: false })])).toBe("went");
      expect(settledBy({ id: "ps-1", mayLand: true }, [row("ps-1", { state: "done", mayLand: false })])).toBe("went");
    });

    it("says it was refused when it failed for good, and nothing while it may still land", () => {
      expect(settledBy(unsettled, [row("ps-1", { state: "failed", mayLand: false })])).toBe("refused");
      expect(settledBy(unsettled, [row("ps-1")])).toBeNull();
      expect(settledBy(unsettled, [row("ps-1", { state: "pending" })])).toBeNull();
    });

    it("says nothing while Activity has not answered or lacks the row, and for an outcome that cannot land", () => {
      expect(settledBy(unsettled, null)).toBeNull();
      expect(settledBy(unsettled, undefined)).toBeNull();
      expect(settledBy(unsettled, [row("ps-2", { state: "done", mayLand: false })])).toBeNull();
      expect(settledBy({ id: "ps-1", mayLand: false }, [row("ps-1", { state: "done", mayLand: false })])).toBeNull();
      expect(settledBy(null, [row("ps-1", { state: "done", mayLand: false })])).toBeNull();
    });

    it("picks the send that may still go through for the form's retry, never a move or a settled send", () => {
      expect(heldSend([row("pm-1", { kind: "moveToCore" }), row("ps-2", { mayLand: false }), row("ps-3")])?.id).toBe("ps-3");
      expect(heldSend([row("ps-4", { state: "pending" })])).toBeNull();
      expect(heldSend([row("ps-5", { to: null })])).toBeNull();
      expect(heldSend(null)).toBeNull();
    });
  });

  it("names a payment Platform refused for good, and keeps the held copy for the rest", () => {
    expect(SEND_COPY.settled.refused).toEqual({ title: "Not sent", text: "Dash Platform refused this payment. Nothing was sent; you can send it again." });
    expect(SEND_COPY.retryNote).toBe("This retries your earlier payment. If that one went through, nothing more is sent.");
    expect(SEND_COPY.earlier("0.01 tDASH", "tdash1x")).toBe("An earlier payment of 0.01 tDASH to tdash1x may still go through.");
    expect(SEND_COPY.retryThat).toBe("Retry that payment");
  });

  it("keeps one flow per wallet", () => {
    const api = setup().flow;
    expect(api).toBeInstanceOf(PlatformSendFlow);
    const vault = fakeVault({ encrypted: false });
    const a = sendFor({ ...vault.api, platformSend: vi.fn(), platformSendQuote: vi.fn() }, WALLET_A);
    expect(sendFor({ ...vault.api, platformSend: vi.fn(), platformSendQuote: vi.fn() }, WALLET_A)).toBe(a);
  });

  it("says why a recipient is refused", () => {
    const text = (reason: string) => sendErrorText(appError("credits.recipient_invalid", { kind: "reason", reason }), credits);
    expect(text("core")).toBe(recipientText("core"));
    expect(text("core")).toMatch(/Dash address/);
    expect(text("shielded")).toMatch(/shielded address/);
    expect(text("network")).toMatch(/another network/);
    expect(text("own")).toMatch(/your own Platform addresses/);
    expect(text("malformed")).toMatch(/not a valid Platform address/);
  });

  it("names the minimum, the shortfall and a held payment or move", () => {
    expect(sendErrorText(appError("credits.send_out_of_range", { kind: "creditMinimum", min: "500000" }), credits)).toBe(
      "Dash Platform cannot send this amount: the payment, and what it takes from each of your Platform addresses, must be at least 0.000005 tDASH. Try another amount, or Max.",
    );
    expect(sendErrorText(appError("platform.insufficient_credits", { kind: "credits", needed: "2000000000", available: "1000000000" }), credits)).toBe(
      "Not enough on your Platform addresses: 0.02 tDASH needed with the fee, 0.01 tDASH available.",
    );
    expect(sendErrorText(appError("credits.move_pending", { kind: "artifact", artifact: "ps-1" }), credits)).toBe(
      "An earlier payment or move from Platform may still go through. Wait for it to finish, or pay the same address the same amount again: only that retry can go now.",
    );
    expect(sendErrorText(appError("platform.broadcast_unknown", { kind: "artifact", artifact: "x" }), credits)).toMatch(/may have been sent/);
  });

  it("says about when the debit counts the fee estimate, and no fee 'up to' wording anywhere", () => {
    expect(SEND_COPY.done.text("0.01 tDASH", false)).toBe("0.01 tDASH left your Platform balance, the fee included. Activity shows the payment.");
    expect(SEND_COPY.done.text("0.01 tDASH", true)).toBe("About 0.01 tDASH left your Platform balance, the fee included. Activity shows the payment.");
    expect(SEND_COPY.fee("a", "b")).toBe("Platform fee about a (at most b)");
    expect(SEND_COPY.fee("a", null)).toBe("Platform fee about a, from the amount");
    expect(SEND_COPY.feeValue("a", "b")).toBe("about a, at most b");
    expect(SEND_COPY.feeValue("a", null)).toBe("about a");
    expect(JSON.stringify(SEND_COPY).toLowerCase()).not.toMatch(/up to|expected/);
  });
});
