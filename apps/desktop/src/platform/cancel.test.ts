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
import { cancelFor, cancelResult, cancelResultText, CANCEL_COPY, PlatformCancelFlow, recipientOf, resetPlatformCancels } from "./cancel";
import { cancelOf, isHeldCancel, isHeldSend, MOVE_COPY, moveErrorText, moveExit, moveExitLine, moveOutcomeCopy } from "./move";

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
  it("tells the user the earlier item can still go through and when to cancel again", () => {
    for (const kind of ["move", "payment"] as const) {
      expect(CANCEL_COPY.restoreRefusal(kind)).toContain(`The earlier ${kind} can still go through.`);
      expect(CANCEL_COPY.restoreRefusal(kind)).toContain("Once it has finished, choose Cancel again.");
    }
  });

  it("a held send: Move to Dash wallet, never a cancel; a cancel row is no send to move", () => {
    const send = held("ps-1", "send", { to: PAYEE });
    expect([isHeldSend(send), cancelOf(send, [send])]).toEqual([true, null]);
    expect(isHeldSend({ ...send, state: "pending" })).toBe(false);
    expect(isHeldSend({ ...send, mayLand: false })).toBe(false);
    expect(isHeldSend({ ...send, cancels: "pm-1", to: OWN })).toBe(false);
  });

  it("a held cancel: Move to Dash wallet, beside its re-send from the item it cancels", () => {
    const cancel = held("pc-1", "send", { to: OWN, cancels: "pm-1" });
    expect([isHeldCancel(cancel), isHeldSend(cancel), cancelOf(cancel, [cancel])]).toEqual([true, false, null]);
    expect(isHeldCancel({ ...cancel, state: "pending" })).toBe(false);
    expect(isHeldCancel({ ...cancel, mayLand: false })).toBe(false);
    expect(isHeldCancel(held("ps-1", "send", { to: PAYEE }))).toBe(false);
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

const ALICE = { identity: "x", username: "alice", displayName: "Alice", avatarUrl: null };
const outcome = { id: "pm-3", credits: "30000000000", expectedDuffs: 29_998_000, to: "y", state: "done", feeCredits: "2000000" } as const;

describe("the move out of a held send", () => {
  const send = held("ps-1", "send", { to: PAYEE });
  const paid = held("ps-1", "send", { contact: ALICE, to: PAYEE });

  it("says the recipient wasn't paid once it went through, and was paid once the payment did", () => {
    expect(moveOutcomeCopy(outcome, false, credits, amount, moveExit([send], "ps-1"))).toEqual({ title: MOVE_COPY.done.title, text: "Moved to Dash wallet. The payment to tdash1kq7r3m…8p0s2a4d6f8h didn't go through, so tdash1kq7r3m…8p0s2a4d6f8h wasn't paid." });
    expect(moveOutcomeCopy({ ...outcome, state: "failed" }, true, credits, amount, moveExit([paid], "ps-1"))).toEqual({ title: MOVE_COPY.failed.title, text: "The payment went through before the move, so Alice was paid. The move wasn't charged." });
  });

  it("says nothing of the payment while neither is known", () => {
    expect(moveOutcomeCopy({ ...outcome, state: "pending" }, false, credits, amount, moveExit([paid], "ps-1")).title).toBe(MOVE_COPY.pending.title);
  });

  it("names the payment, its amount and recipient, on Confirm", () => {
    expect(MOVE_COPY.exitSend.line("0.01 tDASH", "Alice")).toBe(
      "This payment of 0.01 tDASH to Alice may still go through. Moving uses the same Platform funds to send to your Dash wallet instead, so only one of the two can go through. If the move goes through, Alice isn't paid, and the amount, less fees, goes to your Dash wallet.",
    );
  });
});

describe("the move out of a held cancel, or of an item too small to cancel", () => {
  const move = held("pm-1", "moveToCore", { credits: "15000000" });
  const payout = held("cp-1", "contactPayment", { contact: ALICE, credits: "15000000" });
  const cancelOfMove = held("pc-2", "send", { to: OWN, cancels: "pm-1" });
  const cancelOfPayout = held("pc-2", "send", { to: OWN, cancels: "cp-1" });

  it("names what it is the exit of from Activity; a top-up's exit or an unknown row is none", () => {
    expect(moveExit([cancelOfMove, move], "pc-2")).toEqual({ kind: "cancel", of: "move", row: cancelOfMove, held: move });
    expect(moveExit([cancelOfPayout, payout], "pc-2")).toMatchObject({ kind: "cancel", of: "payment", held: payout });
    expect(moveExit([move], "pm-1")).toEqual({ kind: "move", row: move });
    expect(moveExit([payout], "cp-1")).toEqual({ kind: "payment", row: payout });
    expect([moveExit([held("ti-1", "toIdentity")], "ti-1"), moveExit([move], "pm-9"), moveExit([move], null)]).toEqual([null, null, null]);
  });

  it("says on Confirm that only one of the three, or of the two, can go through", () => {
    expect(moveExitLine(moveExit([cancelOfMove, move], "pc-2")!, credits)).toBe(
      "This cancel of 0.4 tDASH, and the move it cancels, may still go through. Moving uses the same Platform funds to send to your Dash wallet instead, so only one of the three can go through. If this move goes through, neither of the others does, and the amount, less fees, goes to your Dash wallet.",
    );
    expect(moveExitLine(moveExit([cancelOfPayout, payout], "pc-2")!, credits)).toContain("and the payment to Alice it cancels, may still go through");
    expect(moveExitLine(moveExit([move], "pm-1")!, credits)).toBe(
      "This move of 0.00015 tDASH may still go through. Moving again uses the same Platform funds, so only one of the two can go through, and either way the amount, less fees, reaches your Dash wallet.",
    );
    expect(moveExitLine(moveExit([payout], "cp-1")!, credits)).toContain("This payment of 0.00015 tDASH to Alice may still go through.");
  });

  it("says which of them went through once Activity shows it", () => {
    const copy = (failed: boolean, rows: PlatformActivity[], id = "pc-2") => moveOutcomeCopy(failed ? { ...outcome, state: "failed" } : outcome, failed, credits, amount, moveExit(rows, id)).text;
    expect(copy(false, [cancelOfMove, move])).toBe("Moved to Dash wallet. The cancel and the earlier move didn't go through.");
    expect(copy(false, [cancelOfPayout, payout])).toBe("Moved to Dash wallet. The cancel and the payment to Alice didn't go through, so Alice wasn't paid.");
    const done = { state: "done" as const, mayLand: false };
    const lost = { state: "failed" as const, mayLand: false };
    expect(copy(true, [{ ...cancelOfMove, ...done }, { ...move, ...lost }])).toBe("The cancel went through before the move, so the amount is in your Platform balance, minus the cancel's fee. The move wasn't charged.");
    expect(copy(true, [{ ...cancelOfPayout, ...done }, { ...payout, ...lost }])).toBe("The cancel went through before the move, so Alice wasn't paid, and the amount is in your Platform balance, minus the cancel's fee. The move wasn't charged.");
    expect(copy(true, [{ ...cancelOfMove, ...lost }, { ...move, ...done }])).toBe("The earlier move went through first, so the amount is in your Dash wallet. This move wasn't charged.");
    expect(copy(true, [{ ...cancelOfPayout, ...lost }, { ...payout, ...done }])).toBe("The payment went through before the move, so Alice was paid. The move wasn't charged.");
    // An item too small to cancel: a move's exit is done as any move; lost, the earlier one went through.
    expect(copy(false, [move], "pm-1")).toBe(moveOutcomeCopy(outcome, false, credits, amount).text);
    expect(copy(true, [{ ...move, ...done }], "pm-1")).toBe("The earlier move went through first, so the amount is in your Dash wallet. This move wasn't charged.");
    expect(copy(false, [payout], "cp-1")).toBe("Moved to Dash wallet. The payment to Alice didn't go through, so Alice wasn't paid.");
    expect(copy(true, [{ ...payout, ...done }], "cp-1")).toBe("The payment went through before the move, so Alice was paid. The move wasn't charged.");
    // None of them went through: something else spent the funds, as any failed move says.
    for (const [rows, id] of [[[{ ...cancelOfMove, ...lost }, { ...move, ...lost }], "pc-2"], [[{ ...cancelOfPayout, ...lost }, { ...payout, ...lost }], "pc-2"], [[{ ...move, ...lost }], "pm-1"], [[{ ...payout, ...lost }], "cp-1"]] as const) {
      expect(copy(true, [...rows], id)).toBe(MOVE_COPY.failed.text);
    }
  });

  it("a pre-check refusal of the move says the earlier item may still go through", () => {
    const refused = appError("credits.preflight_refused");
    const rules = appError("credits.rules_unsupported");
    for (const [rows, id] of [[[cancelOfMove, move], "pc-2"], [[move], "pm-1"], [[payout], "cp-1"]] as const) {
      expect(moveErrorText(refused, credits, rows, id)).toContain("Your earlier payment, move or cancel may still go through, so check Activity");
      expect(moveErrorText(rules, credits, rows, id)).toContain("Your earlier payment, move or cancel may still go through. Update Dash Wallet");
    }
  });

  it("a cancel too small to make says so, and offers the move", () => {
    expect(CANCEL_COPY.tooSmall("move", "0.00015 tDASH", "")).toBe(
      "This move of 0.00015 tDASH is too small to cancel. You can move the same funds to your Dash wallet instead. Only one of the two can go through, and either way the amount, less fees, reaches your Dash wallet.",
    );
    expect(CANCEL_COPY.tooSmall("payment", "0.00015 tDASH", "Alice")).toContain("if the move does, Alice isn't paid");
  });
});

describe("the cancel's outcome", () => {
  const settled = { mayLand: false } as const;
  const won = { state: "done", mayLand: false } as const;
  const failed: PlatformSendOutcome = { ...OUT, state: "failed", mayLand: false };
  const move = held("pm-1", "moveToCore");
  const payout = held("cp-1", "contactPayment", { to: PAYEE });
  const recipient = recipientOf(payout);
  const cancelRow = (of: string, extra: Partial<PlatformActivity> = {}) => held("pc-2", "send", { to: OWN, cancels: of, ...extra });
  const exitRow = (of: string, extra: Partial<PlatformActivity> = {}) => held(`exit-of-${of}-at-7-1`, "moveToCore", extra);

  it("is done once a cancel of the item went through, and waits while any of them may still go through", () => {
    expect(cancelResult(OUT, [cancelRow("pm-1", won), { ...move, ...settled }], "pm-1")).toBe("done");
    // Its identical re-send went through in its place.
    expect(cancelResult(failed, [{ ...held("pc-3", "send", { to: OWN, cancels: "pm-1" }), ...won }, cancelRow("pm-1", { state: "replaced", mayLand: false }), { ...move, ...settled }], "pm-1")).toBe("done");
    expect(cancelResult(OUT, [cancelRow("pm-1"), move], "pm-1")).toBe("pending");
    expect(cancelResult(failed, [cancelRow("pm-1", settled), { ...exitRow("pc-2"), state: "pending" }, { ...move, ...settled }], "pm-1")).toBe("pending");
    expect(cancelResult({ ...OUT, state: "pending", mayLand: true }, undefined, "pm-1")).toBe("pending");
    // Before Activity shows it, a failed cancel says nothing of which went through.
    expect(cancelResult(failed, undefined, "pm-1")).toBe("pending");
    // Activity without the cancel's row yet: what the call returned.
    expect(cancelResult(OUT, [move], "pm-1")).toBe("done");
    expect(cancelResult({ ...OUT, state: "pending", mayLand: true }, [{ ...move, ...settled }], "pm-1")).toBe("pending");
    // A group settled with no evidence of which went through is no proof the cancel did.
    expect(cancelResult(OUT, [cancelRow("pm-1", { ...won, winnerKnown: false }), { ...move, ...settled, winnerKnown: false }], "pm-1")).toBe("unknown");
  });

  it("names no winner of a group settled without evidence of which went through", () => {
    expect(cancelResult(failed, [cancelRow("cp-1", { state: "replaced", mayLand: false, winnerKnown: false }), { ...payout, ...won, winnerKnown: false }], "cp-1")).toBe("unknown");
    expect(cancelResult(failed, [{ ...exitRow("pc-2"), ...won, winnerKnown: false }, cancelRow("cp-1", { ...settled, winnerKnown: false }), { ...payout, ...settled, winnerKnown: false }], "cp-1")).toBe("unknown");
    // A row whose id names a link it does not carry is no exit (the engine reads it as a contact payout).
    expect(cancelResult(failed, [{ ...exitRow("pc-2"), kind: "contactPayment", ...won }, cancelRow("cp-1", settled), { ...payout, ...settled }], "cp-1")).toBe("unknown");
  });

  it("is the exit's once Move to Dash wallet went through: the contact wasn't paid", () => {
    const rows = [{ ...exitRow("pc-2"), ...won }, cancelRow("cp-1", settled), { ...payout, ...settled }];
    expect(cancelResult(failed, rows, "cp-1")).toBe("exited");
    expect(cancelResultText("exited", "payment", recipient)).toBe(`Moved to Dash wallet. The cancel and the payment to ${recipient} didn't go through, so ${recipient} wasn't paid.`);
    expect(cancelResultText(cancelResult(failed, rows, "cp-1"), "payment", recipient)).not.toContain(`${recipient} was paid`);
    // The exit named on the cancel's identical re-send.
    const resent = [{ ...exitRow("pc-3"), ...won }, { ...held("pc-3", "send", { to: OWN, cancels: "cp-1" }), ...settled }, cancelRow("cp-1", settled), { ...payout, ...settled }];
    expect(cancelResult(failed, resent, "cp-1")).toBe("exited");
    // An exit of another record says nothing of this one.
    expect(cancelResult(failed, [{ ...exitRow("pc-9"), ...won }, cancelRow("cp-1", settled), { ...payout, ...settled }], "cp-1")).toBe("unknown");
  });

  it("is the exit's once Move to Dash wallet went through in place of a cancelled move", () => {
    const rows = [{ ...exitRow("pc-2"), ...won }, cancelRow("pm-1", settled), { ...move, ...settled }];
    expect(cancelResult(failed, rows, "pm-1")).toBe("exited");
    expect(cancelResultText("exited", "move", "")).toBe("Moved to Dash wallet. The cancel and the earlier move didn't go through.");
  });

  it("says none of them is known to have gone through once all failed", () => {
    for (const rows of [
      [cancelRow("cp-1", settled), { ...payout, ...settled }],
      [{ ...exitRow("pc-2"), ...settled }, cancelRow("cp-1", settled), { ...payout, ...settled }],
    ]) {
      expect(cancelResult(failed, rows, "cp-1")).toBe("unknown");
    }
    expect(cancelResultText("unknown", "payment", recipient)).toBe(
      "This payment and another payment or move from the same Platform addresses couldn't both go through, and only one did. Dash Wallet can't tell which. Before paying again, check whether the recipient was paid.",
    );
    expect(cancelResultText("unknown", "payment", recipient)).not.toContain(`${recipient} was paid`);
    expect(cancelResult(failed, [cancelRow("pm-1", settled), { ...move, ...settled }], "pm-1")).toBe("unknown");
    expect(cancelResultText("unknown", "move", "")).toContain("Check your Platform balance and Dash wallet before moving again.");
  });

  it("is lost only once the cancelled payment or move went through: then the contact was paid", () => {
    expect(cancelResult(failed, [cancelRow("cp-1", settled), { ...payout, ...won }], "cp-1")).toBe("lost");
    expect(CANCEL_COPY.lost("payment", recipient)).toBe(`The payment went through before the cancel, so ${recipient} was paid. The cancel wasn't charged.`);
    expect(cancelResult(failed, [cancelRow("pm-1", settled), { ...move, ...won }], "pm-1")).toBe("lost");
  });

  it("words each outcome", () => {
    expect(cancelResultText("lost", "payment", recipient)).toBe(CANCEL_COPY.lost("payment", recipient));
    expect(cancelResultText("done", "move", "")).toBe("Move cancelled. The amount is in your Platform balance, minus the fee.");
    expect(cancelResultText("pending", "payment", recipient)).toBe(CANCEL_COPY.pending);
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
