import { describe, expect, it } from "vitest";
import { openFailure } from "./openFailure";

// DP3-04b D35: a wallet file from a newer build is its own, recoverable, refusal.

describe("the splash's open failure", () => {
  it("names a newer build's file and offers no retry", () => {
    const f = openFailure({ code: "storage_too_new", message: "fork schema version 2", data: null });
    expect(f.text).toContain("written by a newer version of Dash Wallet");
    expect(f.text).not.toContain("backup");
    expect(f.text).toContain("(fork schema version 2)");
    expect(f.retry).toBe(false);
  });

  it("keeps the engine's text and the retry for anything else", () => {
    const f = openFailure({ code: "storage", message: "disk full", data: null });
    expect(f).toEqual({ text: "Dash Wallet could not open the wallet: disk full", retry: true });
  });
});
