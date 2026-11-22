import { describe, expect, it } from "vitest";
import type { AppError } from "../api/generated/AppError";
import { moveErrorText } from "../coinjoin/move";
import { failureText as registrationText } from "../dashpay/status";
import { topUpErrorText } from "../dashpay/topup";
import { failureText } from "../onboarding/model";
import { failureText as payFailureText } from "../screens/pay/model";
import { shieldedErrorText } from "../shielded/flow";
import { psbtErrorText } from "../tools/psbt";
import { repairErrorText } from "../tools/repair";

const COPY = "Dash Wallet can't read some of this wallet's payment records, so sending is paused. Try again in a moment, or restart Dash Wallet.";
const refusal: AppError = { code: "evidence_unreadable", message: "some of this wallet's payment records cannot be read; sending is paused", data: null };
const amount = (n: number) => `${n}`;

describe("a spend refused for unreadable payment records", () => {
  it("says why on every screen that can start one", () => {
    expect(failureText(refusal)).toBe(COPY);
    expect(payFailureText(refusal, "test")).toBe(COPY);
    expect(psbtErrorText(refusal)).toBe(COPY);
    expect(repairErrorText(refusal)).toBe(COPY);
    expect(moveErrorText(refusal)).toBe(COPY);
    expect(topUpErrorText(refusal, amount)).toBe(COPY);
    expect(shieldedErrorText(refusal, String, amount)).toBe(COPY);
    expect(registrationText({ phase: "failed", code: "evidence_unreadable", retryable: true, needed: null, available: null }, amount)).toBe(COPY);
  });
});
