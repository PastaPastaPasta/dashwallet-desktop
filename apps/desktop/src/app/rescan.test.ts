import { describe, expect, it } from "vitest";
import { appError } from "../onboarding/testWorld";
import { RESCAN_INTERRUPTED, RESCAN_RUNNING, rescanAware } from "./rescan";

describe("rescanAware", () => {
  it("tells a running rescan from an interrupted one, and leaves other errors alone", async () => {
    const rescan = appError("rescan_in_progress");
    expect(await rescanAware(rescan, "other", async () => ({ scanned: 1 }))).toBe(RESCAN_RUNNING);
    expect(await rescanAware(rescan, "other", async () => null)).toBe(RESCAN_INTERRUPTED);
    // An unreadable progress says the less alarming text.
    expect(await rescanAware(rescan, "other", () => Promise.reject(new Error("x")))).toBe(RESCAN_RUNNING);
    expect(await rescanAware(appError("send.absurd_fee"), "other", async () => null)).toBe("other");
  });
});
