import { describe, expect, it } from "vitest";
import type { Balances } from "../api/generated/Balances";
import { summary } from "../onboarding/testWorld";
import { heldLine, heldWayOutText } from "./heldUnfinished";

const COIN = 100_000_000;
const core: Balances = { confirmed: 3 * COIN, unconfirmed: 0, immature: 0, locked: 0, total: 3 * COIN, coinjoin: 0, catchingUp: false, heldUnfinished: 0, heldUnvalued: 0 };
const fmt = (duffs: number) => `${(duffs / COIN).toFixed(2)} DASH`;
const A = "a1".repeat(32);
const B = "b2".repeat(32);

describe("Home's held line", () => {
  it("is empty while nothing is held", () => {
    expect(heldLine(core, fmt)).toBe("");
  });

  it("gives the held amount when every held coin's value is known", () => {
    expect(heldLine({ ...core, heldUnfinished: 1.5 * COIN }, fmt)).toBe("1.50 DASH held by an unfinished payment");
  });

  it("says at least, summing only the known values, when some are unknown", () => {
    expect(heldLine({ ...core, heldUnfinished: 1.5 * COIN, heldUnvalued: 2 }, fmt)).toBe("At least 1.50 DASH held by an unfinished payment");
  });

  it("counts the coins when no held coin's value is known, never hiding the line", () => {
    expect(heldLine({ ...core, heldUnvalued: 1 }, fmt)).toBe("A coin held by an unfinished payment");
    expect(heldLine({ ...core, heldUnvalued: 3 }, fmt)).toBe("3 coins held by an unfinished payment");
  });
});

describe("the held coins' way out", () => {
  const restore = "On Restore options, under Scan the blockchain from, choose The beginning.";

  it("restores a lone phrase wallet after adding another", () => {
    expect(heldWayOutText(A, [summary(A, "Main")])).toBe(
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. To get back any coins the payment didn't spend, you'll remove Main and restore it from its recovery phrase. First make sure you have that phrase and your own copies of any masternode keys or shared-masternode standbys you added, because the phrase doesn't restore them. Then, in Settings ▸ Wallet: add another wallet (a new one is fine), remove Main, and restore it with Add wallet… and its phrase. On Restore options, under Scan the blockchain from, choose The beginning. It scans the whole chain again, which can take a long time. Coins the payment didn't spend come back once it has finished syncing. You can remove the extra wallet afterwards.",
    );
  });

  it("restores one of several phrase wallets directly", () => {
    expect(heldWayOutText(A, [summary(A, "Main"), summary(B, "Savings")])).toBe(
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. To get back any coins the payment didn't spend, you'll remove Main and restore it from its recovery phrase. First make sure you have that phrase and your own copies of any masternode keys or shared-masternode standbys you added, because the phrase doesn't restore them. Then, in Settings ▸ Wallet, remove Main and restore it with Add wallet… and its phrase. On Restore options, under Scan the blockchain from, choose The beginning. It scans the whole chain again, which can take a long time. Coins the payment didn't spend come back once it has finished syncing.",
    );
  });

  it("adds a watch-only wallet again from its key, with no restore options", () => {
    const watch = summary(A, "Cold", { watchOnly: true });
    expect(heldWayOutText(A, [watch, summary(B, "Savings")])).toBe(
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. To get back any coins the payment didn't spend, you'll remove Cold and add it again as a watch-only wallet. First make sure you have its extended public key (the wallet that signs for it can show it) and your own copies of any masternode keys or shared-masternode standbys you added, because the key doesn't restore them. Then, in Settings ▸ Wallet, remove Cold and add it again with Add watch-only wallet… and that key. It scans the whole chain again, which can take a long time. Coins the payment didn't spend come back once it has finished syncing.",
    );
    expect(heldWayOutText(A, [watch])).toBe(
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. To get back any coins the payment didn't spend, you'll remove Cold and add it again as a watch-only wallet. First make sure you have its extended public key (the wallet that signs for it can show it) and your own copies of any masternode keys or shared-masternode standbys you added, because the key doesn't restore them. Then, in Settings ▸ Wallet: add another wallet (a new one is fine), remove Cold, and add it again with Add watch-only wallet… and that key. It scans the whole chain again, which can take a long time. Coins the payment didn't spend come back once it has finished syncing. You can remove the extra wallet afterwards.",
    );
    expect(heldWayOutText(A, [watch])).not.toContain(restore);
  });

  it("treats a failed wallet read as several wallets, named by id", () => {
    expect(heldWayOutText(A, [])).toContain(`Then, in Settings ▸ Wallet, remove ${A} and restore it with Add wallet… and its phrase.`);
  });
});
