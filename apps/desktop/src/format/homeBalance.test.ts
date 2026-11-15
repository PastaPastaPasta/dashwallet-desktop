import { describe, expect, it } from "vitest";
import type { Balances } from "../api/generated/Balances";
import { compact, masked } from "./amount";
import { breakdownRows, coreSubtitle, heroDigits, homeBalance, POOL_ROWS } from "./homeBalance";

const COIN = 100_000_000;
const core: Balances = { confirmed: 3 * COIN, unconfirmed: COIN, immature: 0, locked: 0, total: 4 * COIN, coinjoin: 2 * COIN };
const fmt = (duffs: number) => (duffs / COIN).toFixed(2);

describe("Home's balance", () => {
  it("is Core (mixed coins included) plus Shielded plus Platform", () => {
    const b = homeBalance(core, 5 * COIN, COIN / 2);
    expect(b.total).toBe(9.5 * COIN);
    expect(b.pools).toEqual({ core: 4 * COIN, shielded: 5 * COIN, platform: COIN / 2 });
  });

  it("leaves out a pool that is not known, and says so in its row", () => {
    const b = homeBalance(core, null, undefined);
    expect(b.total).toBe(4 * COIN);
    expect(b.pools).toEqual({ core: 4 * COIN, shielded: null, platform: null });
    expect(homeBalance(core, 0, 0).pools).toMatchObject({ shielded: 0, platform: 0 });
  });

  it("counts only what can be spent: immature and locked funds stay out of the hero and the Core row", () => {
    const b = homeBalance({ confirmed: COIN, unconfirmed: 0, immature: 2 * COIN, locked: 3 * COIN, total: 6 * COIN, coinjoin: 0 }, 0, 0);
    expect(b.total).toBe(COIN);
    expect(b.pools.core).toBe(COIN);
  });

  it("names what is pending, immature and locked under the Core row", () => {
    expect(coreSubtitle(core, fmt, "Dash wallet")).toBe("1.00 pending");
    const held = { ...core, unconfirmed: 0, immature: 2 * COIN, locked: 10 * COIN };
    expect(coreSubtitle(held, fmt, "Dash wallet")).toBe("2.00 immature · 10.00 locked");
    expect(coreSubtitle({ ...held, immature: 0, locked: 0 }, fmt, "Dash wallet")).toBe("Dash wallet");
  });

  it("always lists Core, Shielded and Platform, in that order", () => {
    expect(POOL_ROWS.map((r) => r.pool)).toEqual(["core", "shielded", "platform"]);
  });
});

describe("Home's breakdown rows", () => {
  const settled: Balances = { ...core, unconfirmed: 0 };

  it("hides an empty or unknown Shielded or Platform row", () => {
    expect(breakdownRows(core, homeBalance(core, 5 * COIN, COIN / 2).pools)).toEqual(["core", "shielded", "platform"]);
    expect(breakdownRows(core, homeBalance(core, 0, COIN / 2).pools)).toEqual(["core", "platform"]);
    expect(breakdownRows(core, homeBalance(core, null, 0).pools)).toEqual(["core"]);
    expect(breakdownRows(settled, homeBalance(settled, 1, undefined).pools)).toEqual(["core", "shielded"]);
  });

  it("drops the breakdown when Core alone would repeat the hero", () => {
    expect(breakdownRows(settled, homeBalance(settled, 0, 0).pools)).toEqual([]);
    expect(breakdownRows(settled, homeBalance(settled, null, null).pools)).toEqual([]);
    // Something pending, immature or locked is worth a line under the hero.
    for (const news of [{ unconfirmed: 1 }, { immature: 1 }, { locked: 1 }]) {
      const b = { ...settled, ...news };
      expect(breakdownRows(b, homeBalance(b, 0, null).pools)).toEqual(["core"]);
    }
  });
});

describe("the hero's digits", () => {
  it("keeps two decimals at full size and the rest smaller, without rounding", () => {
    expect(heroDigits(compact(3_204_698_287, false))).toEqual({ major: "32.04", minor: "698287" });
    expect(heroDigits(compact(125_000_000, false))).toEqual({ major: "1.25", minor: "" });
    expect(heroDigits(compact(100_000_001, false))).toEqual({ major: "1.00", minor: "000001" });
    expect(heroDigits(compact(0, false))).toEqual({ major: "0.00", minor: "" });
  });

  it("joins back into the exact amount in every unit, and leaves discreet and duffs alone", () => {
    for (const unit of ["DASH", "mDASH", "uDASH", "duffs"] as const) {
      const text = compact(123_456_789_012, false, unit);
      const { major, minor } = heroDigits(text);
      expect(major + minor).toBe(text);
    }
    expect(heroDigits(compact(123_456_789_012, false, "duffs")).minor).toBe("");
    expect(heroDigits(masked())).toEqual({ major: masked(), minor: "" });
  });
});
