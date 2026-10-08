import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import { bridgeSignals, invalidationsFor } from "./events";
import { FixtureBackend, FIXTURE_WALLET_ID } from "./fixture";
import { keys } from "./queries";

describe("event bridge", () => {
  it("maps each signal to the queries it touches", () => {
    expect(invalidationsFor({ kind: "history", network: "testnet", walletId: "w" })).toEqual([["history", "w"], ["txDetail", "w"]]);
    expect(invalidationsFor({ kind: "lockState", network: "testnet" })).toEqual([keys.lockStatus, keys.wallets]);
    expect(invalidationsFor({ kind: "session", network: "testnet", open: true })).toEqual([[]]);
  });

  it("invalidates the lock status when the fixture vault locks", async () => {
    const backend = new FixtureBackend(FixtureBackend.browserLaunch());
    const client = new QueryClient();
    client.setQueryData(keys.lockStatus, await backend.lockStatus());
    client.setQueryData(keys.balance(FIXTURE_WALLET_ID), await backend.balance(FIXTURE_WALLET_ID));
    await bridgeSignals(backend, client);
    await backend.lock();
    expect(client.getQueryState(keys.lockStatus)?.isInvalidated).toBe(true);
    expect(client.getQueryState(keys.balance(FIXTURE_WALLET_ID))?.isInvalidated).toBe(false);
  });
});
