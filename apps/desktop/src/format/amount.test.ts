import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { compact, floorWithUnit, format, formatAmount, formatWithUnit, masked, trimDecimals, type Unit } from "./amount";

// The dash-qt oracle vectors rust/crates/dw-units is tested against.
const vectors = JSON.parse(readFileSync(resolve(__dirname, "../../../../testdata/amount_format.json"), "utf8"));
const unit = (u: string) => u as Unit;

describe("dash-qt golden vectors (testdata/amount_format.json)", () => {
  it("format", () => {
    for (const v of vectors.format) {
      expect(format(unit(v.unit), v.amount, v.plus, v.separators, v.justify), JSON.stringify(v)).toBe(v.out);
    }
    expect(vectors.format.length).toBe(1440);
  });

  it("format_with_unit", () => {
    for (const v of vectors.format_with_unit) {
      expect(formatWithUnit(unit(v.unit), v.amount, v.plus, v.separators, v.network), JSON.stringify(v)).toBe(v.out);
    }
  });

  it("floor_with_unit", () => {
    for (const v of vectors.floor_with_unit) {
      expect(floorWithUnit(unit(v.unit), v.amount, v.plus, v.separators, v.digits, v.network), JSON.stringify(v)).toBe(v.out);
    }
  });

  it("format_amount", () => {
    for (const v of vectors.format_amount) {
      expect(formatAmount(unit(v.unit), v.amount, v.signed, v.truncate, v.network), JSON.stringify(v)).toBe(v.out);
    }
  });
});

describe("UX-SPEC §5.2 styles", () => {
  it("compact keeps every significant duff and at least two decimals", () => {
    expect(compact(20_000_000, false)).toBe("0.20");
    expect(compact(3_039_698_287, false)).toBe("30.39698287");
    expect(compact(-50_000_226, true)).toBe("-0.50000226");
    expect(compact(125_000_000, true)).toBe("+1.25");
    expect(compact(0, true)).toBe("0.00");
    expect(compact(1_234_567_800_000_000, false)).toBe("12 345 678.00");
  });

  it("trims only after the decimal mark", () => {
    expect(trimDecimals("100")).toBe("100");
    expect(trimDecimals("1.10000000")).toBe("1.10");
  });

  it("masks with the floored width", () => {
    expect(masked()).toBe("#.##");
  });
});
