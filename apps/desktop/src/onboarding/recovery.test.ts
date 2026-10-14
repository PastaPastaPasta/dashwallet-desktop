// Show Recovery Phrase (IOS-003, IOS-108): the Swift IOS-108 test, ported, and U-02 D13.

import { describe, expect, it } from "vitest";
import { LockText } from "./lock";
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
    expect(model.lock.message).toBe("Enter your wallet passphrase.");
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
    expect(model.lock.message).toBe("The passphrase entered for the wallet decryption was incorrect. 7 attempts remaining");
    expect(model.words).toEqual([]);
  });

  it("revealFollowsTheLockScreenPolicy", async () => {
    // `authorize` counts failures as `unlock` does (review r1 #4): the eighth stops the prompt.
    const w = world();
    w.status = status({ state: "unlockedMixingOnly", walletsWithSecrets: [WALLET_A], failedAttempts: 7 });
    const model = new RecoveryPhraseModel(w);
    await model.load(WALLET_A);
    expect(model.lock.blocked).toBe(false);
    await model.reveal("wrong");
    expect(model.lock.disabled).toBe(true);
    expect(model.lock.message).toBe(LockText.disabled);
    await model.reveal("right");
    expect(w.authorizations).toHaveLength(1);
    expect(model.words).toEqual([]);
  });

  it("revealWaitsOutTheThrottle", async () => {
    const w = world();
    w.status = status({ walletsWithSecrets: [WALLET_A], failedAttempts: 3 });
    w.lastFailureAt = w.clock;
    const model = new RecoveryPhraseModel(w);
    await model.load(WALLET_A);
    expect(model.lock.blocked).toBe(true);
    await model.reveal("right");
    expect(w.authorizations).toEqual([]);
  });

  it("IOS003_closingDropsThePhrase", async () => {
    const model = new RecoveryPhraseModel(world());
    await model.load(WALLET_A);
    await model.reveal("right");
    model.dispose();
    expect(model.words).toEqual([]);
    expect(model.hasBip39).toBe(false);
  });

  it("aRevealThatEndsAfterTheSheetClosedIsDropped", async () => {
    const w = world();
    let open!: () => void;
    w.revealGate = new Promise((r) => (open = r));
    const model = new RecoveryPhraseModel(w);
    await model.load(WALLET_A);
    const reveal = model.reveal("right");
    await Promise.resolve();
    model.dispose();
    open();
    await reveal;
    expect(model.words).toEqual([]);
    expect(model.step).not.toBe("shown");
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

  it("aSheetMountedAgainStillReveals", async () => {
    // Review r2 #2: StrictMode disposes and mounts the same model.
    const model = new RecoveryPhraseModel(world());
    void model.load(WALLET_A);
    model.dispose();
    await model.load(WALLET_A);
    await model.reveal("right");
    expect(model.step).toBe("shown");
    expect(model.words).toEqual(PHRASE.split(" "));
  });

  it("aRevealFailureAfterTheRightPassphraseIsExplained", async () => {
    // Review r2 #3: the prompt shows `lock.message ?? error`; the passphrase was right, so the
    // reason is in `error`.
    const w = world();
    w.revealErrors = [appError("vault.locked")];
    const model = new RecoveryPhraseModel(w);
    await model.load(WALLET_A);
    await model.reveal("right");
    expect(model.step).toBe("confirm");
    expect(model.lock.message).toBeNull();
    expect(model.error).toBe("Unlock the wallet and try again.");
  });

  it("anUnencryptedRevealTakesOneClick", async () => {
    const w = world(false);
    const model = new RecoveryPhraseModel(w);
    await model.load(WALLET_A);
    await Promise.all([model.reveal(""), model.reveal("")]);
    expect(model.step).toBe("shown");
    expect(w.authorizations).toHaveLength(1);
  });

  it("noWalletWithAPhrase", async () => {
    const w = world();
    w.status = status({ walletsWithSecrets: [] });
    const model = new RecoveryPhraseModel(w);
    await model.load();
    expect(model.step).toBe("unavailable");
  });
});
