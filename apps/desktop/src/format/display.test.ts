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

  it("DEC1058_balancesNeverTruncate", () => {
    // Home's hero, rows and cards use `compact`: the exact amount, trailing zeros trimmed to two decimals.
    const d = display({ unit: "dash", decimalDigits: 2 }, MAINNET);
    expect([999_819, 1_000_000, 100_000_000, 1_234_567_890, 0].map((n) => d.compact(n))).toEqual(["0.00999819", "0.01", "1.00", "12.3456789", "0.00"]);
    expect(d.compact(129_999_999)).toBe("1.29999999");
    expect(display({ unit: "milliDash", decimalDigits: 2 }, MAINNET).compact(999_819)).toBe("9.99819");
    expect(display({ unit: "dash", decimalDigits: 4 }, TESTNET).masked()).toBe("#.####");
    expect(display({ unit: "duffs", decimalDigits: 2 }, TESTNET).masked()).toBe("#");
  });

  it("amount fields name the unit they parse", () => {
    expect(display({ unit: "dash", decimalDigits: 2 }, TESTNET)).toMatchObject({ placeholder: "0.00", invalidAmount: "Enter an amount in DASH, for example 0.25." });
    expect(display({ unit: "duffs", decimalDigits: 2 }, TESTNET)).toMatchObject({ placeholder: "0", invalidAmount: "Enter an amount in duffs, for example 25000." });
  });
});
