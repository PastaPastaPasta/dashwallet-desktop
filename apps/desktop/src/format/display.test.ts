// The display settings applied to amounts (U-04 D4): QT-139/QT-152's unit everywhere at once,
// QT-020/QT-036's decimal digits on the hero and breakdown.

import { describe, expect, it } from "vitest";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { THIN_SP as T } from "./amount";
import { display } from "./display";

const TESTNET: NetworkInfo = { kind: "testnet", dirName: "testnet", unitName: "tDASH" };
const MAINNET: NetworkInfo = { kind: "mainnet", dirName: "mainnet", unitName: "DASH" };
const AMOUNT = 125_000_000;

describe("display unit", () => {
  it("QT139_QT152_everyAmountIsInTheDisplayUnit", () => {
    const shown = (unit: "dash" | "milliDash" | "microDash" | "duffs") => display({ unit, decimalDigits: 2 }, TESTNET);
    expect([shown("dash").full(AMOUNT), shown("milliDash").full(AMOUNT), shown("microDash").full(AMOUNT), shown("duffs").full(AMOUNT)]).toEqual([
      "1.25000000",
      "1250.00000",
      `1${T}250${T}000.00`,
      `125${T}000${T}000`,
    ]);
    expect(shown("milliDash").compact(-AMOUNT, true)).toBe("-1250.00");
    expect(["dash", "milliDash", "microDash", "duffs"].map((u) => shown(u as "dash").name)).toEqual(["tDASH", "mtDASH", "μtDASH", "tduffs"]);
    expect(display({ unit: "milliDash", decimalDigits: 2 }, MAINNET).withUnit(AMOUNT)).toBe("1250.00000 mDASH");
  });

  it("QT020_QT036_heroAndBreakdownFloorToTheDecimalDigits", () => {
    expect(display({ unit: "dash", decimalDigits: 4 }, TESTNET).floored(129_999_999)).toBe("1.2999");
    expect(display({ unit: "dash", decimalDigits: 4 }, TESTNET).masked()).toBe("#.####");
    // A unit with fewer decimals than the setting shows all of them.
    expect(display({ unit: "microDash", decimalDigits: 8 }, TESTNET).floored(129_999_999)).toBe(`1${T}299${T}999.99`);
    expect(display({ unit: "duffs", decimalDigits: 2 }, TESTNET).masked()).toBe("#");
  });

  it("amount fields name the unit they parse", () => {
    expect(display({ unit: "dash", decimalDigits: 2 }, TESTNET)).toMatchObject({ placeholder: "0.00", invalidAmount: "Enter an amount in DASH, for example 0.25." });
    expect(display({ unit: "duffs", decimalDigits: 2 }, TESTNET)).toMatchObject({ placeholder: "0", invalidAmount: "Enter an amount in duffs, for example 25000." });
  });
});
