import { describe, expect, it } from "vitest";
import type { TxStatusInfo } from "../api/generated/TxStatusInfo";
import { abandonQuestion, actionProblem, confirmationDetail, statusText } from "./TxDetailSheet";

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
    // No "0 confirmations", whatever the status says.
    expect(confirmationDetail(status({ kind: "confirming", confirmations: 0 }))).toBeNull();
    expect(confirmationDetail(status({ kind: "confirmed", confirmations: 0 }))).toBeNull();
    expect(confirmationDetail(status({ kind: "immature", confirmations: 0, maturesIn: 100 }))).toBe("Available in 100 more blocks");
  });

  it("keeps the problem states readable", () => {
    expect(statusText(status({ kind: "conflicted" }))).toBe("Conflicted");
    expect(statusText(status({ kind: "abandoned" }))).toBe("Abandoned");
    expect(statusText(status({ kind: "notAccepted" }))).toBe("Not accepted");
    expect(statusText(status({ kind: "immature", confirmations: 1, maturesIn: 99 }))).toBe("Maturing");
  });
});

describe("the abandon confirmation", () => {
  const OUTGOING = "Abandon this transaction? Dash Wallet stops counting it, and the coins it spent come back to your balance unless something else has already spent them. If it still goes through, it counts again, so sending the same payment again could pay twice.";
  const INCOMING = "Abandon this transaction? Dash Wallet stops counting it. If it's locked or confirmed anyway, it counts again.";

  it("warns about paying twice when the wallet spent inputs", () => {
    expect(abandonQuestion({ totalDebit: 50_000_226 })).toBe(OUTGOING);
  });

  it("treats a payment to yourself as outgoing: it nets only the fee but spends this wallet's coins", () => {
    // Inputs 1.00000226 DASH of ours, 1 DASH back to an address of ours: net -226, the fee.
    expect(abandonQuestion({ totalDebit: 100_000_226 })).toBe(OUTGOING);
  });

  it("uses the recipient's text when the wallet spent nothing", () => {
    expect(abandonQuestion({ totalDebit: 0 })).toBe(INCOMING);
  });

  it("takes the spending text when the debit is unknown", () => {
    expect(abandonQuestion({ totalDebit: null })).toBe(OUTGOING);
  });
});

describe("a refused abandon", () => {
  const refused = (refusal: string, message: string) => ({ code: "tx_action_refused", message, data: { kind: "txAction", refusal } });

  it("gives a rescanFirst refusal's steps from the catalog, not the engine's text", () => {
    expect(actionProblem(refused("rescanFirst", "engine text"))).toBe("Restart Dash Wallet, run Rescan from the wallet's birth, then try Abandon again.");
  });

  it("gives any other refusal the engine's message", () => {
    expect(actionProblem(refused("instantLocked", "transaction action refused: InstantLocked"))).toBe("transaction action refused: InstantLocked");
  });
});
