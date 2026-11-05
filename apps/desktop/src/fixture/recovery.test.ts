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
    expect(await cmd.unfinishedTransfers(W)).toEqual({ locks: [], complete: true });
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

  it("Finish needs a platformOp grant of a duff, and a lock that is listed", async () => {
    open("recovery-one");
    const [l] = (await cmd.unfinishedTransfers(W)).locks;
    expect(await code(cmd.finishTransfer(W, l.kind, l.txid, l.vout, "no-such-grant"))).toBe("platform.grant_invalid");
    expect(await code(cmd.finishTransfer(W, l.kind, l.txid, l.vout, await grant(0)))).toBe("platform.grant_invalid");
    expect(await code(cmd.finishTransfer(W, l.kind, "00".repeat(32), 0, await grant()))).toBe("invalid_argument");
    expect((await cmd.unfinishedTransfers(W)).locks).toHaveLength(1);
  });

  it("a checking lock stays checking, a confirming one waits, a not-active one is refused", async () => {
    const finish = async (s: ScenarioName) => {
      open(s);
      const [l] = (await cmd.unfinishedTransfers(W)).locks;
      return cmd.finishTransfer(W, l.kind, l.txid, l.vout, await grant());
    };
    expect(await finish("recovery-checking")).toEqual({ kind: "checking" });
    expect(await finish("recovery-confirming")).toEqual({ kind: "proofWaiting" });
    expect(await finish("recovery-not-active")).toEqual({ kind: "notApplied", error: "shielded.not_active" });
  });

  it("recovery-finish-fails: the first finish is not applied, the retry is", async () => {
    open("recovery-finish-fails");
    const [l] = (await cmd.unfinishedTransfers(W)).locks;
    expect(await cmd.finishTransfer(W, l.kind, l.txid, l.vout, await grant())).toEqual({ kind: "notApplied", error: "platform.unavailable" });
    expect((await cmd.unfinishedTransfers(W)).locks).toHaveLength(1);
    expect(await cmd.finishTransfer(W, l.kind, l.txid, l.vout, await grant())).toEqual({ kind: "applied" });
    expect((await cmd.unfinishedTransfers(W)).locks).toEqual([]);
  });

  it("Finish transfers applies the ready restored locks and reports them by kind", async () => {
    open("recovery-restored-mixed");
    expect(await cmd.finishAssetLocks(W, await grant())).toEqual({
      resumed: 3,
      completed: 2,
      stillPending: 1,
      alreadyUsed: 0,
      byKind: [{ kind: "shielded", resumed: 3, completed: 2, stillPending: 1 }],
    });
    expect((await cmd.unfinishedTransfers(W)).locks.map((l) => l.state)).toEqual(["checking"]);
  });

  it("reads through the adapter", async () => {
    const backend = new IpcBackend("fixture");
    open("recovery-one");
    expect((await backend.unfinishedTransfers(W))?.locks).toHaveLength(1);
  });
});
