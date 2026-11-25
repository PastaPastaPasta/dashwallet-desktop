import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "./backend";
import type { LockStatus } from "./generated/LockStatus";
import {
  closeWalletGate,
  CONFIRM_DEADLINE_MS,
  CONFIRM_RETRY_MAX_MS,
  CONFIRM_RETRY_MS,
  confirmLockState,
  disposeConfirm,
  followLockStatus,
  forgetWalletContent,
  freshLockStatus,
  guardWhileLocked,
  keptWhileLocked,
  lockGeneration,
  lockStateChanged,
  lockUnconfirmed,
  refusedWhileLocked,
  resetWalletGate,
  seedLockStatus,
  subscribeWalletGate,
  walletGateClosed,
  walletNamesForRecovery,
} from "./lockGate";
import { keys } from "./queries";

const STATUS: LockStatus = { state: "unlocked", encrypted: true, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSecs: null, walletsWithSecrets: [] };

beforeEach(() => resetWalletGate());
afterEach(() => resetWalletGate());

describe("forgetWalletContent", () => {
  it("keeps the lock screen's reads and the network, and drops everything else", () => {
    const client = new QueryClient();
    const kept = [keys.network, keys.lockStatus, keys.lastLockReport, [...keys.lockReconcile, { flows: [] }], keys.lockDrain];
    const dropped = [keys.wallets, keys.balance("w"), keys.history("w", {} as never, 20), keys.receiveAddress("w"), keys.contactsAll("w"), keys.sync, ["qr", "dash:x"]];
    for (const k of [...kept, ...dropped]) client.setQueryData(k, "x");
    forgetWalletContent(client);
    for (const k of kept) expect(client.getQueryData(k)).toBe("x");
    for (const k of dropped) expect(client.getQueryData(k)).toBeUndefined();
    expect(keptWhileLocked(keys.lockStatus)).toBe(true);
    expect(keptWhileLocked(keys.wallets)).toBe(false);
  });

  it("a read under way when the lock comes does not put its answer back", async () => {
    const client = new QueryClient();
    let answer!: (v: string) => void;
    const read = client.fetchQuery({ queryKey: keys.balance("w"), queryFn: () => new Promise<string>((r) => (answer = r)) }).catch(() => "cancelled");
    forgetWalletContent(client);
    answer("32.04698287");
    expect(await read).toBe("cancelled");
    expect(client.getQueryData(keys.balance("w"))).toBeUndefined();
    expect(client.getQueryCache().find({ queryKey: keys.balance("w") })).toBeUndefined();
  });
});

describe("the lock gate", () => {
  it("closes at once on the lock, says locked meanwhile, and opens only once a status read says unlocked", () => {
    const client = new QueryClient();
    client.setQueryData(keys.lockStatus, STATUS);
    client.setQueryData(keys.balance("w"), "x");
    const stop = followLockStatus(client);
    const seen: boolean[] = [];
    const unsubscribe = subscribeWalletGate(() => seen.push(walletGateClosed()));
    expect(walletGateClosed()).toBe(false);
    closeWalletGate(client);
    expect(walletGateClosed()).toBe(true);
    expect(client.getQueryData<LockStatus>(keys.lockStatus)?.state).toBe("locked");
    expect(client.getQueryData(keys.balance("w"))).toBeUndefined();
    client.setQueryData(keys.lockStatus, { ...STATUS, state: "locked" });
    expect(walletGateClosed()).toBe(true);
    // A status read before the lock, written after it, does not open the gate.
    client.setQueryData(keys.lockStatus, STATUS);
    expect(walletGateClosed()).toBe(true);
    seedLockStatus(client, { ...STATUS, state: "unlockedMixingOnly" }, lockGeneration());
    expect(walletGateClosed()).toBe(false);
    expect(seen).toEqual([true, false]);
    unsubscribe();
    stop();
  });

  it("follows a status that starts locked", () => {
    const client = new QueryClient();
    client.setQueryData(keys.lockStatus, { ...STATUS, state: "locked" });
    const stop = followLockStatus(client);
    expect(walletGateClosed()).toBe(true);
    stop();
  });

  it("refuses every wallet command while closed without asking the engine, and lets the locked window's through", async () => {
    const client = new QueryClient();
    const asked: string[] = [];
    const ask = (name: string) => async () => (asked.push(name), name);
    const backend = guardWhileLocked({ kind: "fixture", balance: ask("balance"), draftBroadcast: ask("draftBroadcast"), unlock: ask("unlock"), lockStatus: ask("lockStatus"), onEvent: ask("onEvent") } as unknown as Backend);
    expect(await backend.balance("w")).toBe("balance");
    closeWalletGate(client);
    const refused = await backend.draftBroadcast(1, 2).catch((e: unknown) => e);
    expect(refusedWhileLocked(refused)).toBe(true);
    expect(refused).toMatchObject({ code: "vault.locked" });
    await expect(backend.balance("w")).rejects.toMatchObject({ code: "vault.locked" });
    expect(await backend.unlock({ kind: "passphrase", passphrase: "p" } as never)).toBe("unlock");
    expect(await backend.lockStatus()).toBe("lockStatus");
    expect(backend.kind).toBe("fixture");
    expect(asked).toEqual(["balance", "unlock", "lockStatus"]);
    expect(refusedWhileLocked({ code: "vault.locked", message: "", data: null })).toBe(false);
  });

  it("lets Quit and Restart through a gate that never opened, as on a refused or held store", async () => {
    resetWalletGate(true);
    const asked: string[] = [];
    const ask = (name: string) => async () => (asked.push(name), null);
    const backend = guardWhileLocked({ quitApp: ask("quitApp"), restartApp: ask("restartApp"), balance: ask("balance") } as unknown as Backend);
    await expect(backend.balance("w")).rejects.toMatchObject({ code: "vault.locked" });
    expect(await backend.quitApp()).toBeNull();
    expect(await backend.restartApp()).toBeNull();
    expect(asked).toEqual(["quitApp", "restartApp"]);
  });

  it("stays closed from the start until a status says the vault is not locked", () => {
    resetWalletGate(true);
    const client = new QueryClient();
    const stop = followLockStatus(client);
    expect(walletGateClosed()).toBe(true);
    seedLockStatus(client, STATUS, lockGeneration());
    expect(walletGateClosed()).toBe(false);
    stop();
  });

  it("keeps closed when the network's opening status was read before a lock event", () => {
    resetWalletGate(true);
    const client = new QueryClient();
    const stop = followLockStatus(client);
    const since = lockGeneration();
    lockStateChanged();
    closeWalletGate(client);
    seedLockStatus(client, STATUS, since);
    expect(walletGateClosed()).toBe(true);
    expect(client.getQueryData(keys.lockStatus)).toBeUndefined();
    stop();
  });

  it("does not close for a lock call that answers after a newer unlock", () => {
    const client = new QueryClient();
    client.setQueryData(keys.lockStatus, STATUS);
    const stop = followLockStatus(client);
    const since = lockGeneration();
    lockStateChanged();
    closeWalletGate(client);
    lockStateChanged();
    seedLockStatus(client, STATUS, lockGeneration());
    expect(walletGateClosed()).toBe(false);
    closeWalletGate(client, since);
    expect(walletGateClosed()).toBe(false);
    const now = lockGeneration();
    closeWalletGate(client, now);
    expect(walletGateClosed()).toBe(true);
    stop();
  });

  it("reads the status again when the lock state changed during the read", async () => {
    const answers: LockStatus[] = [STATUS, { ...STATUS, state: "locked" }];
    let reads = 0;
    const status = await freshLockStatus(async () => {
      const s = answers[reads++];
      if (reads === 1) lockStateChanged();
      return s;
    });
    expect(reads).toBe(2);
    expect(status.state).toBe("locked");
  });

  /** A wallet command through the guard, and what reached the engine. */
  const guarded = () => {
    const asked: string[] = [];
    const backend = guardWhileLocked({ balance: async () => (asked.push("balance"), 1) } as unknown as Backend);
    return { asked, balance: () => backend.balance("w").catch((e: unknown) => e) };
  };
  /** A status read answered or refused by the test, one call at a time. */
  const reads = () => {
    const calls: { answer: (s: LockStatus) => void; fail: (e: unknown) => void }[] = [];
    const read = () => new Promise<LockStatus>((answer, fail) => calls.push({ answer, fail }));
    return { calls, read };
  };

  /** Lets the replies already given reach the gate. */
  const settle = () => vi.advanceTimersByTimeAsync(0);
  const withTimers = (body: () => Promise<void>) => async () => {
    vi.useFakeTimers();
    try {
      await body();
      disposeConfirm();
    } finally {
      vi.useRealTimers();
    }
  };

  it(
    "lost events close the gate and forget the wallet at once, refuse commands without reading again, and open on a read that succeeds",
    withTimers(async () => {
      const client = new QueryClient();
      client.setQueryData(keys.lockStatus, STATUS);
      client.setQueryData(keys.balance("w"), "x");
      const stop = followLockStatus(client);
      const { asked, balance } = guarded();
      const { calls, read } = reads();
      confirmLockState(client, read);
      expect(walletGateClosed()).toBe(true);
      expect(client.getQueryData(keys.balance("w"))).toBeUndefined();
      expect(client.getQueryData<LockStatus>(keys.lockStatus)?.state).toBe("locked");
      expect(refusedWhileLocked(await balance())).toBe(true);
      expect(asked).toEqual([]);
      expect(calls).toHaveLength(1);
      calls[0].answer(STATUS);
      await settle();
      expect(walletGateClosed()).toBe(false);
      expect(lockUnconfirmed()).toBe(false);
      expect(await balance()).toBe(1);
      expect(asked).toEqual(["balance"]);
      stop();
    }),
  );

  it(
    "7: a read that never settles fails at its deadline, the gate stays closed, the next read comes after the wait, and the late reply is ignored",
    withTimers(async () => {
      const client = new QueryClient();
      client.setQueryData(keys.lockStatus, STATUS);
      const { calls, read } = reads();
      confirmLockState(client, read);
      await vi.advanceTimersByTimeAsync(CONFIRM_DEADLINE_MS - 1);
      expect(calls).toHaveLength(1);
      await vi.advanceTimersByTimeAsync(1);
      expect(walletGateClosed()).toBe(true);
      expect(calls).toHaveLength(1);
      await vi.advanceTimersByTimeAsync(CONFIRM_RETRY_MS);
      expect(calls).toHaveLength(2);
      calls[0].answer(STATUS);
      await settle();
      expect(walletGateClosed()).toBe(true);
      expect(lockUnconfirmed()).toBe(true);
      calls[1].answer(STATUS);
      await settle();
      expect(walletGateClosed()).toBe(false);
    }),
  );

  it(
    "7: failed reads wait twice as long each time up to the cap, and no command, event or further loss cuts a wait short",
    withTimers(async () => {
      const client = new QueryClient();
      const { balance } = guarded();
      const { calls, read } = reads();
      confirmLockState(client, read);
      let wait = CONFIRM_RETRY_MS;
      for (let i = 0; i < 8; i++) {
        calls[i].fail(new Error("no answer"));
        await vi.advanceTimersByTimeAsync(wait - 1);
        expect(refusedWhileLocked(await balance())).toBe(true);
        confirmLockState(client, read);
        expect(calls).toHaveLength(i + 1);
        await vi.advanceTimersByTimeAsync(1);
        expect(calls).toHaveLength(i + 2);
        wait = Math.min(CONFIRM_RETRY_MAX_MS, wait * 2);
      }
      expect(wait).toBe(CONFIRM_RETRY_MAX_MS);
      expect(walletGateClosed()).toBe(true);
    }),
  );

  it(
    "7: lost events while closed, the read failing and then succeeding unlocked, open the gate",
    withTimers(async () => {
      resetWalletGate(true);
      const client = new QueryClient();
      const { calls, read } = reads();
      confirmLockState(client, read);
      calls[0].fail(new Error("no answer"));
      await vi.advanceTimersByTimeAsync(CONFIRM_RETRY_MS);
      expect(walletGateClosed()).toBe(true);
      calls[1].answer(STATUS);
      await settle();
      expect(walletGateClosed()).toBe(false);
      expect(client.getQueryData(keys.lockStatus)).toEqual(STATUS);
    }),
  );

  it(
    "8: an unlocked reply whose generation a lock moved before it lands is read again, and the gate stays closed",
    withTimers(async () => {
      const client = new QueryClient();
      client.setQueryData(keys.lockStatus, STATUS);
      const stop = followLockStatus(client);
      const { calls, read } = reads();
      confirmLockState(client, read);
      // The reply is in, and a lock completes before the gate hears of it.
      calls[0].answer(STATUS);
      lockStateChanged();
      closeWalletGate(client);
      await settle();
      expect(walletGateClosed()).toBe(true);
      expect(calls).toHaveLength(2);
      calls[1].answer({ ...STATUS, state: "locked" });
      await settle();
      expect(walletGateClosed()).toBe(true);
      expect(lockUnconfirmed()).toBe(false);
      expect(client.getQueryData<LockStatus>(keys.lockStatus)?.state).toBe("locked");
      stop();
    }),
  );

  it("8: a status query accepted before a lock and written after it does not open the gate", async () => {
    resetWalletGate(true);
    const client = new QueryClient();
    const stop = followLockStatus(client);
    const status = await freshLockStatus(async () => STATUS);
    closeWalletGate(client);
    client.setQueryData(keys.lockStatus, status);
    expect(walletGateClosed()).toBe(true);
    stop();
  });

  it("5: a lock state change between a status read's acceptance and the query's write opens nothing, and a read begun since does", async () => {
    resetWalletGate(true);
    const client = new QueryClient();
    const stop = followLockStatus(client);
    let reads = 0;
    const read = async () => (reads++, STATUS);
    // freshLockStatus accepts the reply in its generation; the engine's event lands before the query writes it.
    await client.fetchQuery({ queryKey: keys.lockStatus, queryFn: () => freshLockStatus(read).then((s) => (lockStateChanged(), s)) });
    expect(reads).toBe(1);
    expect(client.getQueryData<LockStatus>(keys.lockStatus)?.state).toBe("unlocked");
    expect(walletGateClosed()).toBe(true);
    expect(client.getQueryState(keys.lockStatus)?.isInvalidated).toBe(true);
    await client.fetchQuery({ queryKey: keys.lockStatus, queryFn: () => freshLockStatus(read) });
    expect(reads).toBe(2);
    expect(walletGateClosed()).toBe(false);
    stop();
  });

  it(
    "9: a newer loss during a read is answered by a read begun after it, never two at once",
    withTimers(async () => {
      const client = new QueryClient();
      client.setQueryData(keys.lockStatus, STATUS);
      const { calls, read } = reads();
      confirmLockState(client, read);
      confirmLockState(client, read);
      expect(calls).toHaveLength(1);
      calls[0].answer(STATUS);
      await settle();
      expect(walletGateClosed()).toBe(true);
      expect(calls).toHaveLength(2);
      calls[1].answer(STATUS);
      await settle();
      expect(walletGateClosed()).toBe(false);
    }),
  );

  it(
    "9: a status read since the loss opens the gate and ends the confirmation; its own late reply then counts for nothing",
    withTimers(async () => {
      const client = new QueryClient();
      client.setQueryData(keys.lockStatus, STATUS);
      const stop = followLockStatus(client);
      const { calls, read } = reads();
      confirmLockState(client, read);
      seedLockStatus(client, STATUS, lockGeneration());
      expect(walletGateClosed()).toBe(false);
      expect(lockUnconfirmed()).toBe(false);
      calls[0].answer({ ...STATUS, state: "locked" });
      await settle();
      expect(walletGateClosed()).toBe(false);
      expect(vi.getTimerCount()).toBe(0);
      stop();
    }),
  );

  it(
    "9: the confirmation ends with its bridge: no timer is left, and a reply after opens nothing",
    withTimers(async () => {
      const client = new QueryClient();
      const { calls, read } = reads();
      confirmLockState(client, read);
      calls[0].fail(new Error("no answer"));
      await settle();
      expect(vi.getTimerCount()).toBe(1);
      disposeConfirm();
      expect(vi.getTimerCount()).toBe(0);
      expect(lockUnconfirmed()).toBe(false);
      confirmLockState(client, read);
      disposeConfirm();
      calls[1].answer(STATUS);
      await settle();
      expect(walletGateClosed()).toBe(true);
      expect(vi.getTimerCount()).toBe(0);
    }),
  );

  it("lets Forgot passphrase read the wallets' names alone while locked, and nothing else read them", async () => {
    const client = new QueryClient();
    const summary = { walletId: "w", name: "Main", balance: { confirmed: 5 }, network: "testnet" };
    const backend = guardWhileLocked({ listWallets: async () => [summary] } as unknown as Backend);
    closeWalletGate(client);
    await expect(backend.listWallets()).rejects.toMatchObject({ code: "vault.locked" });
    expect(await walletNamesForRecovery(backend)).toEqual([{ walletId: "w", name: "Main" }]);
  });
});
