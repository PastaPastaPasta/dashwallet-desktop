// Spending from Shielded where this build cannot: the engine's flag, the one reason line every spend
// entry point gives, and the engine's refusal read the same. Where the flag says spends work (main
// today), every line stays what it was.

import { describe, expect, it } from "vitest";
import type { ShieldedStatus } from "../api/generated/ShieldedStatus";
import { appError } from "../onboarding/testWorld";
import { fromRows } from "../screens/pay/fromBalance";
import { shieldedErrorText } from "./flow";
import { SHIELDED_SPENDS_COPY, shieldedSpendsOff, spendsOffRefusal } from "./spends";

const STATUS: ShieldedStatus = { active: true, protocolVersion: 13, activationVersion: 12, ready: true, balance: "125000000000", pending: "0", pendingShields: [], unresolvedSpends: [], heldSpends: [], held: "0", unrecordedSpends: [], syncedIndex: 4096, lastSyncAt: 1, syncing: false, proverReady: true, spendsAvailable: true, account: "bound", scan: null, lastPassFailed: false };
const credits = (c: string) => `${c} credits`;
const duffs = (d: number) => `${d} duffs`;
const UNAVAILABLE = appError("platform.unavailable");

describe("Shielded spends off", () => {
  it("is off only where Shielded is on and the engine says it cannot spend", () => {
    expect(shieldedSpendsOff(STATUS)).toBe(false);
    expect(shieldedSpendsOff({ ...STATUS, spendsAvailable: false })).toBe(true);
    expect(shieldedSpendsOff({ ...STATUS, active: false, spendsAvailable: false })).toBe(false);
    expect(shieldedSpendsOff(null)).toBe(false);
    expect(shieldedSpendsOff(undefined)).toBe(false);
  });

  it("reads the engine's refusal of a spend or a new shield as the reason line, and not a resume's", () => {
    for (const kind of ["send", "out", "toPlatform", "add"] as const) {
      expect(spendsOffRefusal("platform.unavailable", kind, true), kind).toBe(true);
      expect(shieldedErrorText(UNAVAILABLE, credits, duffs, kind, false, false, true)).toBe(SHIELDED_SPENDS_COPY.unavailable);
    }
    expect(spendsOffRefusal("platform.unavailable", "resume", true)).toBe(false);
    expect(spendsOffRefusal("platform.timeout", "send", true)).toBe(false);
    expect(shieldedErrorText(UNAVAILABLE, credits, duffs, "resume", false, false, true)).not.toBe(SHIELDED_SPENDS_COPY.unavailable);
  });

  it("leaves every refusal as it was while spends work", () => {
    for (const kind of ["send", "out", "toPlatform", "add"] as const) {
      expect(shieldedErrorText(UNAVAILABLE, credits, duffs, kind, false, false, false)).toBe(shieldedErrorText(UNAVAILABLE, credits, duffs, kind));
      expect(shieldedErrorText(UNAVAILABLE, credits, duffs, kind)).not.toBe(SHIELDED_SPENDS_COPY.unavailable);
    }
  });

  it("gives Pay a contact's Shielded row its short reason, and leaves it as it was while spends work", () => {
    const inputs = { coreDuffs: 1_000, shielded: 125_000_000, platform: 40_000_000, amount: (d: number) => `${d}`, discreet: false, acceptsToo: false, live: new Set(["core", "shielded", "platform"] as const) };
    expect(fromRows({ ...inputs, shieldedSpendsOff: true }).find((r) => r.source === "shielded")?.reason).toBe(SHIELDED_SPENDS_COPY.source);
    expect(fromRows({ ...inputs, shieldedSpendsOff: true }).find((r) => r.source === "platform")?.reason).toBeNull();
    expect(fromRows({ ...inputs, shieldedSpendsOff: false })).toEqual(fromRows(inputs));
    expect(fromRows({ ...inputs, shieldedSpendsOff: false }).find((r) => r.source === "shielded")?.reason).toBeNull();
  });
});
