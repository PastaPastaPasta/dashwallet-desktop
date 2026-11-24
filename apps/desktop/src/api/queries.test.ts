// Platform activity is read again while a move or send is held.

import { describe, expect, it } from "vitest";
import type { Balances } from "./generated/Balances";
import type { PlatformActivity } from "./generated/PlatformActivity";
import type { ShieldedStatus } from "./generated/ShieldedStatus";
import { PENDING_MOVE_REFETCH_MS, platformActivityRefetchMs, poolReading, settledBalance, shieldedReading } from "./queries";

const row = (over: Partial<PlatformActivity>): PlatformActivity => ({
  id: "r",
  kind: "send",
  state: "done",
  credits: "1",
  address: null,
  to: null,
  expectedDuffs: null,
  feeCredits: null,
  at: 1,
  mayLand: false,
  estimated: false,
  ...over,
});

describe("platformActivityRefetchMs", () => {
  it("keeps reading while a move or send is pending or has mayLand", () => {
    expect(platformActivityRefetchMs([row({ state: "pending", mayLand: true })])).toBe(PENDING_MOVE_REFETCH_MS);
    expect(platformActivityRefetchMs([row({ kind: "moveToCore", state: "pending", mayLand: true })])).toBe(PENDING_MOVE_REFETCH_MS);
    expect(platformActivityRefetchMs([row({ state: "failed", mayLand: true })])).toBe(PENDING_MOVE_REFETCH_MS);
  });

  it("stops when nothing can still land", () => {
    expect(platformActivityRefetchMs([row({ state: "failed" }), row({ state: "replaced" }), row({ kind: "incoming" }), row({})])).toBe(false);
    expect(platformActivityRefetchMs([])).toBe(false);
    expect(platformActivityRefetchMs(null)).toBe(false);
    expect(platformActivityRefetchMs(undefined)).toBe(false);
  });
});

describe("Home's reading of a pool query", () => {
  const duffs = (n: number | null) => n ?? ("absent" as const);

  it("tells loading, failed, absent and a known balance apart", () => {
    expect(poolReading({ data: undefined, isError: false }, duffs)).toBe("loading");
    expect(poolReading({ data: undefined, isError: true }, duffs)).toBe("failed");
    expect(poolReading({ data: null, isError: false }, duffs)).toBe("absent");
    expect(poolReading({ data: 0, isError: false }, duffs)).toBe(0);
    expect(poolReading({ data: 40_000_000, isError: false }, duffs)).toBe(40_000_000);
  });

  it("keeps the last good reading when a refetch fails", () => {
    expect(poolReading({ data: 40_000_000, isError: true }, duffs)).toBe(40_000_000);
    expect(poolReading({ data: 0, isError: true }, duffs)).toBe(0);
  });

  it("reads Shielded as absent when inactive, loading until ready, then its balance", () => {
    const status = { active: true, ready: true, balance: "125000000000" } as ShieldedStatus;
    expect(shieldedReading(null)).toBe("absent");
    expect(shieldedReading({ ...status, active: false })).toBe("absent");
    expect(shieldedReading({ ...status, ready: false })).toBe("loading");
    expect(shieldedReading(status)).toBe(125_000_000);
    expect(shieldedReading({ ...status, balance: "0" })).toBe(0);
  });
});

describe("the wallet balance the screens read", () => {
  const known: Balances = { confirmed: 5, unconfirmed: 1, immature: 0, locked: 0, total: 6, coinjoin: 0, catchingUp: false, heldUnfinished: 0, heldUnvalued: 0 };

  it("is withheld while the wallet is catching up, and passed on once it is not", () => {
    expect(settledBalance({ ...known, catchingUp: true })).toBeNull();
    expect(settledBalance(known)).toBe(known);
    expect(settledBalance(null)).toBeNull();
  });
});
