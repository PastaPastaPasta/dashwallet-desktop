import { describe, expect, it } from "vitest";
import { holdFor, RECEIVED_HOLD_MS, ROTATION_GRACE_MS } from "./hold";

describe("DEC1058_receiveHold", () => {
  it("shows the address a user asked for at once, a paid one for the hold, and waits the grace otherwise", () => {
    expect(holdFor(true, false)).toBe(0);
    expect(holdFor(true, true)).toBe(0);
    expect(holdFor(false, true)).toBe(RECEIVED_HOLD_MS);
    expect(holdFor(false, false)).toBe(ROTATION_GRACE_MS);
    expect(RECEIVED_HOLD_MS).toBeGreaterThan(ROTATION_GRACE_MS);
  });
});
