// Shielded → Platform in the fixture (C-routes D-C1, C1c), through IpcBackend as Transfer calls it:
// the engine's order of refusals, the own-address rule, the outcomes, and mainnet off.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SHIELDED_CODES } from "../api/pending";
import { IpcBackend } from "../api/tauri";
import { installFixture, type FixtureHandle } from "./index";
import { PLATFORM_PAYEE } from "./pending";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID as W } from "./scenarios";
import { noteSelectionFee, UNSHIELD_ADDRESS_FEE_CREDITS, UNSHIELD_TO_PLATFORM } from "./shielded";

let fixture: FixtureHandle;
let backend: IpcBackend;
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  const stored = new Map<string, string>();
  vi.stubGlobal("sessionStorage", { getItem: (k: string) => stored.get(k) ?? null, setItem: (k: string, v: string) => stored.set(k, v), removeItem: (k: string) => stored.delete(k) });
  fixture = installFixture({ scenario: "funded" });
  backend = new IpcBackend("fixture");
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

const passphrase = { kind: "passphrase" as const, passphrase: FIXTURE_PASSPHRASE };
const opGrant = (maxCredits: string) => backend.authorize({ kind: "platformOp", maxDuffs: 0, maxCredits }, W, passphrase);
const shielded = () => fixture.world.pending.shielded!;
const platform = () => fixture.world.pending.platform!;
const onReceive = () => platform().addresses.find((a) => a.address === platform().receive.address)?.credits ?? "0";

describe("fixture Shielded → Platform", () => {
  it("quotes an unshield to the receive address: the spend's fee, nothing in duffs, a credits grant", async () => {
    const q = await backend.unshieldToPlatformQuote(W, "10000000000", null);
    expect(q).toMatchObject({ credits: "10000000000", expectedDuffs: null, grant: { maxDuffs: 0, maxCredits: q.totalCredits } });
    expect(BigInt(q.totalCredits)).toBe(10000000000n + BigInt(q.feeCredits));
    // The engine's unshield fee: the spend's, plus the flat cost of the address it credits.
    const toCore = await backend.unshieldQuote(W, "10000000000", null);
    expect(UNSHIELD_ADDRESS_FEE_CREDITS).toBe(6_082_800n);
    expect(BigInt(q.feeCredits)).toBe(BigInt(toCore.feeCredits) + UNSHIELD_ADDRESS_FEE_CREDITS);
    // The wallet's own Platform addresses are taken; anyone else's is refused before the grant.
    await expect(backend.unshieldToPlatformQuote(W, "10000000000", platform().addresses[0].address)).resolves.toMatchObject({ credits: "10000000000" });
    await expect(backend.unshieldToPlatformQuote(W, "10000000000", PLATFORM_PAYEE)).rejects.toMatchObject({ code: SHIELDED_CODES.invalidAddress });
    await expect(backend.unshieldToPlatformQuote(W, "0", null)).rejects.toMatchObject({ code: SHIELDED_CODES.belowMinimum });
    await expect(backend.unshieldToPlatformQuote(W, "999000000000", null)).rejects.toMatchObject({ code: SHIELDED_CODES.insufficientFunds });
  });

  it("done: Shielded is debited the total, the receive address credited the amount, and the outcome names it", async () => {
    const before = BigInt(shielded().balance);
    const was = BigInt(onReceive());
    const q = await backend.unshieldToPlatformQuote(W, "10000000000", null);
    const grant = await opGrant(q.totalCredits);
    const out = await backend.unshieldToPlatform(W, "10000000000", null, grant.id, false);
    expect(out).toMatchObject({ credits: "10000000000", state: "done", to: platform().receive.address, unresolved: null });
    expect(BigInt(shielded().balance)).toBe(before - BigInt(q.totalCredits));
    expect(BigInt(onReceive())).toBe(was + 10000000000n);
    expect(shielded().activity[0]).toMatchObject({ kind: "unshield", direction: "out", status: "confirmed", counterparty: platform().receive.address });
    expect(platform().activity[0]).toMatchObject({ kind: "incoming", credits: "10000000000", address: platform().receive.address });
  });

  it("pending: nothing reaches Platform, the spend's total stays reserved and the row stays pending", async () => {
    shielded().next = "unknown";
    const before = BigInt(shielded().balance);
    const was = onReceive();
    const q = await backend.unshieldToPlatformQuote(W, "10000000000", null);
    const out = await backend.unshieldToPlatform(W, "10000000000", null, (await opGrant(q.totalCredits)).id, false);
    expect(out.state).toBe("pending");
    expect(out.unresolved).not.toBeNull();
    expect([BigInt(shielded().balance), onReceive()]).toEqual([before - BigInt(q.totalCredits), was]);
    expect(shielded().activity[0]).toMatchObject({ kind: "unshield", status: "pending" });
    // The reserved credits are not spendable again: the old Max is now short.
    const max = String(before - BigInt(q.feeCredits));
    await expect(backend.unshieldToPlatformQuote(W, max, null)).rejects.toMatchObject({ code: SHIELDED_CODES.insufficientFunds });
  });

  it("pending with notes: the notes the spend took are held back whole, change and all", async () => {
    shielded().notes = ["60000000000", "40000000000", "25000000000"];
    shielded().next = "unknown";
    const q = await backend.unshieldToPlatformQuote(W, "50000000000", null);
    expect(BigInt(q.feeCredits)).toBe(noteSelectionFee([60000000000n, 40000000000n, 25000000000n], 50000000000n, UNSHIELD_ADDRESS_FEE_CREDITS));
    await backend.unshieldToPlatform(W, "50000000000", null, (await opGrant(q.totalCredits)).id, false);
    expect([shielded().notes, shielded().balance]).toEqual([["40000000000", "25000000000"], "65000000000"]);
  });

  it("held pool-floor refusal stays pending with notes held and carries poolTooSmall", async () => {
    shielded().poolTooSmallHeld = true;
    const before = BigInt(shielded().balance);
    const was = onReceive();
    const q = await backend.unshieldToPlatformQuote(W, "10000000000", null);
    const out = await backend.unshieldToPlatform(W, "10000000000", null, (await opGrant(q.totalCredits)).id, false);
    expect(out.state).toBe("pending");
    expect(out.unresolved).not.toBeNull();
    expect(out.pendingReason).toEqual({ kind: "poolTooSmall", notes: 120, required: 250 });
    expect([BigInt(shielded().balance), onReceive()]).toEqual([before - BigInt(q.totalCredits), was]);
    expect(shielded().held).toHaveLength(1);
    shielded().poolTooSmallHeld = false;
  });

  it("checks the grant before Platform's pool floor, and the floor refusal uses the grant up", async () => {
    shielded().poolTooSmall = true;
    const before = [shielded().balance, onReceive()];
    const q = await backend.unshieldToPlatformQuote(W, "10000000000", null);
    await expect(backend.unshieldToPlatform(W, "10000000000", null, "no-grant", false)).rejects.toMatchObject({ code: "platform.grant_invalid" });
    const grant = await opGrant(q.totalCredits);
    await expect(backend.unshieldToPlatform(W, "10000000000", null, grant.id, false)).rejects.toMatchObject({ code: SHIELDED_CODES.poolTooSmall });
    shielded().poolTooSmall = false;
    await expect(backend.unshieldToPlatform(W, "10000000000", null, grant.id, false)).rejects.toMatchObject({ code: "platform.grant_invalid" });
    expect([shielded().balance, onReceive()]).toEqual(before);
  });

  it("refuses a grant smaller than the total, a foreign address and an unconfirmed lost-record spend, and moves nothing", async () => {
    const before = [shielded().balance, onReceive()];
    const q = await backend.unshieldToPlatformQuote(W, "10000000000", null);
    const small = await opGrant(String(BigInt(q.totalCredits) - 1n));
    await expect(backend.unshieldToPlatform(W, "10000000000", null, small.id, false)).rejects.toMatchObject({ code: "platform.grant_exceeded" });
    await expect(backend.unshieldToPlatform(W, "10000000000", PLATFORM_PAYEE, (await opGrant(q.totalCredits)).id, false)).rejects.toMatchObject({ code: SHIELDED_CODES.invalidAddress });
    shielded().unrecorded = [{ activityId: "sh-x", attempts: 1 } as never];
    await expect(backend.unshieldToPlatform(W, "10000000000", null, (await opGrant(q.totalCredits)).id, false)).rejects.toMatchObject({ code: SHIELDED_CODES.unrecordedSpend });
    expect([shielded().balance, onReceive()]).toEqual(before);
  });

  it("is off on mainnet: the quote and the call refuse feature_off first, and nothing moves", async () => {
    fixture.world.launch.network = { kind: "mainnet", dirName: "mainnet", unitName: "DASH" };
    const before = [shielded().balance, onReceive(), shielded().activity.length, platform().activity.length];
    const off = { code: "platform.feature_off", data: { kind: "feature", feature: UNSHIELD_TO_PLATFORM } };
    expect(UNSHIELD_TO_PLATFORM).toBe("unshield_to_platform");
    await expect(backend.unshieldToPlatformQuote(W, "10000000000", null)).rejects.toMatchObject(off);
    // Before the own-address rule and the grant: even a foreign address or no grant reads as off.
    await expect(backend.unshieldToPlatformQuote(W, "10000000000", PLATFORM_PAYEE)).rejects.toMatchObject(off);
    await expect(backend.unshieldToPlatform(W, "10000000000", null, "no-grant", false)).rejects.toMatchObject(off);
    expect([shielded().balance, onReceive(), shielded().activity.length, platform().activity.length]).toEqual(before);
  });
});
