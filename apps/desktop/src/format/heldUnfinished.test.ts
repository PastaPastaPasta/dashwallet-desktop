import { describe, expect, it } from "vitest";
import type { Balances } from "../api/generated/Balances";
import { summary } from "../onboarding/testWorld";
import { heldLine, heldWayOutText } from "./heldUnfinished";

const COIN = 100_000_000;
const core: Balances = { confirmed: 3 * COIN, unconfirmed: 0, immature: 0, locked: 0, total: 3 * COIN, coinjoin: 0, catchingUp: false, heldUnfinished: 0, heldUnvalued: 0 };
const fmt = (duffs: number) => (duffs / COIN).toFixed(2);
const A = "a1".repeat(32);
const B = "b2".repeat(32);

describe("Home's held line", () => {
  it("is empty while nothing is held", () => {
    expect(heldLine(core, fmt)).toBe("");
  });

  it("gives the held amount when every held coin's value is known", () => {
    expect(heldLine({ ...core, heldUnfinished: 1.5 * COIN }, fmt)).toBe("1.50 held by an unfinished payment");
  });

  it("says at least, summing only the known values, when some are unknown", () => {
    expect(heldLine({ ...core, heldUnfinished: 1.5 * COIN, heldUnvalued: 2 }, fmt)).toBe("At least 1.50 held by an unfinished payment");
  });

  it("counts the coins when no held coin's value is known, never hiding the line", () => {
    expect(heldLine({ ...core, heldUnvalued: 1 }, fmt)).toBe("A coin held by an unfinished payment");
    expect(heldLine({ ...core, heldUnvalued: 3 }, fmt)).toBe("3 coins held by an unfinished payment");
  });
});

describe("the held coins' way out", () => {
  const restore = "under Scan the blockchain from, choose The beginning.";

  it("restores one of several phrase wallets directly", () => {
    expect(heldWayOutText(A, [summary(A, "Main"), summary(B, "Savings")])).toBe(
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. To get back any coins the payment didn't spend, you'll remove Main and restore it from its recovery phrase. First make sure you have that phrase, and its BIP39 passphrase if the wallet was created with one. That isn't the passphrase you unlock Dash Wallet with: the same words without it restore a different wallet. Check too whether you added Main as Dash Core compatible (a recovery phrase from Dash Core), because you need to choose the same again. Keep your own copies of any masternode keys or shared-masternode standbys you added, because the phrase doesn't restore them. Removing Main also unlocks any coins you locked with Lock unspent in Tools ▸ Coin Selection, so note which they are and lock them again before you send or mix. Then, in Settings ▸ Wallet, remove Main (if it asks you to open another wallet first, do that), and restore it with Add wallet… and its phrase. On Restore options, enter the BIP39 passphrase if it has one, set Dash Core compatible (a recovery phrase from Dash Core) the same way as before, and under Scan the blockchain from, choose The beginning. It scans the whole chain again, which can take a long time. Coins the payment didn't spend come back once it has finished syncing. Until then, coins Main has already spent can show as unspent, so don't send anything from it until syncing finishes: the network won't accept a payment that uses one of them. If the unfinished payment was replaced by a newer version that hasn't confirmed yet, a coin only that newer version uses can also come back as unspent. Spending it conflicts with that payment, and only one of the two goes through.",
    );
  });

  it("restores a lone phrase wallet from the welcome screen once it is removed", () => {
    expect(heldWayOutText(A, [summary(A, "Main")])).toBe(
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. To get back any coins the payment didn't spend, you'll remove Main and restore it from its recovery phrase. First make sure you have that phrase, and its BIP39 passphrase if the wallet was created with one. That isn't the passphrase you unlock Dash Wallet with: the same words without it restore a different wallet. Check too whether you added Main as Dash Core compatible (a recovery phrase from Dash Core), because you need to choose the same again. Keep your own copies of any masternode keys or shared-masternode standbys you added, because the phrase doesn't restore them. Removing Main also unlocks any coins you locked with Lock unspent in Tools ▸ Coin Selection, so note which they are and lock them again before you send or mix. Then, in Settings ▸ Wallet, remove Main. If it asks you to open another wallet first, open it, remove Main, and restore it with Add wallet… and its phrase. Otherwise Dash Wallet shows Welcome to Dash Wallet once Main is removed: choose Restore from recovery phrase and enter its phrase. On Restore options, enter the BIP39 passphrase if it has one, set Dash Core compatible (a recovery phrase from Dash Core) the same way as before, and under Scan the blockchain from, choose The beginning. It scans the whole chain again, which can take a long time. Coins the payment didn't spend come back once it has finished syncing. Until then, coins Main has already spent can show as unspent, so don't send anything from it until syncing finishes: the network won't accept a payment that uses one of them. If the unfinished payment was replaced by a newer version that hasn't confirmed yet, a coin only that newer version uses can also come back as unspent. Spending it conflicts with that payment, and only one of the two goes through.",
    );
  });

  it("adds a watch-only wallet again from its key, with no restore options", () => {
    const watch = summary(A, "Cold", { watchOnly: true, hasMnemonic: false });
    expect(heldWayOutText(A, [watch, summary(B, "Savings")])).toBe(
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. To get back any coins the payment didn't spend, you'll remove Cold and add it again as a watch-only wallet. First make sure you have its extended public key (the wallet that signs for it can show it) and your own copies of any masternode keys or shared-masternode standbys you added, because the key doesn't restore them. Removing Cold also unlocks any coins you locked with Lock unspent in Tools ▸ Coin Selection, so note which they are and lock them again before you send or mix. Then, in Settings ▸ Wallet, remove Cold and add it again with Add watch-only wallet… and that key. It scans the whole chain again, which can take a long time. Coins the payment didn't spend come back once it has finished syncing. Until then, coins Cold has already spent can show as unspent, so don't create a payment from it until syncing finishes: the network won't accept a payment that uses one of them. If the unfinished payment was replaced by a newer version that hasn't confirmed yet, a coin only that newer version uses can also come back as unspent. Spending it conflicts with that payment, and only one of the two goes through.",
    );
    expect(heldWayOutText(A, [watch])).toBe(
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. To get back any coins the payment didn't spend, you'll remove Cold and add it again as a watch-only wallet. First make sure you have its extended public key (the wallet that signs for it can show it) and your own copies of any masternode keys or shared-masternode standbys you added, because the key doesn't restore them. Removing Cold also unlocks any coins you locked with Lock unspent in Tools ▸ Coin Selection, so note which they are and lock them again before you send or mix. Then, in Settings ▸ Wallet: add another wallet (a new one is fine), remove Cold, and add it again with Add watch-only wallet… and that key. It scans the whole chain again, which can take a long time. Coins the payment didn't spend come back once it has finished syncing. Until then, coins Cold has already spent can show as unspent, so don't create a payment from it until syncing finishes: the network won't accept a payment that uses one of them. If the unfinished payment was replaced by a newer version that hasn't confirmed yet, a coin only that newer version uses can also come back as unspent. Spending it conflicts with that payment, and only one of the two goes through. You can remove the extra wallet afterwards.",
    );
    expect(heldWayOutText(A, [watch])).not.toContain(restore);
  });

  it("keeps a signing wallet added from a seed with no recovery phrase", () => {
    expect(heldWayOutText(A, [summary(A, "Seed", { hasMnemonic: false })])).toBe(
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. Keep Seed: don't remove it to get them back. It was added from a seed without a recovery phrase, so it can't be restored from a phrase, and removing it deletes its keys from this computer. Coins the payment didn't spend stay held in Seed: they're still yours, but this version won't spend them. The rest of Seed works as usual.",
    );
  });

  it("keeps a wallet.dat import with legacy address ranges", () => {
    expect(heldWayOutText(A, [summary(A, "Core", { legacyIssued: true }), summary(B, "Savings")])).toBe(
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. Keep Core: don't remove it to get them back. It was added from a Dash Core wallet.dat that uses older addresses a recovery phrase doesn't restore, so removing and restoring it would leave some of its coins out. Coins the payment didn't spend stay held in Core: they're still yours, but this version won't spend them. The rest of Core works as usual.",
    );
  });

  it("keeps a wallet with no phrase and legacy ranges as one with no phrase", () => {
    expect(heldWayOutText(A, [summary(A, "Seed", { hasMnemonic: false, legacyIssued: true })])).toBe(
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. Keep Seed: don't remove it to get them back. It was added from a seed without a recovery phrase, so it can't be restored from a phrase, and removing it deletes its keys from this computer. Coins the payment didn't spend stay held in Seed: they're still yours, but this version won't spend them. The rest of Seed works as usual.",
    );
  });

  it("gives nothing for a wallet the list doesn't hold, or a failed read", () => {
    expect(heldWayOutText(A, [summary(B, "Savings")])).toBeNull();
    expect(heldWayOutText(A, [])).toBeNull();
  });
});
