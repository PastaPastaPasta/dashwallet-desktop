// C-funding F16, F17, F19: the "Pay with" step's rows and their reasons, and the fixture's Shielded quote.

import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import type { ShieldedCheckName } from "../api/generated/ShieldedCheckName";
import type { ShieldedReadiness } from "../api/generated/ShieldedReadiness";
import type { UnrecordedSpend } from "../api/generated/UnrecordedSpend";
import { appError } from "../onboarding/testWorld";
import { costs, platformQuote, SHIELDED_DENOMINATION, shieldedQuote, shieldedReadiness } from "../fixture/registration";
import { noIdentity } from "../fixture/dashpay";
import { newPlatformWorld } from "../fixture/pending";
import { newShieldedWorld, type ShieldedWorld } from "../fixture/shielded";
import { FIXTURE_NOW, secs } from "../fixture/wallet";
import { dashpayKeys, platformSourceOf } from "./queries";
import { LIVE_SOURCES, PAY_WITH_COPY, payWithRows, platformReason, shieldedReason, type PayWithInputs, type PaySource } from "./payWith";

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
  platform: { quote: platformQuote("alice-77", newPlatformWorld(), false, noIdentity()) },
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

  it("Dash wallet and Platform are offered; Shielded is not until this build can start it", () => {
    expect(LIVE_SOURCES).toEqual(new Set(["core", "platform"]));
    expect(reasons(inputs())).toEqual({ core: null, platform: null, shielded: PAY_WITH_COPY.notOffered });
    expect(reasons(inputs({ live: new Set<PaySource>(["core"]) })).platform).toBe(PAY_WITH_COPY.notOffered);
    const live = new Set<PaySource>(["core", "platform", "shielded"]);
    expect(reasons(inputs({ live }))).toEqual({ core: null, platform: null, shielded: null });
  });

  it("F9: Platform's reason is its quote's refusal", () => {
    const refused = (code: Parameters<typeof appError>[0], data: Parameters<typeof appError>[1] = null) => reasons(inputs({ platform: { error: appError(code, data) } })).platform;
    // The planned debit, rounded up to a duff.
    expect(refused("platform.insufficient_credits", { kind: "credits", needed: "2126000001", available: "5" })).toBe(PAY_WITH_COPY.short("0.02126001 Dash"));
    expect(refused("credits.move_pending", { kind: "artifact", artifact: "m1" })).toBe(PAY_WITH_COPY.movePending);
    // Mainnet's flag, and a network without Platform.
    expect(refused("platform.feature_off", { kind: "feature", feature: "platform_registration" })).toBe(PAY_WITH_COPY.notOffered);
    expect(refused("platform.unavailable")).toBe("Dash Platform did not answer. Try again.");
    expect(platformReason(undefined, amount)).toBe("Reading your balances…");
  });

  it("a refused read of the Platform quote is the row's reason even with an earlier quote kept", () => {
    const quote = platformQuote("alice-77", newPlatformWorld(), false, noIdentity());
    const error = appError("credits.move_pending", { kind: "artifact", artifact: "m1" });
    expect(platformSourceOf({ data: quote, isError: false, error: null })).toEqual({ quote });
    expect(platformSourceOf({ data: quote, isError: true, error })).toEqual({ error });
    expect(platformSourceOf({ data: undefined, isError: false, error: null })).toBeUndefined();
    expect(reasons(inputs({ platform: platformSourceOf({ data: quote, isError: true, error }) })).platform).toBe(PAY_WITH_COPY.movePending);
  });

  it("Dash wallet short of the quote names the least it needs", () => {
    expect(reasons(inputs({ core: { availableDuffs: 1_000, totalDuffs: 3_002_260 } })).core).toBe(PAY_WITH_COPY.short("0.0300226 Dash"));
  });

  it("Shielded not offered here says so over any readiness reason; offered, its readiness reason", () => {
    expect(reasons(inputs({ readiness: readiness("balance") })).shielded).toBe(PAY_WITH_COPY.notOffered);
    expect(reasons(inputs({ readiness: readiness("balance"), live: new Set<PaySource>(["core", "shielded"]) })).shielded).toBe(PAY_WITH_COPY.short("0.03 Dash"));
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

  it("an unsettled spend that is a hold says so and offers no Try again", () => {
    const hold = { count: 1, credits: "30000000000", duffs: 30_000_000 };
    const held = shieldedReason(readiness("noUnsettledSpend"), amount, clock, hold);
    expect(held).toBe(PAY_WITH_COPY.held);
    expect(held).toBe("An earlier shielded payment may still go through, so part of your Shielded balance is held and can't be used yet. You can pay from Shielded once that payment goes through, or once the network shows it can no longer go through. Dash Wallet keeps checking while it's open.");
    expect(held).not.toMatch(/try again/i);
    // A listed spend that holds nothing (a scan saw its notes spent) is still settling.
    expect(shieldedReason(readiness("noUnsettledSpend"), amount, clock, null)).toBe(PAY_WITH_COPY.unsettled);
    // The row says the same once Shielded is offered, and only for that check.
    const live = new Set<PaySource>(["core", "shielded"]);
    expect(reasons(inputs({ readiness: readiness("noUnsettledSpend"), shieldedHold: hold, live })).shielded).toBe(PAY_WITH_COPY.held);
    expect(reasons(inputs({ readiness: readiness("noUnsettledSpend"), shieldedHold: null, live })).shielded).toBe(PAY_WITH_COPY.unsettled);
    expect(reasons(inputs({ readiness: readiness("synced"), shieldedHold: hold, live })).shielded).toBe(PAY_WITH_COPY.syncing);
    expect(reasons(inputs({ readiness: readiness("noUnsettledSpend"), shieldedHold: hold })).shielded).toBe(PAY_WITH_COPY.notOffered);
  });

  it("a balance short only because of a hold says the hold, not Not enough", () => {
    // The denomination is 3000000000 credits (3,000,000 duffs).
    const hold = (credits: string) => ({ count: 1, credits, duffs: Number(BigInt(credits) / 1000n) });
    const short = PAY_WITH_COPY.short("0.03 Dash");
    expect(shieldedReason(readiness("balance"), amount, clock, hold("2000000000"), 1_000_000)).toBe(PAY_WITH_COPY.held);
    expect(shieldedReason(readiness("balance"), amount, clock, hold("1999999999"), 1_000_000)).toBe(short);
    expect(shieldedReason(readiness("balance"), amount, clock, null, 1_000_000)).toBe(short);
    expect(shieldedReason(readiness("balance"), amount, clock, hold("2000000000"), null)).toBe(short);
    const live = new Set<PaySource>(["core", "shielded"]);
    expect(reasons(inputs({ readiness: readiness("balance"), shieldedDuffs: 1_000_000, shieldedHold: hold("2000000000"), live })).shielded).toBe(PAY_WITH_COPY.held);
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
    const later = secs(FIXTURE_NOW) + 600;
    expect(shieldedReadiness(world({ restingUntil: later }))).toMatchObject({ readyAt: later });
    expect(failed(world({ restingUntil: later }))).toBe("rested");
    // A deadline passed no longer holds the notes.
    expect(shieldedReadiness(world({ restingUntil: secs(FIXTURE_NOW) - 1 }))).toMatchObject({ readyAt: null });
    expect(failed(world({ restingUntil: secs(FIXTURE_NOW) - 1 }))).toBeNull();
    expect(failed(world({ active: false }))).toBe("poolActive");
    expect(failed(world({ unrecorded: [{} as UnrecordedSpend] }))).toBe("noUnsettledSpend");
    expect(failed(world({ held: [{ activityId: "sh-held-1", transition: "00", credits: "0" }] }))).toBe("noUnsettledSpend");
  });

  it("asks nothing in duffs and a grant for the denomination and the name", () => {
    const quote = shieldedQuote("alice-77", world());
    const maxCredits = String(costs("alice-77").credits + SHIELDED_DENOMINATION);
    expect(quote).toMatchObject({ totalDuffs: 0, availableDuffs: 0, grant: { maxDuffs: 0, maxCredits } });
    expect(shieldedQuote("alice-77", undefined).grant.maxCredits).toBe(String(costs("alice-77").credits));
  });
});

describe("the Shielded readiness quote's cache key", () => {
  it("is under the wallet's quotes and is no username's", async () => {
    const client = new QueryClient();
    client.setQueryData(dashpayKeys.shieldedReadiness("w"), "shielded");
    for (const label of ["shielded", "funding"]) expect(client.getQueryData(dashpayKeys.labelQuote("w", label))).toBeUndefined();
    await client.invalidateQueries({ queryKey: dashpayKeys.quote("w") });
    expect(client.getQueryState(dashpayKeys.shieldedReadiness("w"))?.isInvalidated).toBe(true);
  });
});

