import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { duffsToMinor, fiatText, formatMinor, isLocalCurrency, localeCurrency, minorToDuffs, minorToPlain, minorUnits, parseFiat, parsePrice } from "./fiat";

const vectors = JSON.parse(readFileSync(resolve(__dirname, "../../../../testdata/fiat_convert.json"), "utf8")) as {
  duffsToMinor: { duffs: number; price: string; currency: string; minor: string }[];
  minorToDuffs: { minor: string; price: string; currency: string; duffs: string }[];
};

describe("the pinned rounding (testdata/fiat_convert.json)", () => {
  it.each(vectors.duffsToMinor)("$duffs duffs at $price $currency is $minor minor units", (v) => {
    expect(duffsToMinor(v.duffs, v.price, v.currency)).toBe(BigInt(v.minor));
  });

  it.each(vectors.minorToDuffs)("$minor minor units of $currency at $price is $duffs duffs", (v) => {
    expect(minorToDuffs(BigInt(v.minor), v.price, v.currency)).toBe(BigInt(v.duffs));
  });

  it("covers ties both ways, every exponent and the whole supply", () => {
    const ties = vectors.duffsToMinor.filter((v) => ["0.05", "0.01", "1"].includes(v.price));
    expect(new Set(ties.map((v) => BigInt(v.minor) % 2n))).toEqual(new Set([0n]));
    expect(new Set(vectors.duffsToMinor.map((v) => minorUnits(v.currency)))).toEqual(new Set([0, 2, 3]));
    expect(vectors.duffsToMinor.some((v) => v.duffs === 2_100_000_000_000_000)).toBe(true);
  });

  it("round-trips fiat entry within one minor unit", () => {
    for (const price of ["49.0967", "0.05", "7873.9517", "15.4476"]) {
      for (const currency of ["USD", "JPY", "KWD"]) {
        for (const minor of [1n, 99n, 1234n, 100_000n]) {
          const duffs = minorToDuffs(minor, price, currency)!;
          const back = duffsToMinor(duffs, price, currency)!;
          expect(back - minor <= 1n && minor - back <= 1n).toBe(true);
        }
      }
    }
  });
});

describe("prices and entry", () => {
  it("takes only the engine's plain positive decimals", () => {
    expect(parsePrice("49.0967")).toEqual({ mantissa: 490967n, scale: 4 });
    for (const bad of ["", "0", "0.00", "-1", "1e3", "1.", ".5", "1,5", " 1", "1.0000000000001"]) expect(parsePrice(bad)).toBeNull();
    expect(duffsToMinor(100, "0", "USD")).toBeNull();
    expect(duffsToMinor(-1, "1", "USD")).toBeNull();
    expect(duffsToMinor(0.5, "1", "USD")).toBeNull();
  });

  it("parses typed fiat to at most the currency's decimals", () => {
    expect(parseFiat("10", "USD")).toBe(1000n);
    expect(parseFiat("10.5", "USD")).toBe(1050n);
    expect(parseFiat(".25", "USD")).toBe(25n);
    expect(parseFiat("1 000.00", "USD")).toBe(100_000n);
    expect(parseFiat("1.234", "USD")).toBeNull();
    expect(parseFiat("1.234", "KWD")).toBe(1234n);
    expect(parseFiat("100", "JPY")).toBe(100n);
    expect(parseFiat("1.5", "JPY")).toBeNull();
    for (const bad of ["", ".", "-1", "1,5", "1e2", "abc", "1.2.3"]) expect(parseFiat(bad, "USD")).toBeNull();
    expect(minorToPlain(1050n, "USD")).toBe("10.50");
    expect(minorToPlain(5n, "USD")).toBe("0.05");
    expect(minorToPlain(7874n, "JPY")).toBe("7874");
    expect(minorToPlain(1234n, "KWD")).toBe("1.234");
  });
});

describe("the text", () => {
  // Intl separates a code or symbol with a no-break space.
  const plain = (s: string | null) => s?.replace(/\u00a0/g, " ") ?? null;
  it("is built from the exact integer, never re-rounded", () => {
    expect(plain(formatMinor(4910n, "USD", "en-US"))).toBe("$49.10");
    expect(plain(formatMinor(123_456_789n, "USD", "en-US"))).toBe("$1,234,567.89");
    expect(plain(formatMinor(7874n, "JPY", "en-US"))).toBe("¥7,874");
    expect(plain(formatMinor(15_448n, "KWD", "en-US"))).toBe("KWD 15.448");
    expect(plain(formatMinor(4387n, "EUR", "de-DE"))).toBe("43,87 €");
    // A float would print 0.30000000000000004 or round 1.005 to 1.00; the integer path cannot.
    expect(plain(formatMinor(100_500n, "KWD", "en-US"))).toBe("KWD 100.500");
    // Beyond 2^53 the digits are still exact.
    expect(plain(formatMinor(10_309_449_999_999_999_999n, "USD", "en-US"))).toBe("$103,094,499,999,999,999.99");
  });

  it("never shows 0 and marks dust", () => {
    expect(plain(fiatText(0, "49.0967", "USD", "en-US"))).toBeNull();
    expect(plain(fiatText(1, "49.0967", "USD", "en-US"))).toBe("< $0.01");
    expect(plain(fiatText(100_000_000, "49.0967", "USD", "en-US"))).toBe("$49.10");
    expect(plain(fiatText(-100_000_000, "49.0967", "USD", "en-US"))).toBe("$49.10");
    expect(plain(fiatText(100_000_000, "bad", "USD", "en-US"))).toBeNull();
  });

  it("proposes the locale's currency and lists local currencies only", () => {
    expect(localeCurrency("en-US")).toBe("USD");
    expect(localeCurrency("de-DE")).toBe("EUR");
    expect(localeCurrency("ja")).toBe("JPY");
    expect(localeCurrency("xx")).toBe("USD");
    expect(["USD", "VES", "JPY"].every(isLocalCurrency)).toBe(true);
    expect(["BTC", "ETH", "XAU", "XAG", "XDR", "usd", "USDT"].some(isLocalCurrency)).toBe(false);
  });
});
