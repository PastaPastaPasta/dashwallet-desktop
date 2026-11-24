import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { AppError } from "../api/generated/commands";
import { IpcBackend } from "../api/tauri";
import { installFixture, type FixtureHandle } from "./index";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID as W, type ScenarioName } from "./scenarios";

const PASS = { kind: "passphrase", passphrase: FIXTURE_PASSPHRASE } as const;

let fixture: FixtureHandle;
const open = (scenario: ScenarioName) => {
  clearMocks();
  fixture = installFixture({ scenario });
};
const grant = async (maxDuffs = 1) => (await cmd.authorize({ kind: "platformOp", maxDuffs, maxCredits: "0" }, W, PASS)).id;
const code = (p: Promise<unknown>) => p.then(() => "ok", (e: AppError) => e.code);

beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  open("funded");
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("fixture unfinished transfers", () => {
  it("lists nothing in an ordinary scenario", async () => {
    expect(await cmd.unfinishedTransfers(W)).toEqual({ locks: [], complete: true, unread: [] });
  });

  it("a kind that can't be read is unread, neither listed nor finished; the other kind is both", async () => {
    open("paddr-recovery-shielded-unread");
    fixture.world.pending.recovery.locks.push({ txid: "ab".repeat(32), vout: 0, kind: "shielded", duffs: 1_000_000, origin: "restored", state: "ready", remainingCredits: null });
    const listed = await cmd.unfinishedTransfers(W);
    expect(listed.unread).toEqual(["shielded"]);
    expect(listed.locks.map((l) => l.kind)).toEqual(["platformAddress"]);
    const report = await cmd.finishAssetLocks(W, await grant());
    expect(report.unread).toEqual(["shielded"]);
    expect(report.byKind).toEqual([{ kind: "platformAddress", resumed: 1, completed: 1, stillPending: 0 }]);
    expect(fixture.world.pending.recovery.locks.map((l) => l.kind)).toEqual(["shielded"]);
  });

  it("each scenario lists its restored locks", async () => {
    const states = async (s: ScenarioName) => (open(s), (await cmd.unfinishedTransfers(W)).locks.map((l) => [l.origin, l.kind, l.state]));
    expect(await states("recovery-one")).toEqual([["restored", "shielded", "ready"]]);
    expect(await states("recovery-restored-mixed")).toEqual([
      ["restored", "shielded", "ready"],
      ["restored", "shielded", "ready"],
      ["restored", "shielded", "checking"],
    ]);
    expect(await states("recovery-checking")).toEqual([["restored", "shielded", "checking"]]);
    expect(await states("recovery-confirming")).toEqual([["restored", "shielded", "confirming"]]);
    expect(await states("recovery-not-active")).toEqual([["restored", "shielded", "notActive"]]);
    expect(await states("recovery-unsupported")).toEqual([
      ["restored", "shielded", "ready"],
      ["restored", "shielded", "unsupported"],
      ["restored", "shielded", "tooSmall"],
    ]);
    open("recovery-one");
    expect((await cmd.unfinishedTransfers(W)).locks[0].duffs).toBe(50_000_000);
  });

  it("Finish applies a ready lock: it leaves the list and joins the balance", async () => {
    open("recovery-one");
    const [l] = (await cmd.unfinishedTransfers(W)).locks;
    const before = BigInt((await cmd.shieldedStatus(W)).balance);
    expect(await cmd.finishTransfer(W, l.kind, l.txid, l.vout, await grant())).toEqual({ kind: "applied" });
    expect((await cmd.unfinishedTransfers(W)).locks).toEqual([]);
    expect(BigInt((await cmd.shieldedStatus(W)).balance)).toBe(before + 50_000_000n * 1000n - 10_000_000n);
    expect(fixture.calls).toContain("finish_transfer");
  });

  it("Finish needs a platformOp grant with a funding budget, and a lock that is listed", async () => {
    open("recovery-one");
    const [l] = (await cmd.unfinishedTransfers(W)).locks;
    expect(await code(cmd.finishTransfer(W, l.kind, l.txid, l.vout, "no-such-grant"))).toBe("platform.grant_invalid");
    expect(await code(cmd.finishTransfer(W, l.kind, l.txid, l.vout, await grant(0)))).toBe("platform.needs_grant");
    expect(await code(cmd.finishTransfer(W, l.kind, "00".repeat(32), 0, await grant()))).toBe("invalid_argument");
    expect((await cmd.unfinishedTransfers(W)).locks).toHaveLength(1);
  });

  it("a checking lock stays checking (recovery-checking applies the second try), a confirming one waits, a not-active one is refused", async () => {
    const finish = async (s: ScenarioName) => {
      open(s);
      const [l] = (await cmd.unfinishedTransfers(W)).locks;
      return cmd.finishTransfer(W, l.kind, l.txid, l.vout, await grant());
    };
    expect(await finish("recovery-checking")).toEqual({ kind: "checking" });
    open("recovery-restored-mixed");
    const mixed = (await cmd.unfinishedTransfers(W)).locks[2];
    expect(await cmd.finishTransfer(W, mixed.kind, mixed.txid, mixed.vout, await grant())).toEqual({ kind: "checking" });
    expect(await cmd.finishTransfer(W, mixed.kind, mixed.txid, mixed.vout, await grant())).toEqual({ kind: "checking" });
    expect(await finish("recovery-confirming")).toEqual({ kind: "proofWaiting" });
    expect(await finish("recovery-not-active")).toEqual({ kind: "notApplied", error: "shielded.not_active" });
    open("recovery-unsupported");
    const [ready, second, small] = (await cmd.unfinishedTransfers(W)).locks;
    // Refused before the lease: the same grant then finishes the ready lock.
    const kept = await grant();
    expect(await cmd.finishTransfer(W, second.kind, second.txid, second.vout, kept)).toEqual({ kind: "notApplied", error: "recovery.unsupported_output" });
    expect(await cmd.finishTransfer(W, small.kind, small.txid, small.vout, kept)).toEqual({ kind: "notApplied", error: "shielded.below_minimum" });
    expect(await cmd.finishTransfer(W, ready.kind, ready.txid, ready.vout, kept)).toEqual({ kind: "applied" });
  });

  it("recovery-finish-fails: the first finish is not applied, the retry is", async () => {
    open("recovery-finish-fails");
    const [l] = (await cmd.unfinishedTransfers(W)).locks;
    expect(await cmd.finishTransfer(W, l.kind, l.txid, l.vout, await grant())).toEqual({ kind: "notApplied", error: "platform.unavailable" });
    expect((await cmd.unfinishedTransfers(W)).locks).toHaveLength(1);
    expect(await cmd.finishTransfer(W, l.kind, l.txid, l.vout, await grant())).toEqual({ kind: "applied" });
    expect((await cmd.unfinishedTransfers(W)).locks).toEqual([]);
  });

  it("recovery-checking: the second check applies the lock", async () => {
    open("recovery-checking");
    const [l] = (await cmd.unfinishedTransfers(W)).locks;
    expect(await cmd.finishTransfer(W, l.kind, l.txid, l.vout, await grant())).toEqual({ kind: "checking" });
    expect(await cmd.finishTransfer(W, l.kind, l.txid, l.vout, await grant())).toEqual({ kind: "applied" });
    expect((await cmd.unfinishedTransfers(W)).locks).toEqual([]);
  });

  it("Finish transfers counts as the engine does: ready applied, a restored checking one tried and still checking", async () => {
    open("recovery-restored-mixed");
    expect(await cmd.finishAssetLocks(W, await grant())).toEqual({
      resumed: 3,
      completed: 2,
      stillPending: 1,
      alreadyUsed: 0,
      byKind: [{ kind: "shielded", resumed: 3, completed: 2, stillPending: 1 }],
      unread: [],
    });
    expect((await cmd.unfinishedTransfers(W)).locks.map((l) => l.state)).toEqual(["checking"]);
  });

  it("a restored lock Platform already took reads checking; Check again is answered already used, adds nothing and leaves the list", async () => {
    for (const scenario of ["recovery-restored-finished", "paddr-recovery-restored-finished"] as const) {
      open(scenario);
      const [l] = (await cmd.unfinishedTransfers(W)).locks;
      expect([l.origin, l.state]).toEqual(["restored", "checking"]);
      const shielded = (await cmd.shieldedStatus(W)).balance;
      const credits = (await cmd.platformAddresses(W))?.credits;
      expect(await cmd.finishTransfer(W, l.kind, l.txid, l.vout, await grant())).toEqual({ kind: "reportedUsed" });
      expect((await cmd.unfinishedTransfers(W)).locks).toEqual([]);
      expect((await cmd.shieldedStatus(W)).balance).toBe(shielded);
      expect((await cmd.platformAddresses(W))?.credits).toBe(credits);
      expect((await cmd.platformActivity(W))?.filter((a) => a.id === `fund-${l.txid}`)).toEqual([]);
    }
  });

  it("Finish transfers tries a restored lock Platform already took and counts it already used", async () => {
    open("recovery-restored-finished");
    expect(await cmd.finishAssetLocks(W, await grant())).toEqual({
      resumed: 1,
      completed: 0,
      stillPending: 0,
      alreadyUsed: 1,
      byKind: [{ kind: "shielded", resumed: 1, completed: 0, stillPending: 0 }],
      unread: [],
    });
    expect((await cmd.unfinishedTransfers(W)).locks).toEqual([]);
  });

  it("a restored checking lock Platform never took is applied by Check again, and by Finish transfers", async () => {
    open("recovery-restored-unapplied");
    const [l] = (await cmd.unfinishedTransfers(W)).locks;
    expect([l.origin, l.state]).toEqual(["restored", "checking"]);
    const before = BigInt((await cmd.shieldedStatus(W)).balance);
    expect(await cmd.finishTransfer(W, l.kind, l.txid, l.vout, await grant())).toEqual({ kind: "applied" });
    expect((await cmd.unfinishedTransfers(W)).locks).toEqual([]);
    expect(BigInt((await cmd.shieldedStatus(W)).balance)).toBe(before + 50_000_000n * 1000n - 10_000_000n);
    open("recovery-restored-unapplied");
    expect(await cmd.finishAssetLocks(W, await grant())).toMatchObject({ resumed: 1, completed: 1, stillPending: 0, alreadyUsed: 0 });
    expect((await cmd.unfinishedTransfers(W)).locks).toEqual([]);
  });

  it("Finish transfers does not try confirming or not-active locks, and lists their kind", async () => {
    open("recovery-confirming");
    expect(await cmd.finishAssetLocks(W, await grant())).toMatchObject({ resumed: 0, completed: 0, stillPending: 1, byKind: [{ kind: "shielded", resumed: 0, completed: 0, stillPending: 1 }] });
    open("recovery-not-active");
    expect(await cmd.finishAssetLocks(W, await grant())).toMatchObject({ resumed: 0, stillPending: 1 });
    open("recovery-unsupported");
    expect(await cmd.finishAssetLocks(W, await grant())).toMatchObject({ resumed: 1, completed: 1, stillPending: 2, byKind: [{ kind: "shielded", resumed: 1, completed: 1, stillPending: 2 }] });
    expect((await cmd.unfinishedTransfers(W)).locks.map((l) => l.state)).toEqual(["unsupported", "tooSmall"]);
    open("funded");
    expect(await cmd.finishAssetLocks(W, await grant())).toMatchObject({ byKind: [] });
  });

  it("each Add from Dash wallet scenario lists its restored locks", async () => {
    const kinds = async (s: ScenarioName) => (open(s), (await cmd.unfinishedTransfers(W)).locks.map((l) => [l.origin, l.kind, l.state]));
    expect(await kinds("paddr-recovery-restored")).toEqual([["restored", "platformAddress", "ready"]]);
    expect(await kinds("paddr-recovery-mixed")).toEqual([
      ["restored", "shielded", "ready"],
      ["restored", "platformAddress", "ready"],
    ]);
  });

  describe("an Add from Dash wallet lock", () => {
    const platform = () => fixture.world.pending.platform!;
    const credits = async () => BigInt((await cmd.platformAddresses(W))!.credits);
    const fromCore = async () => (await cmd.platformActivity(W))!.filter((a) => a.kind === "fromCore");

    it("Finish credits the Platform receive address, records a done fromCore row and leaves the list", async () => {
      open("paddr-recovery-restored");
      const [l] = (await cmd.unfinishedTransfers(W)).locks;
      const before = await credits();
      const shielded = BigInt((await cmd.shieldedStatus(W)).balance);
      expect(await cmd.finishTransfer(W, l.kind, l.txid, l.vout, await grant())).toEqual({ kind: "applied" });
      expect((await cmd.unfinishedTransfers(W)).locks).toEqual([]);
      const gained = 50_000_000n * 1000n - 56_000_000n;
      expect(await credits()).toBe(before + gained);
      expect(BigInt((await cmd.shieldedStatus(W)).balance)).toBe(shielded);
      expect(platform().addresses.find((a) => a.address === platform().receive.address)).toBeDefined();
      expect(await fromCore()).toMatchObject([{ id: `fund-${l.txid}`, state: "done", credits: String(gained), address: platform().receive.address }]);
    });

    it("Finish transfers applies both kinds and counts each", async () => {
      open("paddr-recovery-mixed");
      const before = await credits();
      expect(await cmd.finishAssetLocks(W, await grant())).toEqual({
        resumed: 2,
        completed: 2,
        stillPending: 0,
        alreadyUsed: 0,
        byKind: [
          { kind: "shielded", resumed: 1, completed: 1, stillPending: 0 },
          { kind: "platformAddress", resumed: 1, completed: 1, stillPending: 0 },
        ],
        unread: [],
      });
      expect((await cmd.unfinishedTransfers(W)).locks).toEqual([]);
      expect(await credits()).toBe(before + 25_000_000n * 1000n - 56_000_000n);
      expect(await fromCore()).toHaveLength(1);
    });

    it("is refused for a kind that does not match the listed lock", async () => {
      open("paddr-recovery-restored");
      const [l] = (await cmd.unfinishedTransfers(W)).locks;
      expect(await code(cmd.finishTransfer(W, "shielded", l.txid, l.vout, await grant()))).toBe("invalid_argument");
      expect((await cmd.unfinishedTransfers(W)).locks).toHaveLength(1);
    });
  });

  describe("a refused transfer to Shielded (#402)", () => {
    const creditsGrant = async (maxCredits: string) => (await cmd.authorize({ kind: "platformOp", maxDuffs: 1, maxCredits }, W, PASS)).id;

    it("lists this device's lock as recoverable, quotes what is left, and recovers it under a grant of exactly that", async () => {
      open("recovery-recoverable");
      const [l] = (await cmd.unfinishedTransfers(W)).locks;
      expect([l.origin, l.state, l.remainingCredits]).toEqual(["thisDevice", "recoverable", "30000000000"]);
      expect(await cmd.recoverTransferQuote(W, l.txid, l.vout)).toEqual({ kind: "ready", credits: "30000000000", feeCredits: "56000000" });
      expect(await code(cmd.recoverTransfer(W, l.txid, l.vout, await creditsGrant("299999999999")))).toBe("platform.grant_invalid");
      expect(await cmd.recoverTransfer(W, l.txid, l.vout, await creditsGrant("30000000000"))).toEqual({ kind: "applied" });
      expect((await cmd.unfinishedTransfers(W)).locks).toEqual([]);
      const rows = (await cmd.platformActivity(W))!.filter((a) => a.kind === "recovered");
      expect(rows.map((a) => [a.state, a.credits])).toEqual([["done", String(30_000_000_000n - 56_000_000n)]]);
    });

    it("is off on mainnet: the quote and the recover are refused, and the lock stays", async () => {
      open("recovery-recoverable-mainnet");
      const [l] = (await cmd.unfinishedTransfers(W)).locks;
      expect(await cmd.recoverTransferQuote(W, l.txid, l.vout)).toEqual({ kind: "notApplied", error: "platform.feature_off" });
      expect(await cmd.recoverTransfer(W, l.txid, l.vout, await creditsGrant("30000000000"))).toEqual({ kind: "notApplied", error: "platform.feature_off" });
      expect((await cmd.unfinishedTransfers(W)).locks).toHaveLength(2);
    });

    it("quotes a below-fee lock too small and never recovers it; a lock that is not partly used is not recoverable", async () => {
      open("recovery-recoverable-small");
      const [, small] = (await cmd.unfinishedTransfers(W)).locks;
      expect(small.state).toBe("tooSmall");
      expect(await cmd.recoverTransferQuote(W, small.txid, small.vout)).toEqual({ kind: "tooSmall", credits: "40000000", feeCredits: "56000000" });
      expect(await cmd.recoverTransfer(W, small.txid, small.vout, await creditsGrant("40000000"))).toEqual({ kind: "notApplied", error: "credits.below_minimum" });
      open("recovery-one");
      const [plain] = (await cmd.unfinishedTransfers(W)).locks;
      expect(await cmd.recoverTransferQuote(W, plain.txid, plain.vout)).toEqual({ kind: "notApplied", error: "recovery.not_recoverable" });
    });
  });

  it("reads through the adapter", async () => {
    const backend = new IpcBackend("fixture");
    open("recovery-one");
    expect((await backend.unfinishedTransfers(W))?.locks).toHaveLength(1);
  });
});
