import { describe, expect, it } from "vitest";
import type { TxStatusInfo } from "../api/generated/TxStatusInfo";
import { confirmationDetail, statusText } from "./TxDetailSheet";

const status = (over: Partial<TxStatusInfo> = {}): TxStatusInfo => ({
  kind: "confirmed",
  confirmations: 10,
  instantLocked: false,
  chainLocked: false,
  maturesIn: null,
  ...over,
});

describe("transaction detail status", () => {
  it("says confirmed or waiting, never a raw confirmation count", () => {
    expect(statusText(status({ kind: "unconfirmed", confirmations: 0 }))).toBe("Waiting for confirmation");
    expect(statusText(status({ kind: "unconfirmed", instantLocked: true, confirmations: 0 }))).toBe("InstantSend");
    expect(statusText(status({ kind: "confirming", confirmations: 3 }))).toBe("Confirmed");
    expect(statusText(status({ kind: "confirmed", confirmations: 6 }))).toBe("Confirmed");
    expect(statusText(status({ kind: "confirmed", confirmations: 7 }))).toBe("Confirmed");
    expect(statusText(status({ kind: "confirmed", confirmations: 1 }))).not.toContain("0/");
    expect(statusText(status({ kind: "unconfirmed" }))).not.toMatch(/unconfirmed/i);
  });

  it("an InstantSend-locked row that a block confirmed reads as confirmed", () => {
    expect(statusText(status({ kind: "confirmed", confirmations: 150, instantLocked: true }))).toBe("Confirmed");
    expect(statusText(status({ kind: "confirming", confirmations: 2, instantLocked: true }))).toBe("Confirmed");
    expect(statusText(status({ kind: "confirmed", confirmations: 1, chainLocked: true }))).toBe("Confirmed");
  });

  it("keeps the confirmation count for the folded details", () => {
    expect(confirmationDetail(status({ kind: "unconfirmed", confirmations: 0 }))).toBeNull();
    expect(confirmationDetail(status({ kind: "confirming", confirmations: 1 }))).toBe("1 confirmation");
    expect(confirmationDetail(status({ kind: "confirmed", confirmations: 6 }))).toBe("6 confirmations");
    expect(confirmationDetail(status({ kind: "immature", confirmations: 1, maturesIn: 99 }))).toBe("1 confirmation, available in 99 more blocks");
    expect(confirmationDetail(status({ kind: "immature", confirmations: 40, maturesIn: null }))).toBe("40 confirmations");
  });

  it("keeps the problem states readable", () => {
    expect(statusText(status({ kind: "conflicted" }))).toBe("Conflicted");
    expect(statusText(status({ kind: "abandoned" }))).toBe("Abandoned");
    expect(statusText(status({ kind: "notAccepted" }))).toBe("Not accepted");
    expect(statusText(status({ kind: "immature", confirmations: 1, maturesIn: 99 }))).toBe("Maturing");
  });
});
