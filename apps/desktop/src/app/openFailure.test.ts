import { describe, expect, it } from "vitest";
import { STORE_SCREENS, storageInUseError, storeRefusedError } from "../fixture/errors";
import { openFailure } from "./openFailure";

// DP3-04b D35: a wallet file from a newer build is its own, recoverable, refusal.

describe("the splash's open failure", () => {
  it("names a newer build's file and offers no retry", () => {
    const f = openFailure({ code: "storage_too_new", message: "fork schema version 2", data: null });
    expect(f.kind).toBe("message");
    if (f.kind !== "message") return;
    expect(f.text).toContain("written by a newer version of Dash Wallet");
    expect(f.text).not.toContain("backup");
    expect(f.text).toContain("(fork schema version 2)");
    expect(f.retry).toBe(false);
  });

  it("keeps the engine's text and the retry for anything else", () => {
    const f = openFailure({ code: "storage", message: "disk full", data: null });
    expect(f).toEqual({ kind: "message", text: "Dash Wallet could not open the wallet: disk full", retry: true });
  });

  it("gives a store refusal its own screen, with the engine's screen", () => {
    const refusal = STORE_SCREENS.cantOpen("/data/testnet");
    expect(openFailure(storeRefusedError(refusal))).toEqual({ kind: "refused", refusal });
  });

  it("gives StorageInUse its own screen, retryable only when the engine says so", () => {
    expect(openFailure(storageInUseError(true))).toEqual({ kind: "inUse", retryable: true });
    expect(openFailure(storageInUseError(false))).toEqual({ kind: "inUse", retryable: false });
    expect(openFailure({ code: "storage_in_use", message: "held", data: null })).toEqual({ kind: "inUse", retryable: false });
  });

  it("does not take a storage_refused without its screen for a refusal", () => {
    expect(openFailure({ code: "storage_refused", message: "x", data: null })).toMatchObject({ kind: "message", retry: false });
    expect(openFailure({ code: "storage_refused", message: "x", data: { kind: "storeRefused", folder: "/data", cause: "P9aBroken" } })).toMatchObject({ kind: "message", retry: false });
    const whole = STORE_SCREENS.cantOpen("/data");
    const broken = [
      { ...whole, screen: { ...whole.screen, buttons: undefined } },
      { ...whole, screen: { ...whole.screen, buttons: [{ key: "storeUpgrade.cantOpen.quit", label: 3, destructive: false, defaultFocus: false }] } },
      { ...whole, screen: { ...whole.screen, title_key: "storeUpgrade.cantOpen.title", titleKey: undefined } },
      { ...whole, screen: { ...whole.screen, hint: { key: "x" } } },
      { ...whole, folder: null },
    ];
    for (const data of broken) expect(openFailure({ code: "storage_refused", message: "x", data })).toMatchObject({ kind: "message", retry: false });
  });
});
