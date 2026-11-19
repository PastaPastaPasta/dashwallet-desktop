// Max on Send from Shielded, against quotes answered as the engine answers them: a refusal names the
// amount plus the fee the note selection had reached (first the two-action minimum), so Max derives
// again from each refusal until a quote takes the amount.

import { afterEach, describe, expect, it, vi } from "vitest";
import type { ShieldedSpendQuote } from "../api/generated/ShieldedSpendQuote";
import { SHIELDED_CODES } from "../api/pending";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { fakeVault } from "../tools/testApi";
import { resetShielded, ShieldedFlow } from "./flow";
import { sendMax, sendMaxLine } from "./sendMax";
import { engineSpendQuote, NOTE_SETS, NOTES_BALANCE } from "./testNotes";

const TO = "tdash1zq0k8y";
const PROBE = Number(NOTES_BALANCE / 1000n);

function setup(answer: (to: string, credits: string) => Promise<ShieldedSpendQuote>) {
  const flow = new ShieldedFlow(fakeVault({ encrypted: false }).api as never, WALLET_A);
  const shieldedSendQuote = vi.fn((_w: string, to: string, credits: string) => answer(to, credits));
  const line = sendMaxLine({ shieldedSendQuote }, flow);
  return { flow, line, shieldedSendQuote };
}

const overNotes = (n: keyof typeof NOTE_SETS) => setup(async (_to, c) => engineSpendQuote(NOTE_SETS[n], c));

afterEach(() => resetShielded());

describe("Max on Send from Shielded", () => {
  it.each([
    [2, 124_980_000, ["125000000000", "124980000000"]],
    [3, 124_970_000, ["125000000000", "124980000000", "124970000000"]],
    [5, 124_950_000, ["125000000000", "124980000000", "124950000000"]],
  ] as const)("over %i notes is the balance less the fee of every note it takes, to the address typed", async (n, max, asked) => {
    const { flow, line, shieldedSendQuote } = overNotes(n);
    flow.setTo(`  ${TO} `);
    expect(await sendMax(line, PROBE)).toBe(max);
    expect(shieldedSendQuote.mock.calls).toEqual(asked.map((c) => [WALLET_A, TO, c]));
    // The amount filled is the one the last quote took: amount + fee is the balance, never above it.
    expect(line.state).toMatchObject({ kind: "quoted", amount: max, quote: { totalCredits: String(NOTES_BALANCE), grant: { maxCredits: String(NOTES_BALANCE) } } });
  });

  it("stops after four quotes, showing the last refusal", async () => {
    let needed = 125_020_000_000n;
    const { flow, line, shieldedSendQuote } = setup(async (_to, c) => {
      needed += 10_000_000n;
      throw appError(SHIELDED_CODES.insufficientFunds, { kind: "credits", needed: String(BigInt(c) + needed - NOTES_BALANCE), available: String(NOTES_BALANCE) });
    });
    flow.setTo(TO);
    expect(await sendMax(line, PROBE)).toBeNull();
    expect(shieldedSendQuote).toHaveBeenCalledTimes(4);
    expect(line.state).toMatchObject({ kind: "refused", error: { code: SHIELDED_CODES.insufficientFunds } });
  });

  it("fills nothing when the balance does not cover the fee", async () => {
    const { flow, line, shieldedSendQuote } = setup(async (_to, c) => engineSpendQuote([15_000_000n], c));
    flow.setTo(TO);
    expect(await sendMax(line, 15_000)).toBeNull();
    expect(shieldedSendQuote).toHaveBeenCalledTimes(1);
    expect(line.state).toMatchObject({ kind: "refused", amount: 15_000, error: { code: SHIELDED_CODES.insufficientFunds } });
  });

  it("fills nothing for an address the engine refuses", async () => {
    const { flow, line, shieldedSendQuote } = setup(async () => {
      throw appError(SHIELDED_CODES.invalidAddress, { kind: "reason", reason: "not a shielded address" });
    });
    flow.setTo("not-an-address");
    expect(await sendMax(line, 125_000)).toBeNull();
    expect(shieldedSendQuote).toHaveBeenCalledTimes(1);
    expect(line.state).toMatchObject({ kind: "refused", error: { code: SHIELDED_CODES.invalidAddress } });
  });

  it("drops its answer once the amount or address is changed under it", async () => {
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const { flow, line } = setup(async (_to, c) => (await gate, engineSpendQuote(NOTE_SETS[3], c)));
    flow.setTo(TO);
    const got = sendMax(line, PROBE);
    line.set(null);
    release();
    expect(await got).toBeNull();
    expect(line.state).toEqual({ kind: "idle" });
  });
});
