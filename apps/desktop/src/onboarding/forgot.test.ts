// Forgot passphrase (IOS-014): the Swift `WalletsSecurityAndHomeTests` IOS-014 tests, ported by
// parity ID, plus the D11 warning about the other wallets.

import { describe, expect, it } from "vitest";
import { ForgotPassphraseModel, ForgotText } from "./forgot";
import { PassphraseText } from "./passphrase";
import { appError, FakeWorld, PHRASE, status, summary, WALLET_A, WALLET_B } from "./testWorld";

function world(walletsWithSecrets = [WALLET_A, WALLET_B]): FakeWorld {
  const w = new FakeWorld();
  w.status = status({ state: "locked", walletsWithSecrets });
  w.wallets = [summary(WALLET_A, "Main"), summary(WALLET_B, "Savings")];
  w.phrases.set(WALLET_A, PHRASE);
  w.phrases.set(WALLET_B, "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong");
  return w;
}

async function loaded(w: FakeWorld) {
  const model = new ForgotPassphraseModel(w);
  await model.load();
  return model;
}

describe("forgot passphrase view model", () => {
  it("IOS014_forgotPassphraseChecksThePhraseThenReplacesTheVault", async () => {
    const w = world();
    const model = await loaded(w);
    expect(model.step).toBe("wallet");
    expect(model.wallets.map((x) => x.name)).toEqual(["Main", "Savings"]);
    model.chooseWallet(WALLET_A);
    // D11: the other wallet loses its keys, and the user is told before anything else.
    expect(model.step).toBe("consequences");
    expect(model.others).toEqual(["Savings"]);
    model.acceptConsequences();
    expect(model.step).toBe("phrase");
    await model.updatePhrase("bad phrase");
    expect(model.entry.problem).not.toBeNull();
    model.continuePhrase();
    expect(model.step).toBe("phrase");
    await model.updatePhrase(`  Abandon ${PHRASE.split(" ").slice(1).join(" ")} `);
    model.continuePhrase();
    expect(model.step).toBe("newPassphrase");
    await model.submitNewPassphrase("new", "nope");
    expect(model.error).toBe(PassphraseText.mismatch);
    await model.submitNewPassphrase("new", "new");
    expect(model.step).toBe("done");
    expect(w.recoveries).toEqual([{ walletId: WALLET_A, mnemonic: PHRASE, bip39Passphrase: "", newPassphrase: "new" }]);
    expect(model.walletsWithoutSecrets).toEqual(["Savings"]);
    expect(ForgotText.walletsWithoutSecrets(model.walletsWithoutSecrets)).toContain("Savings");
  });

  it("IOS014_aPhraseOfAnotherWalletIsAMismatch", async () => {
    const w = world([WALLET_A]);
    const model = await loaded(w);
    // One wallet with secrets: no choice and nothing else to lose.
    expect(model.step).toBe("phrase");
    await model.updatePhrase("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about");
    model.continuePhrase();
    await model.submitNewPassphrase("new", "new");
    expect(model.step).toBe("phrase");
    expect(model.error).toBe("This recovery phrase does not belong to the selected wallet.");
    expect(w.passphrase).toBe("right");
  });

  it("IOS014_thePhraseIsDroppedWhateverTheOutcome", async () => {
    const w = world([WALLET_A]);
    const model = await loaded(w);
    await model.updatePhrase("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about");
    model.continuePhrase();
    await model.submitNewPassphrase("new", "new");
    expect(model.entry.phrase).toBeNull();
    await model.updatePhrase(PHRASE);
    model.continuePhrase();
    await model.submitNewPassphrase("new", "new");
    expect(model.step).toBe("done");
    expect(model.entry.phrase).toBeNull();
    expect(model.walletsWithoutSecrets).toEqual([]);
  });

  it("IOS014_theBip39PassphraseIsPassedOn", async () => {
    const w = world([WALLET_A]);
    const model = await loaded(w);
    await model.updatePhrase(PHRASE);
    model.setBip39Passphrase("25th word");
    model.continuePhrase();
    await model.submitNewPassphrase("new", "new");
    expect(w.recoveries[0].bip39Passphrase).toBe("25th word");
  });

  it("IOS014_nothingToRecoverWithoutSecrets", async () => {
    const model = await loaded(world([]));
    expect(model.step).toBe("unavailable");
  });

  it("aWalletWithoutASummaryIsNamedByItsId", async () => {
    const w = world();
    w.wallets = [summary(WALLET_A, "Main")];
    const model = await loaded(w);
    expect(model.wallets.map((x) => x.name)).toEqual(["Main", `Wallet ${WALLET_B.slice(0, 8)}`]);
  });
  it("continueWaitsForTheNewestCheck", async () => {
    // Review r2 #5: Continue on the last answer while a newer edit was still being checked.
    const w = world([WALLET_A]);
    const model = await loaded(w);
    await model.updatePhrase(PHRASE);
    expect(model.entry.valid).toBe(true);
    let open!: () => void;
    const typo = `${PHRASE} zzzz`;
    w.held.set(typo, new Promise((r) => (open = r)));
    const check = model.updatePhrase(typo);
    expect(model.entry.valid).toBe(false);
    model.continuePhrase();
    expect(model.step).toBe("phrase");
    open();
    await check;
    expect(model.entry.valid).toBe(false);
    expect(model.entry.problem).not.toBeNull();
  });

  it("backFromTheNewPassphraseDropsThePhrase", async () => {
    // The phrase field starts empty again, so the model's copy goes too.
    const model = await loaded(world([WALLET_A]));
    await model.updatePhrase(PHRASE);
    model.continuePhrase();
    expect(model.step).toBe("newPassphrase");
    model.back();
    expect(model.step).toBe("phrase");
    expect(model.entry.phrase).toBeNull();
    expect(model.entry.valid).toBe(false);
  });

  it("backAfterALoadFailureLoadsAgain", async () => {
    // Review r1 #9: Back used to leave the sheet on "Loading…".
    const w = world();
    const listWallets = w.listWallets;
    w.listWallets = async () => {
      throw appError("storage");
    };
    const model = await loaded(w);
    expect(model.step).toBe("failed");
    w.listWallets = listWallets;
    model.back();
    await new Promise((r) => setTimeout(r, 0));
    expect(model.step).toBe("wallet");
  });
});
