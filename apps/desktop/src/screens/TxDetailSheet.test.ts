import { describe, expect, it } from "vitest";
import type { TxStatusInfo } from "../api/generated/TxStatusInfo";
import { statusText } from "./TxDetailSheet";

const status = (over: Partial<TxStatusInfo> = {}): TxStatusInfo => ({
  kind: "confirmed",
  confirmations: 10,
  instantLocked: false,
  chainLocked: false,
  maturesIn: null,
  ...over,
});

describe("transaction detail status", () => {
  it("uses plain words for pending, InstantSend and confirmations", () => {
    expect(statusText(status({ kind: "unconfirmed" }))).toBe("Pending");
    expect(statusText(status({ kind: "unconfirmed", instantLocked: true }))).toBe("Instant (InstantSend locked)");
    expect(statusText(status({ kind: "confirming", confirmations: 3 }))).toBe("Confirmed · 3 confirmations");
    expect(statusText(status({ kind: "confirmed", confirmations: 6 }))).toBe("Confirmed · 6 confirmations");
    expect(statusText(status({ kind: "confirmed", confirmations: 7 }))).toBe("Confirmed");
  });

  it("an InstantSend-locked row that a block confirmed reads as confirmed", () => {
    expect(statusText(status({ kind: "confirmed", confirmations: 150, instantLocked: true }))).toBe("Confirmed");
    expect(statusText(status({ kind: "confirming", confirmations: 2, instantLocked: true }))).toBe("Confirmed · 2 confirmations");
    expect(statusText(status({ kind: "confirmed", confirmations: 1, chainLocked: true }))).toBe("Confirmed · 1 confirmation");
  });

  it("counts one confirmation in the singular", () => {
    expect(statusText(status({ kind: "confirming", confirmations: 1 }))).toBe("Confirmed · 1 confirmation");
    expect(statusText(status({ kind: "immature", confirmations: 1, maturesIn: 99 }))).toBe("Maturing · 1 confirmation, available in 99 more blocks");
  });

  it("keeps the problem states readable", () => {
    expect(statusText(status({ kind: "conflicted" }))).toBe("Conflicted");
    expect(statusText(status({ kind: "abandoned" }))).toBe("Abandoned");
    expect(statusText(status({ kind: "notAccepted" }))).toBe("Not accepted");
  });
});
