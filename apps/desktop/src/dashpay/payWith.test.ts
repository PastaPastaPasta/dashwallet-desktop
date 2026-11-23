// C-funding F16, F17, F19: the "Pay with" step's rows and their reasons, and the fixture's Shielded quote.

import { describe, expect, it } from "vitest";
import type { ShieldedCheckName } from "../api/generated/ShieldedCheckName";
import type { ShieldedReadiness } from "../api/generated/ShieldedReadiness";
import type { UnrecordedSpend } from "../api/generated/UnrecordedSpend";
import { costs, SHIELDED_DENOMINATION, shieldedQuote, shieldedReadiness } from "../fixture/registration";
import { newShieldedWorld, type ShieldedWorld } from "../fixture/shielded";
import { LIVE_SOURCES, PAY_WITH_COPY, payWithRows, shieldedReason, type PayWithInputs, type PaySource } from "./payWith";

const amount = (duffs: number) => `${duffs / 1e8} Dash`;
const clock = (secs: number) => `T${secs}`;
const CHECKS: ShieldedCheckName[] = ["poolActive", "featureOn", "denomination", "balance", "noUnsettledSpend", "synced", "rested"];

/** Every check ready but `failed`. */
const readiness = (failed: ShieldedCheckName | null, readyAt: number | null = null): ShieldedReadiness => ({
  denomination: "3000000000",
  checks: CHECKS.map((check) => ({ check, ready: check !== failed })),
  readyAt,
});

const inputs = (i: Partial<PayWithInputs> = {}): PayWithInputs => ({
  core: { availableDuffs: 10e8, totalDuffs: 3_002_260 },
  platformDuffs: 5e8,
  shieldedDuffs: 2e8,
  readiness: readiness(null),
  amount,
  clock,
  discreet: false,
  ...i,
});

const reasons = (i: PayWithInputs) => Object.fromEntries(payWithRows(i).map((r) => [r.source, r.reason]));

describe("Pay with rows (F19)", () => {
  it("lists Dash wallet, Platform and Shielded with their balances", () => {
    const rows = payWithRows(inputs());
    expect(rows.map((r) => [r.source, r.label, r.balance])).toEqual([
      ["core", PAY_WITH_COPY.core, "10 Dash"],
      ["platform", PAY_WITH_COPY.platform, "5 Dash"],
      ["shielded", PAY_WITH_COPY.shielded, "2 Dash"],
    ]);
  });

  it("shows no balance in discreet mode or while unread", () => {
    expect(payWithRows(inputs({ discreet: true })).map((r) => r.balance)).toEqual([null, null, null]);
    expect(payWithRows(inputs({ core: null, platformDuffs: null, shieldedDuffs: null })).map((r) => r.balance)).toEqual([null, null, null]);
  });

  it("Platform and Shielded are not offered until this build can start them", () => {
    expect(LIVE_SOURCES).toEqual(new Set(["core"]));
    expect(reasons(inputs())).toEqual({ core: null, platform: PAY_WITH_COPY.notOffered, shielded: PAY_WITH_COPY.notOffered });
    const live = new Set<PaySource>(["core", "platform", "shielded"]);
    expect(reasons(inputs({ live }))).toEqual({ core: null, platform: null, shielded: null });
  });

  it("Dash wallet short of the quote names the least it needs", () => {
    expect(reasons(inputs({ core: { availableDuffs: 1_000, totalDuffs: 3_002_260 } })).core).toBe(PAY_WITH_COPY.short("0.0300226 Dash"));
  });

  it("a Shielded readiness reason shows before the not-offered one", () => {
    expect(reasons(inputs({ readiness: readiness("synced") })).shielded).toBe(PAY_WITH_COPY.syncing);
  });
});

describe("Shielded readiness reasons (F16, F17)", () => {
  const reason = (r: ShieldedReadiness | null) => shieldedReason(r, amount, clock);

  it("each failed check has its one reason, in the engine's order", () => {
    expect(reason(readiness(null))).toBeNull();
    expect(reason(null)).toBeNull();
    for (const check of ["poolActive", "featureOn", "denomination"] as const) expect(reason(readiness(check))).toBe(PAY_WITH_COPY.notOffered);
    expect(reason(readiness("balance"))).toBe(PAY_WITH_COPY.short("0.03 Dash"));
    expect(reason(readiness("noUnsettledSpend"))).toBe(PAY_WITH_COPY.unsettled);
    expect(reason(readiness("synced"))).toBe(PAY_WITH_COPY.syncing);
    expect(reason(readiness("rested", 1_700_000_000))).toBe(PAY_WITH_COPY.resting("T1700000000"));
    // The first failure wins.
    const two = readiness("synced");
    two.checks = two.checks.map((c) => (c.check === "noUnsettledSpend" ? { ...c, ready: false } : c));
    expect(reason(two)).toBe(PAY_WITH_COPY.unsettled);
  });

  it("notes not rested with no time read (clock unknown, first scan running) say syncing", () => {
    expect(reason(readiness("rested", null))).toBe(PAY_WITH_COPY.syncing);
  });
});

describe("the fixture's Shielded quote", () => {
  const world = (w: Partial<ShieldedWorld> = {}): ShieldedWorld => ({ ...newShieldedWorld(), ...w });
  const failed = (w: ShieldedWorld | undefined) => shieldedReadiness(w).checks.find((c) => !c.ready)?.check ?? null;

  it("reads the Shielded world as the engine's readiness does", () => {
    expect(failed(world())).toBeNull();
    expect(failed(undefined)).toBe("poolActive");
    expect(failed(world({ balance: "1000" }))).toBe("balance");
    expect(failed(world({ scanning: true }))).toBe("synced");
    expect(shieldedReadiness(world({ restingUntil: 99 }))).toMatchObject({ readyAt: 99 });
    expect(failed(world({ restingUntil: 99 }))).toBe("rested");
    expect(failed(world({ active: false }))).toBe("poolActive");
    expect(failed(world({ unrecorded: [{} as UnrecordedSpend] }))).toBe("noUnsettledSpend");
  });

  it("asks nothing in duffs and a grant for the denomination and the name", () => {
    const quote = shieldedQuote("alice-77", world());
    const maxCredits = String(costs("alice-77").credits + SHIELDED_DENOMINATION);
    expect(quote).toMatchObject({ totalDuffs: 0, availableDuffs: 0, grant: { maxDuffs: 0, maxCredits } });
    expect(shieldedQuote("alice-77", undefined).grant.maxCredits).toBe(String(costs("alice-77").credits));
  });
});
