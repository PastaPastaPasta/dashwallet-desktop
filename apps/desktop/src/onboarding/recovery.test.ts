// Show Recovery Phrase (IOS-003, IOS-108): the Swift IOS-108 test, ported, and U-02 D13.

import { describe, expect, it } from "vitest";
import { RecoveryPhraseModel } from "./recovery";
import { appError, FakeWorld, PHRASE, status, summary, WALLET_A, WALLET_B } from "./testWorld";

function world(encrypted = true): FakeWorld {
  const w = new FakeWorld();
  w.status = status({ state: encrypted ? "unlocked" : "unencrypted", encrypted, walletsWithSecrets: [WALLET_A, WALLET_B] });
  w.wallets = [summary(WALLET_A, "Main"), summary(WALLET_B, "Savings"), summary("d4".repeat(32), "Watch", { watchOnly: true, hasMnemonic: false })];
  w.phrases.set(WALLET_A, PHRASE);
  return w;
}

describe("recovery phrase view model", () => {
  it("IOS108_revealPhraseOfAChosenWalletNeedsThePassphrase", async () => {
    const w = world();
    const model = new RecoveryPhraseModel(w);
    await model.load();
    // Watch-only wallets have no phrase to show.
    expect(model.wallets.map((x) => x.name)).toEqual(["Main", "Savings"]);
    expect(model.step).toBe("wallet");
    model.choose(WALLET_A);
    await model.reveal("");
    expect(model.error).toBe("Enter your wallet passphrase.");
    expect(w.authorizations).toEqual([]);
    await model.reveal("right");
    expect(model.step).toBe("shown");
    expect(model.words).toEqual(PHRASE.split(" "));
    expect(w.authorizations).toEqual([{ kind: "revealSecret", walletId: WALLET_A, credential: "passphrase" }]);
  });

  it("IOS108_aWrongPassphraseRevealsNothing", async () => {
    const w = world();
    const model = new RecoveryPhraseModel(w);
    await model.load(WALLET_A);
    expect(model.step).toBe("confirm");
    await model.reveal("wrong");
    expect(model.step).toBe("confirm");
    expect(model.error).toBe("The passphrase entered for the wallet decryption was incorrect.");
    expect(model.words).toEqual([]);
  });

  it("IOS003_closingDropsThePhrase", async () => {
    const model = new RecoveryPhraseModel(world());
    await model.load(WALLET_A);
    await model.reveal("right");
    model.dispose();
    expect(model.words).toEqual([]);
    expect(model.bip39Passphrase).toBe("");
  });

  it("IOS108_anUnencryptedVaultUsesTheUnencryptedCredential", async () => {
    const w = world(false);
    const model = new RecoveryPhraseModel(w);
    await model.load(WALLET_A);
    expect(model.encrypted).toBe(false);
    await model.reveal("");
    expect(model.step).toBe("shown");
    expect(w.authorizations[0].credential).toBe("unencrypted");
  });

  it("aGrantThatDidNotReachTheRevealIsRevoked", async () => {
    const w = world();
    w.revealErrors = [appError("storage")];
    const model = new RecoveryPhraseModel(w);
    await model.load(WALLET_A);
    await model.reveal("right");
    expect(model.step).toBe("confirm");
    expect(w.revoked).toEqual(["grant-1"]);
    expect(model.error).toBe("Something went wrong (storage).");
  });

  it("noWalletWithAPhrase", async () => {
    const w = world();
    w.status = status({ walletsWithSecrets: [] });
    const model = new RecoveryPhraseModel(w);
    await model.load();
    expect(model.step).toBe("unavailable");
  });
});
