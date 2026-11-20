import { describe, expect, it } from "vitest";
import { outliving, payFormKey } from "./form";
import type { Outcome } from "./model";

describe("a payment that outlives its form", () => {
  it("keeps each form's payment apart", () => {
    expect(new Set([payFormKey("w"), payFormKey("w", "c"), payFormKey("w", "d"), payFormKey("v")]).size).toBe(4);
  });

  it("offers no retry once its draft closed with the page, and keeps everything else", () => {
    const unknown: Outcome = { kind: "unknown", text: "No answer.", retry: true, txid: "ab".repeat(32), draft: 1, prepared: 2, amount: 5, fee: 1 };
    expect(outliving(unknown)).toEqual({ ...unknown, retry: false });
    const sent: Outcome = { kind: "sent", txid: "cd".repeat(32), amount: 5, fee: 1 };
    expect(outliving(sent)).toBe(sent);
  });
});
