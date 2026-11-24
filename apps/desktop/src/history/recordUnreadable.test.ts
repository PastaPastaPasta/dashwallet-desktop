import { describe, expect, it } from "vitest";
import { summary } from "../onboarding/testWorld";
import { recordUnreadableText, unreadableWallet } from "./recordUnreadable";

const A = "a1".repeat(32);
const only = (name: string) =>
  expect.stringContaining(`Then, in Settings ▸ Wallet: add another wallet (a new one is fine), remove ${name}, and restore it with Add wallet… and its phrase.`);
const several = (name: string) =>
  expect.stringContaining(`unavailable in every wallet until it's fixed. To fix it, you'll remove ${name} and restore it from its recovery phrase.`);

describe("the recordUnreadable refusal's text", () => {
  it("uses the single-wallet text only when exactly one wallet is open", () => {
    const main = summary(A, "Main");
    expect(recordUnreadableText(A, [main])).toEqual(only("Main"));
    expect(recordUnreadableText(A, [main])).toBe(
      "Dash Wallet can't read one of the transaction records in Main, so Abandon and Remove Unconfirmed are unavailable until it's fixed. To fix it, you'll remove Main and restore it from its recovery phrase. First make sure you have that phrase and your own copies of any masternode keys or shared-masternode standbys you added, because the phrase doesn't restore them. Then, in Settings ▸ Wallet: add another wallet (a new one is fine), remove Main, and restore it with Add wallet… and its phrase. Its coins come back once it syncs. Wait until it has finished syncing before you use Abandon or Remove Unconfirmed. You can remove the extra wallet afterwards.",
    );
    expect(recordUnreadableText(A, [main, summary("b2".repeat(32), "Savings")])).toEqual(several("Main"));
    expect(recordUnreadableText(A, [main, summary("b2".repeat(32), "Savings"), summary("c3".repeat(32), "Spending")])).toEqual(several("Main"));
    expect(recordUnreadableText(A, [])).toEqual(several(A));
  });

  it("names the wallet by its display name, and reads the refusal's wallet", () => {
    expect(recordUnreadableText(A, [summary(A, "Main"), summary("b2".repeat(32), "Savings")])).toMatch(/^Dash Wallet can't read one of the transaction records in Main, so Abandon and Remove Unconfirmed are unavailable in every wallet/);
    const refusal = { code: "tx_action_refused", message: "x", data: { kind: "txAction", refusal: "recordUnreadable", walletId: A } };
    expect(unreadableWallet(refusal)).toBe(A);
    expect(unreadableWallet({ ...refusal, data: { kind: "txAction", refusal: "rescanFirst", walletId: null } })).toBeUndefined();
    expect(unreadableWallet(new Error("x"))).toBeUndefined();
  });
});
