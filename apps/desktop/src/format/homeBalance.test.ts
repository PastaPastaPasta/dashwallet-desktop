import { describe, expect, it } from "vitest";
import type { Balances } from "../api/generated/Balances";
import { compact, masked } from "./amount";
import { breakdownRows, coreSubtitle, heroDigits, homeBalance, POOL_ROWS, type PoolReading } from "./homeBalance";

const COIN = 100_000_000;
const core: Balances = { confirmed: 3 * COIN, unconfirmed: COIN, immature: 0, locked: 0, total: 4 * COIN, coinjoin: 2 * COIN, catchingUp: false };
const fmt = (duffs: number) => (duffs / COIN).toFixed(2);

describe("Home's balance", () => {
  it("is Core (mixed coins included) plus Shielded plus Platform", () => {
    const b = homeBalance(core, 5 * COIN, COIN / 2);
    expect(b.total).toBe(9.5 * COIN);
    expect(b.pools).toEqual({ core: 4 * COIN, shielded: 5 * COIN, platform: COIN / 2 });
  });

  it("counts only known pools in the hero, and keeps why the others have no number", () => {
    for (const [shielded, platform] of [["loading", "failed"], ["absent", "loading"], ["failed", "absent"]] as const) {
      const b = homeBalance(core, shielded, platform);
      expect(b.total).toBe(4 * COIN);
      expect(b.pools).toEqual({ core: 4 * COIN, shielded, platform });
    }
    expect(homeBalance(core, 0, 0).pools).toMatchObject({ shielded: 0, platform: 0 });
    expect(homeBalance(core, 5 * COIN, "failed").total).toBe(9 * COIN);
  });

  it("counts only what can be spent: immature and locked funds stay out of the hero and the Core row", () => {
    const b = homeBalance({ confirmed: COIN, unconfirmed: 0, immature: 2 * COIN, locked: 3 * COIN, total: 6 * COIN, coinjoin: 0, catchingUp: false }, 0, 0);
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
  const rows = (shielded: PoolReading, platform: PoolReading, discreet = false) => breakdownRows(homeBalance(core, shielded, platform).pools, discreet);

  it("hides a pool only when the wallet has none or it is known to be empty", () => {
    expect(rows(5 * COIN, COIN / 2)).toEqual(["core", "shielded", "platform"]);
    expect(rows(0, COIN / 2)).toEqual(["core", "platform"]);
    expect(rows("absent", COIN / 2)).toEqual(["core", "platform"]);
    expect(rows(1, 0)).toEqual(["core", "shielded"]);
  });

  it("keeps the row of a pool still being read or whose read failed", () => {
    expect(rows("loading", 0)).toEqual(["core", "shielded"]);
    expect(rows(0, "failed")).toEqual(["core", "platform"]);
    expect(rows("loading", "failed")).toEqual(["core", "shielded", "platform"]);
    expect(rows("absent", "loading")).toEqual(["core", "platform"]);
  });

  it("has no breakdown when Core alone would repeat the hero", () => {
    expect(rows(0, 0)).toEqual([]);
    expect(rows("absent", "absent")).toEqual([]);
    expect(rows("absent", 0)).toEqual([]);
  });

  it("in discreet mode, shows Core and every pool the wallet has, whatever they hold", () => {
    expect(rows(0, 0, true)).toEqual(["core", "shielded", "platform"]);
    expect(rows(5 * COIN, 0, true)).toEqual(["core", "shielded", "platform"]);
    expect(rows("absent", 0, true)).toEqual(["core", "platform"]);
    expect(rows("absent", COIN, true)).toEqual(["core", "platform"]);
    expect(rows("absent", "absent", true)).toEqual([]);
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
