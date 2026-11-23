import { describe, expect, it } from "vitest";
import { feeWarning, feeWarningText, payFeeWarning } from "./feeWarning";

describe("DEC1058_feeWarning", () => {
  it("is silent below a tenth and says so from a tenth", () => {
    expect(feeWarning(99n, 1000n)).toBeNull();
    expect(feeWarning(100n, 1000n)).toEqual({ percent: 10, most: false });
    expect(feeWarning(500n, 1000n)).toEqual({ percent: 50, most: false });
    expect(feeWarning(501n, 1000n)).toEqual({ percent: 50, most: true });
    expect(feeWarning(2_100_000n, 1_000_000n)).toEqual({ percent: 210, most: true });
  });
  it("has nothing to say about no fee or no amount", () => {
    expect(feeWarning(0n, 1000n)).toBeNull();
    expect(feeWarning(10n, 0n)).toBeNull();
  });
  it("words it", () => {
    expect(feeWarningText({ percent: 12, most: false })).toBe("Fees are 12% of this amount.");
    expect(feeWarningText({ percent: 210, most: true })).toBe("Most of this goes to fees.");
  });
});

describe("DEC1058_payFeeWarning", () => {
  it("compares the fee with what the payee receives", () => {
    // 226 duffs on 1,000: 22%. On 774 (the fee subtracted from 1,000): 29%.
    expect(payFeeWarning(226, 1000)).toEqual({ percent: 22, most: false });
    expect(payFeeWarning(226, 774)).toEqual({ percent: 29, most: false });
    // Below a tenth, nothing; above a half, "most".
    expect(payFeeWarning(226, 2260)).toEqual({ percent: 10, most: false });
    expect(payFeeWarning(226, 2261)).toBeNull();
    expect(payFeeWarning(226, 400)).toEqual({ percent: 56, most: true });
    // A whole-wallet payment with the fee taken from it, and nothing to pay: no warning.
    expect(payFeeWarning(636, 3_019_697_651)).toBeNull();
    expect(payFeeWarning(226, 0)).toBeNull();
  });
});
