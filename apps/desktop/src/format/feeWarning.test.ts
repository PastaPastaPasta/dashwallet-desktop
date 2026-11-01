import { describe, expect, it } from "vitest";
import { feeWarning, feeWarningText } from "./feeWarning";

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
