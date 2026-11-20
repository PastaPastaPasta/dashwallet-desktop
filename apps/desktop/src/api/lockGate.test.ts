import { QueryClient } from "@tanstack/react-query";
import { afterEach, describe, expect, it } from "vitest";
import type { Backend } from "./backend";
import type { LockStatus } from "./generated/LockStatus";
import { closeWalletGate, followLockStatus, forgetWalletContent, guardWhileLocked, keptWhileLocked, refusedWhileLocked, resetWalletGate, subscribeWalletGate, walletGateClosed } from "./lockGate";
import { keys } from "./queries";

const STATUS: LockStatus = { state: "unlocked", encrypted: true, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSecs: null, walletsWithSecrets: [] };

afterEach(resetWalletGate);

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
    client.setQueryData(keys.lockStatus, { ...STATUS, state: "unlockedMixingOnly" });
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
});
