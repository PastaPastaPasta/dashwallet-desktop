import { describe, expect, it } from "vitest";
import type { TxDropInfo } from "../api/generated/TxDropInfo";
import { droppedLine, invalidSourceDetail, recipientOf } from "./dropped";

const drop = (state: TxDropInfo["state"], cause: TxDropInfo["cause"] = "invalidSource"): TxDropInfo => ({ cause, state });

describe("a dropped transaction's details", () => {
  it("say where the engine's state puts its coins", () => {
    expect(droppedLine(drop({ kind: "released" }))).toContain("are back in your balance.");
    expect(droppedLine(drop({ kind: "releasing" }))).toContain("when the current rescan finishes.");
    expect(droppedLine(drop({ kind: "held" }))).toContain("open What can I do? under that amount on Home.");
    expect(droppedLine(drop({ kind: "rescanNeeded" }))).toContain("open Tools and run Rescan from the wallet's birth.");
    expect(droppedLine(drop({ kind: "rescanFromHeight", height: 1200 }))).toContain("open Tools and set Wallet birth height to 1200 or lower.");
    expect(droppedLine(drop({ kind: "awaitingFunding" }))).toContain("hasn't confirmed yet.");
  });

  it("say a never-valid incoming transaction counts nowhere, whatever its state", () => {
    for (const state of [{ kind: "released" }, { kind: "rescanNeeded" }] as const) {
      expect(droppedLine(drop(state, "invalidClaim"))).toBe(
        "DashPay Desktop dropped this transaction because it was never valid, so it can never confirm. It no longer counts toward your balance.",
      );
    }
  });

  it("explain an invalid source only once its coins are back", () => {
    expect(invalidSourceDetail(drop({ kind: "released" }))).toBe(true);
    expect(invalidSourceDetail(drop({ kind: "releasing" }))).toBe(false);
    expect(invalidSourceDetail(drop({ kind: "released" }, "replaced"))).toBe(false);
  });

  it("name the recipient by contact, then label, then address", () => {
    const counterparty = { identityId: "id", username: "alice", displayName: "Alice A." } as never;
    expect(recipientOf({ counterparty, label: "Rent", address: "yAddr" })).toBe("Alice A.");
    expect(recipientOf({ label: "Rent", address: "yAddr" })).toBe("Rent");
    expect(recipientOf({ label: null, address: "yAddr" })).toBe("yAddr");
    expect(recipientOf({ label: null, address: null })).toBeNull();
  });
});
