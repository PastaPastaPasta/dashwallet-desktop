// Pay from Platform: the amount the form asks for, the send under the quote's platformOp grant,
// and the refusals' texts.

import { afterEach, describe, expect, it, vi } from "vitest";
import type { PlatformSendOutcome } from "../api/generated/PlatformSendOutcome";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { fakeVault } from "../tools/testApi";
import { onMoveSettled } from "./move";
import { PlatformSendFlow, recipientText, resetPlatformSends, sendAmount, sendCanRetry, sendErrorText, sendFor } from "./send";

const TO = "tdash1kq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h";
const GRANT = { maxDuffs: 0, maxCredits: "1019500000" };
const OUTCOME: PlatformSendOutcome = { id: "ps-1", credits: "1006500000", to: TO, state: "done", feeCredits: "6500000" };
const credits = (c: string) => `${Number(BigInt(c) / 1000n) / 1e8} tDASH`;

function setup() {
  const vault = fakeVault({ encrypted: false });
  const calls: string[] = [];
  let answer: ((o: PlatformSendOutcome) => void) | null = null;
  let refuse: ((e: unknown) => void) | null = null;
  const api = {
    ...vault.api,
    platformSend: vi.fn(
      (_walletId: string, to: string, amount: { kind: string; credits?: string }, grant: string) =>
        new Promise<PlatformSendOutcome>((resolve, reject) => {
          calls.push(`send ${to} ${amount.kind === "all" ? "all" : amount.credits} ${grant}`);
          answer = resolve;
          refuse = reject;
        }),
    ),
  };
  const flow = new PlatformSendFlow(api, WALLET_A);
  return { flow, vault, calls, answer: (o: PlatformSendOutcome) => answer!(o), refuse: (e: unknown) => refuse!(e) };
}

afterEach(() => resetPlatformSends());

describe("Pay from Platform", () => {
  it("asks for Max as all, and an amount as its credits", () => {
    expect(sendAmount(true, undefined)).toEqual({ kind: "all" });
    expect(sendAmount(false, 1_000_000)).toEqual({ kind: "credits", credits: "1000000000" });
    expect([sendAmount(false, 0), sendAmount(false, null), sendAmount(false, undefined)]).toEqual([null, null, null]);
  });

  it("sends under the quote's platformOp grant, one call at a time, and re-reads the balances after", async () => {
    const { flow, vault, calls, answer } = setup();
    const settled = vi.fn();
    const stop = onMoveSettled(settled);
    const going = flow.send(GRANT, TO, { kind: "credits", credits: "1000000000" });
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    expect(vault.calls.authorize[0]).toMatchObject({ purpose: { kind: "platformOp", maxDuffs: 0, maxCredits: "1019500000" }, walletId: WALLET_A });
    await flow.send(GRANT, TO, { kind: "all" });
    expect(calls).toEqual([`send ${TO} 1000000000 grant-1`]);
    answer(OUTCOME);
    await going;
    await Promise.resolve();
    expect([flow.step, flow.outcome, settled.mock.calls.length]).toEqual(["done", OUTCOME, 1]);
    flow.reset();
    expect([flow.step, flow.outcome]).toEqual(["confirm", null]);
    stop();
  });

  it("keeps a refusal to show, and offers Back only when a new quote may pass", async () => {
    const { flow, refuse } = setup();
    const going = flow.send(GRANT, TO, { kind: "all" });
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    refuse(appError("credits.move_pending", { kind: "artifact", artifact: "pm-1" }));
    await going;
    expect(flow.step).toBe("refused");
    expect(sendCanRetry(flow.error)).toBe(false);
    expect(sendCanRetry(appError("platform.grant_exceeded"))).toBe(true);
    expect(sendCanRetry(appError("platform.insufficient_credits"))).toBe(true);
  });

  it("keeps one flow per wallet", () => {
    const api = setup().flow;
    expect(api).toBeInstanceOf(PlatformSendFlow);
    const vault = fakeVault({ encrypted: false });
    const a = sendFor({ ...vault.api, platformSend: vi.fn() }, WALLET_A);
    expect(sendFor({ ...vault.api, platformSend: vi.fn() }, WALLET_A)).toBe(a);
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

  it("names the minimum, the shortfall and a held send", () => {
    expect(sendErrorText(appError("credits.send_out_of_range", { kind: "creditMinimum", min: "500000" }), credits)).toBe(
      "Dash Platform cannot send this amount: the payment, and what it takes from each of your Platform addresses, must be at least 0.000005 tDASH. Try another amount, or Max.",
    );
    expect(sendErrorText(appError("platform.insufficient_credits", { kind: "credits", needed: "2000000000", available: "1000000000" }), credits)).toBe(
      "Not enough on your Platform addresses: 0.02 tDASH needed with the fee, 0.01 tDASH available.",
    );
    expect(sendErrorText(appError("credits.move_pending", { kind: "artifact", artifact: "ps-1" }), credits)).toMatch(/only that same payment \(same address and amount\)/);
    expect(sendErrorText(appError("platform.broadcast_unknown", { kind: "artifact", artifact: "x" }), credits)).toMatch(/may have been sent/);
  });
});
