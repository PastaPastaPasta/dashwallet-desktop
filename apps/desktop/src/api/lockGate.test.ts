import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import { forgetWalletContent, keptWhileLocked } from "./lockGate";
import { keys } from "./queries";

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
