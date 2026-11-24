import { afterEach, describe, expect, it, vi } from "vitest";
import type { FinishReport } from "../api/generated/FinishReport";
import type { LockFinish } from "../api/generated/LockFinish";
import type { RegistrationQuote } from "../api/generated/RegistrationQuote";
import type { UnfinishedLock } from "../api/generated/UnfinishedLock";
import type { UnfinishedTransfers } from "../api/generated/UnfinishedTransfers";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { fakeVault } from "../tools/testApi";
import { onMoveSettled } from "../platform/move";
import type { RecoverQuote } from "../api/generated/RecoverQuote";
import { cardView, failureStale, lockKey, onRecoverySettled, RECOVERY_COPY as C, RecoveryModel, resultText, sheetRows, withoutPendingShields, type RecoveryState } from "./model";

const lock = (n: number, state: UnfinishedLock["state"], origin: UnfinishedLock["origin"] = "restored", duffs = 50_000_000): UnfinishedLock => ({
  txid: String(n).repeat(64),
  vout: 0,
  kind: "shielded",
  duffs,
  origin,
  state,
  remainingCredits: null,
});
/** This device's refused transfer to Shielded with `credits` left (#402). */
const partly = (n: number, state: "recoverable" | "tooSmall", credits: string): UnfinishedLock => ({ ...lock(n, state, "thisDevice"), remainingCredits: credits });
const fundLock = (n: number, state: UnfinishedLock["state"], duffs = 50_000_000): UnfinishedLock => ({ ...lock(n, state, "restored", duffs), kind: "platformAddress" });
const list = (locks: UnfinishedLock[], complete = true): UnfinishedTransfers => ({ locks, complete, unread: [] });
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
    expect(view(list([lock(1, "unsupported")]))).toBeNull();
    expect(view(list([lock(1, "tooSmall")]))).toBeNull();
  });

  it("is hidden while the list is incomplete", () => {
    expect(view(list([lock(1, "ready")], false))).toBeNull();
  });

  it("one ready lock: its amount and Finish", () => {
    const got = view(list([lock(1, "ready"), lock(2, "ready", "thisDevice")]));
    expect(got).toMatchObject({ kind: "ready", text: "50000000 duffs was funded on Core for Shielded, and this wallet hasn't seen it added on Platform. Finishing adds it at most once.", actions: ["finish"], lock: lock(1, "ready") });
  });

  it("several ready locks: the count and total, Finish all and Details", () => {
    const got = view(list([lock(1, "ready"), lock(2, "ready", "restored", 25_000_000), lock(3, "checking")]));
    expect(got).toMatchObject({ kind: "ready", text: "2 transfers to Shielded (75000000 duffs) were funded on Core, and this wallet hasn't seen them added on Platform. Finishing adds each at most once.", actions: ["finishAll", "details"] });
  });

  it("one ready Add from Dash wallet lock: its own copy, and Finish", () => {
    expect(view(list([fundLock(1, "ready")]))).toMatchObject({
      kind: "ready",
      text: "50000000 duffs was funded on Core to add to your Platform address, and this wallet hasn't seen it added on Platform. Finishing adds it at most once.",
      actions: ["finish"],
      lock: fundLock(1, "ready"),
    });
  });

  it("several Add from Dash wallet locks name the Platform address; both kinds name no one destination", () => {
    expect(view(list([fundLock(1, "ready"), fundLock(2, "ready", 25_000_000)]))?.text).toBe(
      "2 transfers to your Platform address (75000000 duffs) were funded on Core, and this wallet hasn't seen them added on Platform. Finishing adds each at most once.",
    );
    expect(view(list([lock(1, "ready"), fundLock(2, "ready", 25_000_000)]))).toMatchObject({
      text: "2 transfers (75000000 duffs) were funded on Core, and this wallet hasn't seen them added on Platform. Finishing adds each at most once.",
      actions: ["finishAll", "details"],
    });
  });

  it("the sheet names each kind, and its rows and result lines follow the lock", () => {
    expect(C.sheet.kind).toEqual({ shielded: "Shielded", platformAddress: "Add from Dash wallet" });
    expect(C.sheet.intro()).toBe("These transfers were funded on Core before the restore, and this wallet hasn't seen them added on Platform. Finishing adds each at most once.");
    expect(C.sheet.intro("shielded")).toBe(C.sheet.intro());
    expect(C.sheet.intro("platformAddress")).toBe("These transfers were funded on Core, and this wallet hasn't seen them added on Platform. Finishing adds each at most once.");
    expect(C.sheet.intro("mixed")).toBe(C.sheet.intro("platformAddress"));
    expect(resultText({ kind: "applied" }, errorText, "platformAddress")).toBe("Added to your Platform address");
    expect(resultText({ kind: "applied" }, errorText, "shielded")).toBe("Added to Shielded");
    const results = new Map([[lockKey(fundLock(1, "ready")), { lock: fundLock(1, "ready"), result: { kind: "applied" } as const }]]);
    expect(sheetRows(list([lock(2, "ready")]), results).map((r) => r.lock.kind)).toEqual(["shielded", "platformAddress"]);
  });

  it("checking locks only: Check again, singular and plural", () => {
    expect(view(list([lock(1, "checking")]))).toMatchObject({ kind: "checking", text: "1 transfer may already be finished. Choose Check again to confirm it. If its amount wasn't added yet, Check again tries to add it. If it still shows Checking, try again later.", actions: ["checkAgain", "details"] });
    expect(view(list([lock(1, "checking"), lock(2, "checking")]))?.text).toBe(
      "2 transfers may already be finished. Choose Check again to confirm them. If an amount wasn't added yet, Check again tries to add it. If any still show Checking, try again later.",
    );
  });

  it("names the Check again button by its own label in each Checking line", () => {
    const label = C.checkAgain;
    const lines = [C.checking(1), C.checking(2), C.sheet.done.checking];
    for (const line of lines) {
      expect(line).toContain(`Choose ${label} to confirm`);
      expect(line).toContain(`, ${label} tries to add it.`);
    }
  });

  it("running says Finishing…, whatever the list", () => {
    expect(view(undefined, { kind: "running" })).toMatchObject({ kind: "running", text: "Finishing…", actions: [] });
  });

  it("a failure shows its error with Retry and Dismiss while its lock is listed", () => {
    const state: RecoveryState = { kind: "failed", error: appError("platform.unavailable"), retry: { kind: "finish", lock: lock(1, "ready") } };
    expect(view(list([lock(1, "ready")]), state)).toMatchObject({ kind: "failed", text: "error platform.unavailable", actions: ["retry", "dismiss"] });
    expect(view(undefined, state)).toMatchObject({ kind: "failed" });
  });

  it("a failure whose lock has left the list is dropped", () => {
    const state: RecoveryState = { kind: "failed", error: appError("platform.unavailable"), retry: { kind: "finish", lock: lock(1, "ready") } };
    expect(failureStale(state, list([]))).toBe(true);
    expect(failureStale(state, list([lock(2, "ready")]))).toBe(true);
    expect(view(list([]), state)).toBeNull();
    const all: RecoveryState = { ...state, retry: { kind: "all" } };
    expect(view(list([]), all)).toMatchObject({ kind: "failed", actions: ["retry", "dismiss"] });
  });

  it("leaves out a lock the card already shows as a pending shield", () => {
    const data = list([lock(1, "ready"), lock(2, "ready")]);
    const kept = withoutPendingShields(data, [{ txid: lock(1, "ready").txid }]);
    expect(kept?.locks.map((l) => l.txid[0])).toEqual(["2"]);
    expect(view(withoutPendingShields(list([lock(1, "ready")]), [{ txid: lock(1, "ready").txid }]))).toBeNull();
    expect(withoutPendingShields(data, [])).toBe(data);
    expect(withoutPendingShields(data, null)).toBe(data);
  });

  it("a Finish all result is kept above the card, and alone once nothing is left", () => {
    const done: RecoveryState = { kind: "done", text: "2 resumed, 2 completed, 0 still pending." };
    expect(view(list([lock(1, "checking")]), done)).toMatchObject({ kind: "checking", note: done.text });
    expect(view(list([]), done)).toMatchObject({ kind: "done", text: done.text, actions: ["dismiss"] });
  });
});

describe("unfinished transfers card: refused transfers to Shielded (#402)", () => {
  it("one recoverable transfer, this device's or restored: what is left and Recover", () => {
    const one = partly(1, "recoverable", "300000000000");
    expect(view(list([one]))).toMatchObject({
      kind: "recoverable",
      text: "300000000 duffs is left on a transfer to Shielded that Platform refused. You can recover it to your Platform balance, minus Platform's fee.",
      actions: ["recover"],
      lock: one,
    });
  });

  it("counts only the rows at or above the fee; a below-fee row stays on the sheet", () => {
    const rows = [partly(1, "recoverable", "300000000000"), partly(2, "recoverable", "100000000000"), partly(3, "tooSmall", "40000000")];
    expect(view(list(rows))).toMatchObject({
      kind: "recoverable",
      text: "2 transfers to Shielded that Platform refused have 400000000 duffs left. You can recover each to your Platform balance, minus Platform's fee.",
      actions: ["details"],
      lock: null,
    });
    const withSmall = view(list([rows[0], rows[2]]));
    expect(withSmall).toMatchObject({ kind: "recoverable", actions: ["recover", "details"], lock: rows[0] });
    expect(withSmall?.text).toContain("300000000 duffs is left");
    expect(sheetRows(list([rows[0], rows[2]]), new Map()).map((r) => r.lock.state)).toEqual(["recoverable", "tooSmall"]);
  });

  it("shows no card when every refused transfer is below the fee", () => {
    expect(view(list([partly(3, "tooSmall", "40000000")]))).toBeNull();
    expect(view(list([partly(3, "tooSmall", "40000000"), partly(4, "tooSmall", "1")]))).toBeNull();
  });

  it("names its states and lines in MG's words", () => {
    expect(C.sheet.state.recoverable).toBe("Can be recovered");
    expect(C.recover).toBe("Recover");
    expect(C.recoverConfirm.title).toBe("Recover leftover value");
    expect(C.recoverConfirm.body("0.3 DASH")).toBe("Move the 0.3 DASH left on this transfer to your Platform balance? Platform's fee comes out of this amount.");
    expect([C.recoverConfirm.confirm, C.recoverConfirm.cancel]).toEqual(["Recover", "Cancel"]);
    expect(C.sheet.recoverTooSmall("0.0004 DASH")).toBe("The 0.0004 DASH left on this transfer is less than Platform's fee for moving it, so it can't be recovered.");
    expect(C.sheet.recoverFailed).toBe("Dash Wallet couldn't recover this value. It's still listed here, and you can try again.");
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

  it("keeps a lock the network reported already added, with its line", () => {
    const a = lock(1, "checking");
    const results = new Map([[lockKey(a), { lock: a, result: { kind: "reportedUsed" } as LockFinish }]]);
    const rows = sheetRows(list([lock(2, "checking")]), results);
    expect(rows.map((r) => r.lock.txid[0])).toEqual(["2", "1"]);
    expect(rows[1].result).toEqual({ kind: "reportedUsed" });
  });

  it("says what each finish came to", () => {
    expect(resultText({ kind: "applied" }, errorText)).toBe("Added to Shielded");
    expect(resultText({ kind: "checking" }, errorText)).toBe(
      "This transfer may already be finished. Choose Check again to confirm it. If its amount wasn't added yet, Check again tries to add it. If it still shows Checking, try again later.",
    );
    expect(resultText({ kind: "reportedUsed" }, errorText)).toBe("The network says this was already added. There's nothing left to finish.");
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
    finishAssetLocks: vi.fn(async (_: string, grant: string): Promise<FinishReport> => {
      calls.push(`all ${grant}`);
      return { resumed: 2, completed: 1, stillPending: 1, alreadyUsed: 1, byKind: [], unread: [] };
    }),
    finishTransfer: vi.fn(async (_: string, kind: string, txid: string, vout: number, grant: string) => {
      calls.push(`finish ${kind} ${txid[0]}:${vout} ${grant}`);
      const got = finish(txid, calls.length);
      if (got instanceof Error) throw got;
      return got;
    }),
    recoverTransferQuote: vi.fn(async (): Promise<RecoverQuote> => ({ kind: "finished" })),
    recoverTransfer: vi.fn(async (): Promise<LockFinish> => ({ kind: "applied" })),
  };
  return { model: new RecoveryModel(api, WALLET_A), api, calls, vault };
}

function recoverSetup(quote: RecoverQuote | Error, recovered: LockFinish | Error | ReturnType<typeof appError> = { kind: "applied" }) {
  const base = setup();
  const api = {
    ...base.api,
    recoverTransferQuote: vi.fn(async (_: string, txid: string, vout: number) => {
      base.calls.push(`quote ${txid[0]}:${vout}`);
      if (quote instanceof Error) throw quote;
      return quote;
    }),
    recoverTransfer: vi.fn(async (_: string, txid: string, vout: number, grant: string) => {
      base.calls.push(`recover ${txid[0]}:${vout} ${grant}`);
      if (recovered instanceof Error || "code" in recovered) throw recovered;
      return recovered;
    }),
  };
  return { ...base, api, model: new RecoveryModel(api, WALLET_A) };
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

  it("a finish that touched an Add from Dash wallet lock reads the Platform side again; a Shielded one does not", async () => {
    const moved = vi.fn();
    const stop = onMoveSettled(moved);
    const { model, calls } = setup();
    await model.finish(lock(1, "ready"));
    await Promise.resolve();
    expect(moved).not.toHaveBeenCalled();
    await model.finish(fundLock(2, "ready"));
    await Promise.resolve();
    expect(calls[1]).toBe("finish platformAddress 2:0 grant-2");
    expect(moved).toHaveBeenCalledTimes(1);
    await model.checkAgain([fundLock(3, "checking")]);
    await Promise.resolve();
    expect(moved).toHaveBeenCalledTimes(2);
    stop();
  });

  it("Finish all reads the Platform side again when the report has an Add from Dash wallet part", async () => {
    const moved = vi.fn();
    const stop = onMoveSettled(moved);
    const { model, api } = setup();
    await model.finishAll();
    await Promise.resolve();
    expect(moved).not.toHaveBeenCalled();
    api.finishAssetLocks.mockResolvedValueOnce({ resumed: 2, completed: 2, stillPending: 0, alreadyUsed: 0, byKind: [{ kind: "shielded", resumed: 1, completed: 1, stillPending: 0 }, { kind: "platformAddress", resumed: 1, completed: 1, stillPending: 0 }], unread: [] });
    await model.finishAll();
    await Promise.resolve();
    expect(moved).toHaveBeenCalledTimes(1);
    expect(model.state).toEqual({ kind: "done", text: "2 resumed, 2 completed, 0 still pending. Shielded: 1 completed. Add from Dash wallet: 1 completed." });
    stop();
  });

  it("Check again finishes each lock in turn, and a lock still checking stays", async () => {
    const { model, calls } = setup(() => ({ kind: "checking" }));
    await model.checkAgain([lock(1, "checking"), lock(2, "checking")]);
    expect(calls).toEqual(["finish shielded 1:0 grant-1", "finish shielded 2:0 grant-2"]);
    expect(model.state).toEqual({ kind: "idle" });
    expect(model.results.get(lockKey(lock(2, "checking")))?.result).toEqual({ kind: "checking" });
  });

  it("a single Finish or Check again the network answers already added leaves that line on the card", async () => {
    const reported = "The network says this was already added. There's nothing left to finish.";
    for (const run of [(m: RecoveryModel) => m.checkAgain([lock(1, "checking")]), (m: RecoveryModel) => m.finish(lock(1, "ready"))]) {
      const { model } = setup(() => ({ kind: "reportedUsed" }));
      await run(model);
      expect(model.state).toEqual({ kind: "done", text: reported });
      expect(view(list([]), model.state)).toMatchObject({ kind: "done", text: reported, actions: ["dismiss"] });
    }
    const { model } = setup((txid) => (txid[0] === "1" ? { kind: "reportedUsed" } : { kind: "checking" }));
    await model.checkAgain([lock(1, "checking"), lock(2, "checking")]);
    expect(model.state).toEqual({ kind: "idle" });
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

  it("settle drops a failure once its lock is gone, and Dismiss clears one", async () => {
    const { model } = setup(() => ({ kind: "notApplied", error: "platform.unavailable" }));
    await model.finish(lock(1, "ready"));
    model.settle(list([lock(1, "ready")]));
    expect(model.state.kind).toBe("failed");
    model.settle(list([]));
    expect(model.state).toEqual({ kind: "idle" });
    await model.finish(lock(1, "ready"));
    model.dismiss();
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

describe("RecoveryModel recover (#402)", () => {
  const one = partly(1, "recoverable", "300000000000");
  const READY: RecoverQuote = { kind: "ready", credits: "290000000000", feeCredits: "56000000" };

  it("reads Platform first, asks to confirm the fresh amount, then recovers under a grant of exactly those credits", async () => {
    const { model, calls, vault } = recoverSetup(READY);
    await model.askRecover(one);
    expect(model.asking).toEqual({ lock: one, credits: "290000000000" });
    expect(calls).toEqual(["quote 1:0"]);
    expect(vault.calls.authorize).toEqual([]);
    await model.confirmRecover();
    expect(vault.calls.authorize[0]).toMatchObject({ purpose: { kind: "platformOp", maxDuffs: 1, maxCredits: "290000000000" }, walletId: WALLET_A });
    expect(calls).toEqual(["quote 1:0", "recover 1:0 grant-1"]);
    expect(model.asking).toBeNull();
    expect(model.results.get(lockKey(one))).toEqual({ lock: one, result: { kind: "applied" }, recovered: true });
    expect(model.state).toEqual({ kind: "idle" });
  });

  it("Cancel recovers nothing", async () => {
    const { model, calls, api } = recoverSetup(READY);
    await model.askRecover(one);
    model.cancelRecover();
    expect(model.asking).toBeNull();
    expect(api.recoverTransfer).not.toHaveBeenCalled();
    expect(calls).toEqual(["quote 1:0"]);
  });

  it("a value now below the fee is never offered: the row says so", async () => {
    const { model, api } = recoverSetup({ kind: "tooSmall", credits: "40000000", feeCredits: "56000000" });
    await model.askRecover(one);
    expect(model.asking).toBeNull();
    expect(model.notes.get(lockKey(one))).toEqual({ kind: "tooSmall", credits: "40000000" });
    expect(api.recoverTransfer).not.toHaveBeenCalled();
  });

  it("a quote that fails, a recover that may have executed, or one that throws, leave the lock listed with the failure line", async () => {
    for (const [quote, recovered] of [
      [new Error("down"), undefined],
      [{ kind: "notApplied", error: "platform.unavailable" } as RecoverQuote, undefined],
      [READY, { kind: "checking" } as LockFinish],
      [READY, appError("platform.grant_invalid")],
    ] as const) {
      const { model } = recoverSetup(quote, recovered);
      await model.askRecover(one);
      if (model.asking) await model.confirmRecover();
      expect(model.notes.get(lockKey(one))).toEqual({ kind: "failed" });
      expect(model.results.has(lockKey(one))).toBe(false);
      expect(model.state).toEqual({ kind: "idle" });
    }
  });

  it("a lock already recovered or used up reads as added to the Platform address", async () => {
    const { model, api } = recoverSetup({ kind: "finished" });
    await model.askRecover(one);
    expect(model.results.get(lockKey(one))).toEqual({ lock: one, result: { kind: "applied" }, recovered: true });
    expect(api.recoverTransfer).not.toHaveBeenCalled();
  });
});
