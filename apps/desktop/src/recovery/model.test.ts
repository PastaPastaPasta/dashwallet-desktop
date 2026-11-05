import { afterEach, describe, expect, it, vi } from "vitest";
import type { LockFinish } from "../api/generated/LockFinish";
import type { RegistrationQuote } from "../api/generated/RegistrationQuote";
import type { UnfinishedLock } from "../api/generated/UnfinishedLock";
import type { UnfinishedTransfers } from "../api/generated/UnfinishedTransfers";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { fakeVault } from "../tools/testApi";
import { cardView, lockKey, onRecoverySettled, RECOVERY_COPY as C, RecoveryModel, resultText, sheetRows, type RecoveryState } from "./model";

const lock = (n: number, state: UnfinishedLock["state"], origin: UnfinishedLock["origin"] = "restored", duffs = 50_000_000): UnfinishedLock => ({
  txid: String(n).repeat(64),
  vout: 0,
  kind: "shielded",
  duffs,
  origin,
  state,
});
const list = (locks: UnfinishedLock[], complete = true): UnfinishedTransfers => ({ locks, complete });
const IDLE: RecoveryState = { kind: "idle" };
const duffs = (d: number) => `${d} duffs`;
const errorText = (e: unknown) => `error ${(e as { code: string }).code}`;
const view = (data: UnfinishedTransfers | null | undefined, state: RecoveryState = IDLE) => cardView(data, state, duffs, errorText);

describe("unfinished transfers card", () => {
  it("is hidden without restored locks that are ready or checking", () => {
    expect(view(undefined)).toBeNull();
    expect(view(null)).toBeNull();
    expect(view(list([]))).toBeNull();
    expect(view(list([lock(1, "ready", "thisDevice"), lock(2, "checking", "thisDevice")]))).toBeNull();
    expect(view(list([lock(1, "confirming")]))).toBeNull();
    expect(view(list([lock(1, "notActive")]))).toBeNull();
    expect(view(list([lock(1, "reportedUsed")]))).toBeNull();
  });

  it("is hidden while the list is incomplete", () => {
    expect(view(list([lock(1, "ready")], false))).toBeNull();
  });

  it("one ready lock: its amount and Finish", () => {
    const got = view(list([lock(1, "ready"), lock(2, "ready", "thisDevice")]));
    expect(got).toMatchObject({ kind: "ready", text: "50000000 duffs was locked for your Shielded balance and isn't added yet.", actions: ["finish"], lock: lock(1, "ready") });
  });

  it("several ready locks: the count and total, Finish all and Details", () => {
    const got = view(list([lock(1, "ready"), lock(2, "ready", "restored", 25_000_000), lock(3, "checking")]));
    expect(got).toMatchObject({ kind: "ready", text: "2 transfers from before the restore aren't finished (75000000 duffs).", actions: ["finishAll", "details"] });
  });

  it("checking locks only: Check again, singular and plural", () => {
    expect(view(list([lock(1, "checking")]))).toMatchObject({ kind: "checking", text: "1 transfer may already be finished. We can't confirm it yet.", actions: ["checkAgain", "details"] });
    expect(view(list([lock(1, "checking"), lock(2, "checking")]))?.text).toBe("2 transfers may already be finished. We can't confirm it yet.");
  });

  it("running says Finishing…, whatever the list", () => {
    expect(view(undefined, { kind: "running" })).toMatchObject({ kind: "running", text: "Finishing…", actions: [] });
  });

  it("a failure shows its error and Retry, even when the list is empty", () => {
    const state: RecoveryState = { kind: "failed", error: appError("platform.unavailable"), retry: { kind: "all" } };
    expect(view(list([]), state)).toMatchObject({ kind: "failed", text: "error platform.unavailable", actions: ["retry"] });
  });

  it("a Finish all result is kept above the card, and alone once nothing is left", () => {
    const done: RecoveryState = { kind: "done", text: "2 resumed, 2 completed, 0 still pending." };
    expect(view(list([lock(1, "checking")]), done)).toMatchObject({ kind: "checking", note: done.text });
    expect(view(list([]), done)).toMatchObject({ kind: "done", text: done.text, actions: ["dismiss"] });
  });
});

describe("unfinished transfers sheet", () => {
  it("lists the restored locks and keeps the ones applied in this session", () => {
    const a = lock(1, "ready");
    const results = new Map([[lockKey(a), { lock: a, result: { kind: "applied" } as LockFinish }]]);
    const rows = sheetRows(list([lock(2, "checking"), lock(3, "ready", "thisDevice"), lock(4, "confirming")]), results);
    expect(rows.map((r) => r.lock.txid[0])).toEqual(["2", "4", "1"]);
    expect(rows[2].result).toEqual({ kind: "applied" });
  });

  it("says what each finish came to", () => {
    expect(resultText({ kind: "applied" }, errorText)).toBe("Added to Shielded");
    expect(resultText({ kind: "checking" }, errorText)).toBe("Checking: this amount may already be in your balance; we can't confirm it yet.");
    expect(resultText({ kind: "reportedUsed" }, errorText)).toBe(C.sheet.done.checking);
    expect(resultText({ kind: "proofWaiting" }, errorText)).toBe("Waiting for the network to confirm");
    expect(resultText({ kind: "notApplied", error: "platform.unavailable" }, errorText)).toBe("error platform.unavailable");
  });
});

const QUOTE = { grant: { maxCredits: "2000000000" } } as unknown as RegistrationQuote;

function setup(finish: (txid: string, n: number) => LockFinish | Error = () => ({ kind: "applied" }), lockStatus: Parameters<typeof fakeVault>[0] = { encrypted: false }) {
  const vault = fakeVault(lockStatus);
  const calls: string[] = [];
  const api = {
    ...vault.api,
    registrationQuote: vi.fn(async () => QUOTE),
    finishAssetLocks: vi.fn(async (_: string, grant: string) => {
      calls.push(`all ${grant}`);
      return { resumed: 2, completed: 1, stillPending: 1, alreadyUsed: 1, byKind: [] };
    }),
    finishTransfer: vi.fn(async (_: string, kind: string, txid: string, vout: number, grant: string) => {
      calls.push(`finish ${kind} ${txid[0]}:${vout} ${grant}`);
      const got = finish(txid, calls.length);
      if (got instanceof Error) throw got;
      return got;
    }),
  };
  return { model: new RecoveryModel(api, WALLET_A), api, calls, vault };
}

describe("RecoveryModel", () => {
  afterEach(() => vi.restoreAllMocks());

  it("Finish asks for a platformOp grant of one duff and no credits, and records the result", async () => {
    const { model, calls, vault } = setup();
    const settled = vi.fn();
    const stop = onRecoverySettled(settled);
    await model.finish(lock(1, "ready"));
    stop();
    expect(vault.calls.authorize[0]).toMatchObject({ purpose: { kind: "platformOp", maxDuffs: 1, maxCredits: "0" }, walletId: WALLET_A });
    expect(calls).toEqual(["finish shielded 1:0 grant-1"]);
    expect(model.state).toEqual({ kind: "idle" });
    expect(model.results.get(lockKey(lock(1, "ready")))?.result).toEqual({ kind: "applied" });
    await Promise.resolve();
    expect(settled).toHaveBeenCalledTimes(1);
  });

  it("Finish all is Repair's Finish transfers with its grant, and reports the result", async () => {
    const { model, calls, vault } = setup();
    await model.finishAll();
    expect(vault.calls.authorize[0]).toMatchObject({ purpose: { kind: "platformOp", maxDuffs: 1, maxCredits: "2000000000" } });
    expect(calls).toEqual(["all grant-1"]);
    expect(model.state).toEqual({ kind: "done", text: "2 resumed, 1 completed, 1 still pending, 1 already used." });
    model.dismiss();
    expect(model.state).toEqual({ kind: "idle" });
  });

  it("Check again finishes each lock in turn, and a lock still checking stays", async () => {
    const { model, calls } = setup(() => ({ kind: "checking" }));
    await model.checkAgain([lock(1, "checking"), lock(2, "checking")]);
    expect(calls).toEqual(["finish shielded 1:0 grant-1", "finish shielded 2:0 grant-2"]);
    expect(model.state).toEqual({ kind: "idle" });
    expect(model.results.get(lockKey(lock(2, "checking")))?.result).toEqual({ kind: "checking" });
  });

  it("a notApplied lock fails the card, stops the rest, and Retry runs it again", async () => {
    let n = 0;
    const { model, calls } = setup(() => (++n === 1 ? { kind: "notApplied", error: "platform.unavailable" } : { kind: "applied" }));
    await model.checkAgain([lock(1, "checking"), lock(2, "checking")]);
    expect(model.state).toMatchObject({ kind: "failed", error: { code: "platform.unavailable" } });
    expect(calls).toHaveLength(1);
    await model.retry();
    expect(calls).toHaveLength(3);
    expect(model.state).toEqual({ kind: "idle" });
  });

  it("a thrown error fails the card and Retry repeats Finish all", async () => {
    const { model, api, calls } = setup();
    api.finishAssetLocks.mockRejectedValueOnce(appError("platform.unavailable"));
    await model.finishAll();
    expect(model.state).toMatchObject({ kind: "failed", error: { code: "platform.unavailable" } });
    await model.retry();
    expect(calls).toHaveLength(1);
    expect(model.state.kind).toBe("done");
  });

  it("a cancelled passphrase ends quietly", async () => {
    const { model, calls, api } = setup(undefined, { encrypted: true, state: "locked" });
    const running = model.finish(lock(1, "ready"));
    await vi.waitFor(() => expect(model.grants.pending).not.toBeNull());
    model.grants.cancel();
    await running;
    expect(model.state).toEqual({ kind: "idle" });
    expect(calls).toEqual([]);
    expect(api.finishTransfer).not.toHaveBeenCalled();
  });
});
