// Platform activity is read again while a move or send is held.

import { describe, expect, it } from "vitest";
import type { PlatformActivity } from "./generated/PlatformActivity";
import { PENDING_MOVE_REFETCH_MS, platformActivityRefetchMs } from "./queries";

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
