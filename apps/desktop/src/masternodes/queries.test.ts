// The masternode wrappers (MG-10 S1): each Backend method sends its `mn_*` command with the
// arguments by name, and the query keys put engine-local reads under the `masternodes` prefix and
// Platform's credits outside it (§3.1).

import { QueryClient } from "@tanstack/react-query";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invalidationsFor } from "../api/events";
import { keys } from "../api/queries";
import { IpcBackend } from "../api/tauri";
import { contestedNamesKey } from "../governance/queries";
import { invalidateAfterKeyChange, mnKeys } from "./queries";

const W = "a1".repeat(32);
const H = "4e".repeat(32);

beforeEach(() => vi.stubGlobal("window", globalThis));
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("masternode wrappers", () => {
  it("send each command with its arguments", async () => {
    const sent: [string, unknown][] = [];
    mockIPC((cmd, payload) => {
      sent.push([cmd, payload ?? {}]);
      return null;
    });
    const b = new IpcBackend("fixture");
    await b.mnListState();
    await b.mnList(W);
    await b.mnMasternode(W, H);
    await b.mnPayouts(W, H, 1_700_000_000);
    await b.mnFind("203.0.113.7:19999");
    await b.mnEvonodeCredits(H);
    await b.mnTrack(H, "Home node");
    await b.mnUntrack(H);
    await b.mnSetLabel(H, null);
    await b.mnAttachKey(H, "payout", "22".repeat(32));
    await b.mnDetachKey(H, "owner");
    await b.mnKeys(W, "operator", 20, 20);
    await b.mnRevealKey(W, "platform_node", 0, "g1");
    await b.mnSharedShares(W, H);
    await b.mnSharedStandby(W, H);
    expect(sent).toEqual([
      ["mn_list_state", {}],
      ["mn_list", { walletId: W }],
      ["mn_masternode", { walletId: W, proTxHash: H }],
      ["mn_payouts", { walletId: W, proTxHash: H, since: 1_700_000_000 }],
      ["mn_find", { query: "203.0.113.7:19999" }],
      ["mn_evonode_credits", { proTxHash: H }],
      ["mn_track", { proTxHash: H, label: "Home node" }],
      ["mn_untrack", { proTxHash: H }],
      ["mn_set_label", { proTxHash: H, label: null }],
      ["mn_attach_key", { proTxHash: H, role: "payout", key: "22".repeat(32) }],
      ["mn_detach_key", { proTxHash: H, role: "owner" }],
      ["mn_keys", { walletId: W, role: "operator", start: 20, count: 20 }],
      ["mn_reveal_key", { walletId: W, role: "platform_node", index: 0, grant: "g1" }],
      ["mn_shared_shares", { walletId: W, proTxHash: H }],
      ["mn_shared_standby", { walletId: W, proTxHash: H }],
    ]);
  });

  it("keep engine reads under the masternodes prefix and credits outside it", async () => {
    const client = new QueryClient();
    const engine = [mnKeys.listState, mnKeys.list(W), mnKeys.detail(W, H), mnKeys.payouts(W, H, null), mnKeys.find(H), mnKeys.keychain(W, "owner", 0, 20), mnKeys.sharedShares(W, H), mnKeys.sharedStandby(W, H)];
    for (const k of [...engine, mnKeys.credits(H)]) client.setQueryData(k, 1);
    const stale = invalidationsFor({ kind: "masternodes", network: "testnet" });
    expect(stale).toEqual([keys.masternodesAll]);
    await client.invalidateQueries({ queryKey: keys.masternodesAll });
    for (const k of engine) expect(client.getQueryState(k)?.isInvalidated, String(k)).toBe(true);
    expect(client.getQueryState(mnKeys.credits(H))?.isInvalidated).toBe(false);
  });

  it("re-read the proposal and username voters after an attached key changes (Astra r1 3)", async () => {
    const client = new QueryClient();
    const voting = [...keys.governanceAll, "voting", W, H];
    const read = [mnKeys.list(W), mnKeys.detail(W, H), voting, contestedNamesKey(W)];
    for (const k of [...read, mnKeys.credits(H)]) client.setQueryData(k, 1);
    await invalidateAfterKeyChange(client);
    for (const k of read) expect(client.getQueryState(k)?.isInvalidated, String(k)).toBe(true);
    expect(client.getQueryState(mnKeys.credits(H))?.isInvalidated).toBe(false);
  });

  it("re-read payouts on history, the list state on sync and the list and pane on an unlock", () => {
    expect(invalidationsFor({ kind: "historyChanged", network: "testnet", walletId: W, txids: [] })).toContainEqual(mnKeys.payoutsAll(W));
    expect(invalidationsFor({ kind: "sync", network: "testnet", status: {} as never })).toContainEqual(mnKeys.listState);
    expect(invalidationsFor({ kind: "vaultLockState", network: "testnet", state: "unlocked" })).toContainEqual(mnKeys.detailAll);
    // The list's rows carry the attached keys too (`null` while locked), beyond §3.5's pane.
    expect(invalidationsFor({ kind: "vaultLockState", network: "testnet", state: "unlockedMixingOnly" })).toContainEqual(mnKeys.listAll);
    // A lock drops them instead (lockGate.ts): nothing of the wallet is read again.
    expect(invalidationsFor({ kind: "vaultLockState", network: "testnet", state: "locked" })).not.toContainEqual(mnKeys.listAll);
  });
});
