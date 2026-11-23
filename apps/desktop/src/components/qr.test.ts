import { describe, expect, it } from "vitest";
import { qrPath } from "./qr";

describe("qrPath", () => {
  it("draws each dark module once, offset by the 4-module quiet zone", () => {
    expect(qrPath({ size: 2, modules: [true, false, false, true] })).toBe("M4 4h1v1h-1zM5 5h1v1h-1z");
    expect(qrPath({ size: 1, modules: [false] })).toBe("");
  });
});
