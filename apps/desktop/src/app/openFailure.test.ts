import { describe, expect, it } from "vitest";
import { restoreFailure, STORE_SCREENS, storageInUseError, storeRefusedError, storeRestoreError, storeRetryError } from "../fixture/errors";
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
    const refusal = STORE_SCREENS.refusedPrevious("/data/testnet");
    expect(openFailure(storeRefusedError(refusal))).toEqual({ kind: "refused", refusal });
  });

  it("gives StorageInUse its own screen, retryable only when the engine says so", () => {
    expect(openFailure(storageInUseError(true))).toEqual({ kind: "inUse", retryable: true });
    expect(openFailure(storageInUseError(false))).toEqual({ kind: "inUse", retryable: false });
    expect(openFailure({ code: "storage_in_use", message: "held", data: null })).toEqual({ kind: "inUse", retryable: false });
  });

  it("gives a restore that stopped the open its own screen, by its state", () => {
    for (const name of ["refused:garbledMarker", "noSpace:diskFull", "notStarted:other", "incomplete:permissionDenied"]) {
      const restore = restoreFailure(name, "/data/testnet");
      expect(openFailure(storeRestoreError(restore))).toEqual({ kind: "restore", restore });
    }
  });

  it("does not take a storage_restore without whole data for a restore screen, and offers no retry", () => {
    const whole = restoreFailure("incomplete:other", "/data");
    const broken = [null, { ...whole, kind: "storeRefused" }, { ...whole, state: "partial" }, { ...whole, cause: 3 }, { ...whole, folder: undefined }, { ...whole, neededBytes: "1" }, { ...whole, neededBytes: -1 }];
    for (const data of broken) expect(openFailure({ code: "storage_restore", message: "stopped", data })).toEqual({ kind: "message", text: "Dash Wallet could not open the wallet: stopped", retry: false });
    const { neededBytes: _, ...withoutBytes } = whole;
    expect(openFailure({ code: "storage_restore", message: "stopped", data: withoutBytes })).toEqual({ kind: "restore", restore: whole });
  });

  it("gives an upgrade that stopped on the machine its own screen; without whole data, the engine's text and no retry", () => {
    expect(openFailure(storeRetryError("/data"))).toEqual({ kind: "retry", retry: { kind: "retry", folder: "/data" } });
    for (const data of [null, { kind: "retry" }, { kind: "retry", folder: 3 }, { kind: "restore", folder: "/data" }])
      expect(openFailure({ code: "storage_retry", message: "stopped", data })).toEqual({ kind: "message", text: "Dash Wallet could not open the wallet: stopped", retry: false });
  });

  it("does not take a storage_refused without its screen for a refusal", () => {
    expect(openFailure({ code: "storage_refused", message: "x", data: null })).toMatchObject({ kind: "message", retry: false });
    expect(openFailure({ code: "storage_refused", message: "x", data: { kind: "storeRefused", folder: "/data", cause: "P9aBroken" } })).toMatchObject({ kind: "message", retry: false });
    const whole = STORE_SCREENS.refusedPrevious("/data");
    const broken = [
      { ...whole, screen: { ...whole.screen, buttons: undefined } },
      { ...whole, screen: { ...whole.screen, buttons: [{ key: "storeUpgrade.refused.quit", label: 3, destructive: false, defaultFocus: false }] } },
      { ...whole, screen: { ...whole.screen, title_key: "storeUpgrade.refused.title", titleKey: undefined } },
      { ...whole, screen: { ...whole.screen, hint: { key: "x" } } },
      { ...whole, screen: { ...whole.screen, lines: undefined } },
      { ...whole, screen: { ...whole.screen, lines: [] } },
      { ...whole, screen: { ...whole.screen, lines: [{ key: "storeUpgrade.refused.intro", text: 3 }] } },
      { ...whole, folder: null },
    ];
    for (const data of broken) expect(openFailure({ code: "storage_refused", message: "x", data })).toMatchObject({ kind: "message", retry: false });
  });
});
