// The way out of a held Platform item: "Move to Dash wallet" beside a held send, the cancel beside a
// held move or contact payment. Only one of the two goes through, and the copy says which did only
// once Activity shows it.

import { afterEach, describe, expect, it, vi } from "vitest";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import type { PlatformCancelQuote } from "../api/generated/PlatformCancelQuote";
import type { PlatformSendOutcome } from "../api/generated/PlatformSendOutcome";
import { platformRow, sourceRowTitle } from "../history/sourceRows";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { fakeVault } from "../tools/testApi";
import { cancelFor, cancelResult, CANCEL_COPY, PlatformCancelFlow, recipientOf, resetPlatformCancels } from "./cancel";
import { cancelOf, isHeldSend, MOVE_COPY, moveErrorText, moveOutcomeCopy } from "./move";

const PAYEE = "tdash1kq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h";
const OWN = "tdash1k9w4v7d2n8x3c5m0q6z8r2t4y6u8p0s2a4d6f8h0";
const base = { credits: "40000000000", address: null, to: null, expectedDuffs: null, feeCredits: null, at: 1, estimated: false } as const;
const held = (id: string, kind: PlatformActivity["kind"], extra: Partial<PlatformActivity> = {}): PlatformActivity => ({ ...base, id, kind, state: "failed", mayLand: true, ...extra });
const QUOTE: PlatformCancelQuote = { of: "pm-1", credits: "40000000000", feeCredits: "6500000", to: OWN, grant: { maxDuffs: 0, maxCredits: "40000000000" } };
const OUT: PlatformSendOutcome = { id: "pc-2", credits: "40000000000", amountCredits: "40000000000", feeFromAmount: true, to: OWN, state: "done", mayLand: false, feeCredits: "6500000", estimated: false };
const credits = (c: string) => `${Number(BigInt(c) / 1000n) / 1e8} tDASH`;
const amount = (d: number) => `${d / 1e8} tDASH`;

afterEach(() => resetPlatformCancels());

describe("which way out a held item offers", () => {
  it("a held send: Move to Dash wallet, never a cancel; a cancel row is no send to move", () => {
    const send = held("ps-1", "send", { to: PAYEE });
    expect([isHeldSend(send), cancelOf(send, [send])]).toEqual([true, null]);
    expect(isHeldSend({ ...send, state: "pending" })).toBe(false);
    expect(isHeldSend({ ...send, mayLand: false })).toBe(false);
    expect(isHeldSend({ ...send, cancels: "pm-1", to: OWN })).toBe(false);
  });

  it("a held move or contact payment: its cancel, unless a send, top-up, pending item or another cancel holds beside it", () => {
    const move = held("pm-1", "moveToCore");
    const payout = held("cp-1", "contactPayment");
    expect([cancelOf(move, [move]), cancelOf(payout, [payout])]).toEqual(["move", "payment"]);
    // A move and its retry at the same inputs: still one withdrawal's document decides.
    expect(cancelOf(move, [move, held("pm-2", "moveToCore")])).toBe("move");
    for (const beside of [held("ps-1", "send", { to: PAYEE }), held("ti-1", "toIdentity"), { ...held("pm-2", "moveToCore"), state: "pending" as const }, held("pc-1", "send", { to: OWN, cancels: "pm-9" }), held("pc-1", "send", { to: OWN, cancels: "pm-1", state: "pending" })]) expect(cancelOf(move, [move, beside])).toBeNull();
    // Its own cancel that still holds: offered again, re-sent as it was signed.
    expect(cancelOf(move, [move, held("pc-1", "send", { to: OWN, cancels: "pm-1" })])).toBe("move");
    expect(cancelOf(payout, [payout, held("pc-2", "send", { to: OWN, cancels: "cp-1" })])).toBe("payment");
    expect(cancelOf({ ...move, state: "pending" }, [])).toBeNull();
    expect(cancelOf({ ...move, mayLand: false }, [])).toBeNull();
    expect(cancelOf({ ...move, state: "done", mayLand: false }, [])).toBeNull();
  });

  it("on mainnet, while new moves are off: no new cancel, only a held cancel's re-send", () => {
    const move = held("pm-1", "moveToCore");
    const payout = held("cp-1", "contactPayment");
    expect([cancelOf(move, [move], true), cancelOf(payout, [payout], true), cancelOf(move, undefined, true)]).toEqual([null, null, null]);
    expect(cancelOf(move, [move, held("pc-1", "send", { to: OWN, cancels: "pm-1" })], true)).toBe("move");
    expect(cancelOf(payout, [payout, held("pc-2", "send", { to: OWN, cancels: "cp-1" })], true)).toBe("payment");
    expect(cancelOf(move, [move, held("pc-1", "send", { to: OWN, cancels: "pm-1", state: "done", mayLand: false })], true)).toBeNull();
  });

  it("names the recipient as its row does: the contact, else the shortened address", () => {
    expect(recipientOf(held("ps-1", "send", { to: PAYEE }))).toBe("tdash1kq7r3m…8p0s2a4d6f8h");
    expect(recipientOf(held("cp-1", "contactPayment", { contact: { identity: "x", username: "alice", displayName: "Alice", avatarUrl: null } }))).toBe("Alice");
  });
});

describe("the move out of a held send", () => {
  const outcome = { id: "pm-3", credits: "30000000000", expectedDuffs: 29_998_000, to: "y", state: "done", feeCredits: "2000000" } as const;

  it("says the recipient wasn't paid once it went through, and was paid once the payment did", () => {
    expect(moveOutcomeCopy(outcome, false, credits, amount, "tdash1kq…6f8h")).toEqual({ title: MOVE_COPY.done.title, text: "Moved to Dash wallet. The payment to tdash1kq…6f8h didn't go through, so tdash1kq…6f8h wasn't paid." });
    expect(moveOutcomeCopy({ ...outcome, state: "failed" }, true, credits, amount, "Alice")).toEqual({ title: MOVE_COPY.failed.title, text: "The payment went through before the move, so Alice was paid. The move wasn't charged." });
  });

  it("says nothing of the payment while neither is known", () => {
    expect(moveOutcomeCopy({ ...outcome, state: "pending" }, false, credits, amount, "Alice").title).toBe(MOVE_COPY.pending.title);
  });

  it("names the payment, its amount and recipient, on Confirm", () => {
    expect(MOVE_COPY.exitSend.line("0.01 tDASH", "Alice")).toBe(
      "This payment of 0.01 tDASH to Alice may still go through. Moving uses the same Platform funds to send to your Dash wallet instead, so only one of the two can go through. If the move goes through, Alice isn't paid, and the amount, less fees, goes to your Dash wallet.",
    );
  });
});

describe("the cancel's outcome", () => {
  it("is done only once Activity shows the cancel done, lost once it failed for good, else waiting", () => {
    expect(cancelResult(OUT, { ...held("pc-2", "send", { cancels: "pm-1" }), state: "done", mayLand: false })).toBe("done");
    expect(cancelResult(OUT, { ...held("pc-2", "send", { cancels: "pm-1" }), mayLand: false })).toBe("lost");
    expect(cancelResult(OUT, held("pc-2", "send", { cancels: "pm-1" }))).toBe("pending");
    expect(cancelResult({ ...OUT, state: "pending", mayLand: true }, undefined)).toBe("pending");
    // A group settled with no evidence of which went through is no proof the cancel did.
    expect(cancelResult(OUT, { ...held("pc-2", "send", { cancels: "pm-1" }), state: "done", mayLand: false, winnerKnown: false })).toBe("pending");
  });

  it("names the move or payment it cancels in Activity", () => {
    const move = held("pm-1", "moveToCore");
    const payout = held("cp-1", "contactPayment");
    const cancel = (of: string) => ({ ...held("pc-2", "send", { to: OWN, cancels: of }), state: "done" as const, mayLand: false });
    expect(sourceRowTitle(platformRow(cancel("pm-1"), [cancel("pm-1"), move]))).toBe("Cancel of an earlier move");
    expect(sourceRowTitle(platformRow(cancel("cp-1"), [cancel("cp-1"), payout]))).toBe("Cancel of an earlier payment");
    // It pays its own address: the amount out, as a send's.
    expect(platformRow(cancel("pm-1"), [move]).amount).toBe(-40_000_000);
  });

  it("words the move's and the payment's cancel apart", () => {
    expect(CANCEL_COPY.body("move", "0.4 tDASH", "0.0000065 tDASH", "")).toContain("this move of 0.4 tDASH to your Dash wallet");
    expect(CANCEL_COPY.body("payment", "0.4 tDASH", "0.0000065 tDASH", "Alice")).toContain("If the cancel goes through, Alice isn't paid");
    expect([CANCEL_COPY.confirm("move"), CANCEL_COPY.confirm("payment"), CANCEL_COPY.dismiss]).toEqual(["Cancel move", "Cancel payment", "Keep waiting"]);
  });
});

describe("the cancel flow", () => {
  function setup(lock = { encrypted: false }) {
    const vault = fakeVault(lock);
    let answer: ((o: PlatformSendOutcome) => void) | null = null;
    const api = {
      ...vault.api,
      platformCancelQuote: vi.fn(async (_w: string, of: string) => ({ ...QUOTE, of })),
      platformCancel: vi.fn((_w: string, _of: string, _to: string, _grant: string) => new Promise<PlatformSendOutcome>((resolve) => (answer = resolve))),
    };
    return { flow: new PlatformCancelFlow(api, WALLET_A), api, answer: (o: PlatformSendOutcome) => answer!(o) };
  }

  it("quotes the held item, then cancels to the quote's own address under its grant, once", async () => {
    const { flow, api, answer } = setup();
    await flow.begin("pm-1");
    expect([flow.of, flow.step, flow.quote?.to]).toEqual(["pm-1", "confirm", OWN]);
    const running = flow.confirm();
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    void flow.confirm();
    answer(OUT);
    await running;
    expect(api.platformCancel).toHaveBeenCalledTimes(1);
    expect(api.platformCancel.mock.calls[0].slice(0, 3)).toEqual([WALLET_A, "pm-1", OWN]);
    expect([flow.step, flow.outcome?.id]).toEqual(["done", "pc-2"]);
  });

  it("a refusal shows its own text; Keep waiting before Confirm runs nothing", async () => {
    const { flow, api } = setup();
    api.platformCancelQuote.mockRejectedValueOnce(appError("credits.move_pending", { kind: "artifact", artifact: "ps-1" }));
    await flow.begin("pm-1");
    expect(flow.quoteError).toMatchObject({ code: "credits.move_pending" });
    flow.dismiss();
    expect(flow.of).toBeNull();
    expect(api.platformCancel).not.toHaveBeenCalled();
  });

  it("the restore's Platform hold: its refusal says nothing was sent and what it waits for; once it clears the same cancel goes through", async () => {
    const { flow, api, answer } = setup();
    api.platformCancel.mockRejectedValueOnce(appError("platform.restored"));
    await flow.begin("pm-1");
    await flow.confirm();
    expect(flow.step).toBe("refused");
    expect(moveErrorText(flow.error, credits, [held("pm-1", "moveToCore")])).toBe(
      "Dash Wallet hasn't finished checking this wallet with Dash Platform since it restored your wallet data from before the upgrade, so Platform and Shielded funds are paused and nothing was sent. The check runs while Dash Platform is on.",
    );
    expect(cancelOf(held("pm-1", "moveToCore"), [held("pm-1", "moveToCore")])).toBe("move");
    flow.close();
    await flow.begin("pm-1");
    const running = flow.confirm();
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    answer(OUT);
    await running;
    expect(api.platformCancel).toHaveBeenCalledTimes(2);
    expect([flow.step, flow.outcome?.id]).toEqual(["done", "pc-2"]);
  });

  it("is one per wallet, kept while the app runs", () => {
    const api = setup().flow;
    expect(cancelFor(api as never, WALLET_A)).toBe(cancelFor(api as never, WALLET_A));
  });
});
