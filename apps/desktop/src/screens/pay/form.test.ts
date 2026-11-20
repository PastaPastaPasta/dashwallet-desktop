import { afterEach, describe, expect, it } from "vitest";
import { acknowledge, beginPayment, beginRetry, draftClosed, keptPayment, outliving, payAttempt, payFormKey, resetKeptPayments } from "./form";
import type { Outcome } from "./model";

const KEY = payFormKey("w");
const SENDING = { amount: 5, fee: 1 };
const unknown: Outcome = { kind: "unknown", text: "No answer.", retry: true, txid: "ab".repeat(32), draft: 1, prepared: 2, amount: 5, fee: 1 };
const sent: Outcome = { kind: "sent", txid: "cd".repeat(32), amount: 5, fee: 1 };

afterEach(resetKeptPayments);

describe("a payment that outlives its form", () => {
  it("keeps each form's payment apart", () => {
    expect(new Set([payFormKey("w"), payFormKey("w", "c"), payFormKey("w", "d"), payFormKey("v")]).size).toBe(4);
  });

  it("offers no retry once its draft closed with the page, and keeps everything else", () => {
    expect(outliving(unknown)).toEqual({ ...unknown, retry: false });
    expect(outliving(sent)).toBe(sent);
  });

  it("is kept from Send on, before anything was signed", () => {
    const a = beginPayment(KEY, SENDING);
    expect(keptPayment(KEY)).toEqual({ kind: "preparing", attempt: a, sending: SENDING });
    payAttempt(KEY, a, () => true).release();
    expect(keptPayment(KEY)).toBeNull();
  });

  it("an older payment's late refusal leaves a newer payment's result alone", () => {
    const a = payAttempt(KEY, beginPayment(KEY, SENDING), () => false);
    const bId = beginPayment(KEY, SENDING);
    const b = payAttempt(KEY, bId, () => true);
    b.broadcasting(SENDING);
    b.outcome(sent);
    a.outcome({ kind: "failed", text: "The payment form was closed." });
    a.broadcasting(SENDING);
    a.release();
    expect(keptPayment(KEY)).toEqual({ kind: "result", attempt: bId, outcome: sent, retrying: false });
    expect(acknowledge(KEY, bId - 1)).toBeNull();
    expect(acknowledge(KEY, bId)).toEqual(sent);
    expect(keptPayment(KEY)).toBeNull();
  });

  it("an unknown result kept just before its page went loses its retry, and a retry under way keeps it busy until it answers", () => {
    const id = beginPayment(KEY, SENDING);
    payAttempt(KEY, id, () => true).outcome(unknown);
    expect(beginRetry(KEY)).toBe(id);
    expect(beginRetry(KEY)).toBeNull();
    draftClosed(KEY);
    expect(keptPayment(KEY)).toMatchObject({ kind: "result", retrying: true, outcome: { kind: "unknown", retry: false } });
    expect(acknowledge(KEY, id)).toBeNull();
    payAttempt(KEY, id, () => false).outcome(unknown);
    expect(keptPayment(KEY)).toMatchObject({ kind: "result", retrying: false, outcome: { kind: "unknown", retry: false } });
    expect(beginRetry(KEY)).toBeNull();
    expect(acknowledge(KEY, id)).toMatchObject({ kind: "unknown", retry: false });
  });
});
