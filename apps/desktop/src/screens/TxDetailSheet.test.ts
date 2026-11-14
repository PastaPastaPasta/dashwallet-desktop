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

  it("keeps the problem states readable", () => {
    expect(statusText(status({ kind: "conflicted" }))).toBe("Conflicted");
    expect(statusText(status({ kind: "abandoned" }))).toBe("Abandoned");
    expect(statusText(status({ kind: "notAccepted" }))).toBe("Not accepted");
  });
});
