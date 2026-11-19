// Max on Send from Shielded: two quotes of a send to the address typed, the second of the balance less
// the first's fee, against a fee that grows with the notes the amount takes (as the engine prices it).

import { afterEach, describe, expect, it, vi } from "vitest";
import type { ShieldedSpendQuote } from "../api/generated/ShieldedSpendQuote";
import { SHIELDED_CODES } from "../api/pending";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { fakeVault } from "../tools/testApi";
import { resetShielded, ShieldedFlow } from "./flow";
import { sendMax, sendMaxLine } from "./sendMax";

const TO = "tdash1zq0k8y";
const NOTES = [100_000_000_000n, 20_000_000_000n, 5_000_000_000n];
const BALANCE = NOTES.reduce((a, b) => a + b);
const fee = (notes: number) => 10_000_000n * BigInt(Math.max(notes, 2));

/** The engine's `send_quote` over `NOTES`: largest first, priced on the notes taken. */
function quote(credits: bigint): ShieldedSpendQuote {
  let sum = 0n;
  for (let n = 1; n <= NOTES.length; n++) {
    sum += NOTES[n - 1];
    if (sum >= credits + fee(n)) {
      const total = credits + fee(n);
      return { credits: String(credits), feeCredits: String(fee(n)), totalCredits: String(total), expectedDuffs: null, grant: { maxDuffs: 0, maxCredits: String(total) } };
    }
  }
  throw appError(SHIELDED_CODES.insufficientFunds, { kind: "credits", needed: String(credits + fee(NOTES.length)), available: String(BALANCE) });
}

function setup(answer: (to: string, credits: string) => Promise<ShieldedSpendQuote> = async (_to, c) => quote(BigInt(c))) {
  const flow = new ShieldedFlow(fakeVault({ encrypted: false }).api as never, WALLET_A);
  const shieldedSendQuote = vi.fn((_w: string, to: string, credits: string) => answer(to, credits));
  const line = sendMaxLine({ shieldedSendQuote }, flow);
  return { flow, line, shieldedSendQuote };
}

afterEach(() => resetShielded());

describe("Max on Send from Shielded", () => {
  it("is the balance less the engine's fee, re-quoted once to the address typed", async () => {
    const { flow, line, shieldedSendQuote } = setup();
    flow.setTo(`  ${TO} `);
    const probe = Number(BALANCE / 1000n);
    expect(await sendMax(line, probe)).toBe(124_970_000);
    expect(shieldedSendQuote.mock.calls).toEqual([
      [WALLET_A, TO, "125000000000"],
      [WALLET_A, TO, "124970000000"],
    ]);
    // The amount filled is the one the second quote took: total = balance, never above it.
    expect(line.state).toMatchObject({ kind: "quoted", amount: 124_970_000, quote: { totalCredits: "125000000000", grant: { maxCredits: "125000000000" } } });
  });

  it("fills nothing when the balance does not cover the fee", async () => {
    const { flow, line, shieldedSendQuote } = setup(async (_to, c) => {
      throw appError(SHIELDED_CODES.insufficientFunds, { kind: "credits", needed: String(BigInt(c) + 20_000_000n), available: "15000000" });
    });
    flow.setTo(TO);
    expect(await sendMax(line, 15_000)).toBeNull();
    expect(shieldedSendQuote).toHaveBeenCalledTimes(1);
    expect(line.state).toMatchObject({ kind: "refused", amount: 15_000, error: { code: SHIELDED_CODES.insufficientFunds } });
  });

  it("fills nothing for an address the engine refuses", async () => {
    const { flow, line } = setup(async () => {
      throw appError(SHIELDED_CODES.invalidAddress, { kind: "reason", reason: "not a shielded address" });
    });
    flow.setTo("not-an-address");
    expect(await sendMax(line, 125_000)).toBeNull();
    expect(line.state).toMatchObject({ kind: "refused", error: { code: SHIELDED_CODES.invalidAddress } });
  });

  it("drops its answer once the amount or address is changed under it", async () => {
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const { flow, line } = setup(async (_to, c) => (await gate, quote(BigInt(c))));
    flow.setTo(TO);
    const got = sendMax(line, Number(BALANCE / 1000n));
    line.set(null);
    release();
    expect(await got).toBeNull();
    expect(line.state).toEqual({ kind: "idle" });
  });
});
