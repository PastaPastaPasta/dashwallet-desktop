import { describe, expect, it } from "vitest";
import type { TxStatusInfo } from "../api/generated/TxStatusInfo";
import { abandonQuestion, actionProblem, confirmationDetail, statusText } from "./TxDetailSheet";
import { summary, WALLET_A } from "../onboarding/testWorld";

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
  const OUTGOING = "Abandon this transaction? Dash Wallet stops counting it, and the coins it spent come back to your balance unless something else has already spent them. If it still goes through, it counts again, so sending the same payment again could pay twice. If Dash Wallet sent it since it last started, it may keep re-sending it for up to a day. To stop that, restart Dash Wallet.";
  const INCOMING = "Abandon this transaction? Dash Wallet stops counting it. If it's locked or confirmed anyway, it counts again.";
  const UNKNOWN_INPUTS =
    "Abandon this transaction? Dash Wallet stops counting it. If it's locked or confirmed anyway, it counts again. If this is a payment you sent, sending it again could pay twice. If Dash Wallet sent it since it last started, it may keep re-sending it for up to a day. To stop that, restart Dash Wallet.";

  it("warns about paying twice when the wallet spent inputs", () => {
    expect(abandonQuestion({ totalDebit: 50_000_226, inputsUnresolved: false })).toBe(OUTGOING);
    expect(abandonQuestion({ totalDebit: 50_000_226, inputsUnresolved: true })).toBe(OUTGOING);
  });

  it("treats a payment to yourself as outgoing: it nets only the fee but spends this wallet's coins", () => {
    // Inputs 1.00000226 DASH of ours, 1 DASH back to an address of ours: net -226, the fee.
    expect(abandonQuestion({ totalDebit: 100_000_226, inputsUnresolved: false })).toBe(OUTGOING);
  });

  it("uses the recipient's text when the wallet spent nothing and knows every input", () => {
    expect(abandonQuestion({ totalDebit: 0, inputsUnresolved: false })).toBe(INCOMING);
  });

  it("says a payment sent elsewhere could pay twice when an input's coin is unknown", () => {
    // Another installation's payment, its change to a receiving address and no sent mark here.
    expect(abandonQuestion({ totalDebit: 0, inputsUnresolved: true })).toBe(UNKNOWN_INPUTS);
  });

  it("takes the spending text when the debit is unknown", () => {
    expect(abandonQuestion({ totalDebit: null, inputsUnresolved: true })).toBe(OUTGOING);
  });
});

describe("a refused abandon", () => {
  const refused = (refusal: string, message: string) => ({ code: "tx_action_refused", message, data: { kind: "txAction", refusal } });

  it("gives a rescanFirst refusal's steps from the catalog, not the engine's text", () => {
    expect(actionProblem(refused("rescanFirst", "engine text"), "abandon")).toBe("Restart Dash Wallet, run Rescan from the wallet's birth, then try Abandon again.");
  });

  it("says an Abandon refused while the wallet catches up after a fork can be tried once it has", () => {
    expect(actionProblem({ code: "rescan_in_progress", message: "a rescan is in progress", data: null }, "abandon")).toBe(
      "The wallet is catching up with the network. Try again once it has finished.",
    );
  });

  it("keeps the engine's text for a Resend refused while the wallet catches up", () => {
    expect(actionProblem({ code: "rescan_in_progress", message: "a rescan is in progress", data: null }, "resend")).toBe("a rescan is in progress");
  });

  it("says why a payment with a locked or confirmed later payment can't be abandoned, from the catalog", () => {
    expect(actionProblem(refused("settledDescendant", "engine text"), "abandon")).toBe(
      "This transaction can't be abandoned because a later transaction that depends on it is already locked or confirmed.",
    );
  });

  it("says why a payment funding an unfinished Platform transfer can't be abandoned, from the catalog", () => {
    expect(actionProblem(refused("platformFunding", "engine text"), "abandon")).toBe(
      "This payment can't be abandoned, because it pays for a Platform transfer, credit top-up or username registration that hasn't finished. Dash Wallet sends it again until that finishes.",
    );
  });

  it("names the wallet whose saved transaction doesn't read, and what to do, from the catalog", () => {
    const unreadable = { ...refused("recordUnreadable", "engine text"), data: { kind: "txAction", refusal: "recordUnreadable", walletId: WALLET_A } };
    const savings = summary(WALLET_A, "Savings");
    expect(actionProblem(unreadable, "abandon", [savings, summary("b2".repeat(32), "Spending")])).toBe(
      "Dash Wallet can't read one of the transaction records in Savings, so Abandon and Remove Unconfirmed are unavailable in every wallet until it's fixed. To fix it, you'll remove Savings and restore it from its recovery phrase. First make sure you have that phrase and your own copies of any masternode keys or shared-masternode standbys you added, because the phrase doesn't restore them. Then, in Settings ▸ Wallet, remove Savings and restore it with Add wallet… and its phrase. Its coins come back once it syncs. Wait until it has finished syncing before you use Abandon or Remove Unconfirmed in any wallet.",
    );
    expect(actionProblem(unreadable, "abandon", [savings])).toBe(
      "Dash Wallet can't read one of the transaction records in Savings, so Abandon and Remove Unconfirmed are unavailable until it's fixed. To fix it, you'll remove Savings and restore it from its recovery phrase. First make sure you have that phrase and your own copies of any masternode keys or shared-masternode standbys you added, because the phrase doesn't restore them. Then, in Settings ▸ Wallet: add another wallet (a new one is fine), remove Savings, and restore it with Add wallet… and its phrase. Its coins come back once it syncs. Wait until it has finished syncing before you use Abandon or Remove Unconfirmed. You can remove the extra wallet afterwards.",
    );
  });

  it("gives any other refusal the engine's message", () => {
    expect(actionProblem(refused("instantLocked", "transaction action refused: InstantLocked"), "abandon")).toBe("transaction action refused: InstantLocked");
  });
});
