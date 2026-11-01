import { describe, expect, it } from "vitest";
import type { Balances } from "../api/generated/Balances";
import { coreSubtitle, homeBalance, POOL_ROWS } from "./homeBalance";

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
