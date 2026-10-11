import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import type { InfiniteData } from "@tanstack/react-query";
import { bridgeEvents, invalidationsFor } from "./events";
import type { Backend } from "./backend";
import type { AppEvent } from "./generated/commands";
import type { HistoryRows } from "./generated/HistoryRows";
import { FixtureBackend, FIXTURE_WALLET_ID } from "./fixture";
import { keys } from "./queries";

describe("event bridge", () => {
  it("maps each signal to the queries it touches", () => {
    expect(invalidationsFor({ kind: "historyChanged", network: "testnet", walletId: "w", txids: [] })).toEqual([["history", "w"], ["txDetail", "w"]]);
    expect(invalidationsFor({ kind: "vaultLockState", network: "testnet", state: "locked" })).toEqual([keys.lockStatus, keys.wallets]);
    expect(invalidationsFor({ kind: "sessionOpened", network: "testnet" })).toBe("all");
    expect(invalidationsFor({ kind: "eventsOverflowed" })).toBe("all");
    expect(invalidationsFor({ kind: "lockProgress", network: "testnet", progress: { kind: "draining", inFlight: 1, deadlineInMs: 500 } })).toEqual([]);
    expect(invalidationsFor({ kind: "platform", network: "testnet", walletId: "w", change: { domain: "names" } })).toEqual([]);
  });

  it("invalidates the lock status when the fixture vault locks", async () => {
    const backend = new FixtureBackend(FixtureBackend.browserLaunch());
    const client = new QueryClient();
    client.setQueryData(keys.lockStatus, await backend.lockStatus());
    client.setQueryData(keys.balance(FIXTURE_WALLET_ID), await backend.balance(FIXTURE_WALLET_ID));
    await bridgeEvents(backend, client);
    await backend.lock();
    expect(client.getQueryState(keys.lockStatus)?.isInvalidated).toBe(true);
    expect(client.getQueryState(keys.balance(FIXTURE_WALLET_ID))?.isInvalidated).toBe(false);
  });

  it("cuts a loaded history list back to its first page before refetching", async () => {
    const client = new QueryClient();
    const key = keys.history(FIXTURE_WALLET_ID, { categories: [], text: null, sort: "newestFirst" }, 2);
    const page = { rows: [], nextCursor: "x", totalMatching: 4 };
    client.setQueryData<InfiniteData<HistoryRows, string | null>>(key, { pages: [page, page], pageParams: [null, "2"] });
    let emit: (e: AppEvent) => void = () => undefined;
    const fake = { onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined) } as unknown as Backend;
    await bridgeEvents(fake, client);
    emit({ kind: "historyChanged", network: "testnet", walletId: FIXTURE_WALLET_ID, txids: [] });
    expect(client.getQueryData<InfiniteData<HistoryRows, string | null>>(key)?.pages).toHaveLength(1);
    expect(client.getQueryState(key)?.isInvalidated).toBe(true);
  });
});
