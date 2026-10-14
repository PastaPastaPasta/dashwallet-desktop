// Create and restore (QT-102…105, QT-111, IOS-002…007, IOS-010, review H-5): the Swift
// `OnboardingViewModelTests`, ported by parity ID, in U-02's order (D2: create asks for the vault
// passphrase first and adds the wallet at the last correct chip).

import { describe, expect, it } from "vitest";
import { birthHeightFor } from "./birthHeight";
import { OnboardingModel, OnboardingText } from "./onboarding";
import { passphraseStrength, PassphraseText } from "./passphrase";
import { PhraseText } from "./phraseEntry";
import { appError, FakeWorld, NEXT_WALLET, PHRASE, seeded, status, WALLET_A } from "./testWorld";

const WORDS = PHRASE.split(" ");

function make(world: FakeWorld, network: "mainnet" | "testnet" | "regtest" = "testnet") {
  return new OnboardingModel(world, network, { random: seeded(42n), now: () => new Date(world.clock) });
}

/** A world whose vault exists and is unlocked (a second wallet). */
function unlockedWorld(): FakeWorld {
  const world = new FakeWorld();
  world.status = status({ walletsWithSecrets: [WALLET_A] });
  return world;
}

/** Picks the right chips for the whole challenge. */
async function solve(model: OnboardingModel) {
  for (const position of model.verifyChallenge.slice(model.verifiedCount)) {
    const chip = model.verifyChips.find((c) => c.word === WORDS[position] && !c.used)!;
    await model.select(chip.id);
  }
}

describe("onboarding view model: create", () => {
  it("IOS002_createShowsAFreshPhraseAndStoresNothing", async () => {
    const world = unlockedWorld();
    const model = make(world);
    await model.startCreate();
    expect(model.step).toBe("showPhrase");
    expect(model.flow).toBe("create");
    expect(model.phraseWords).toEqual(WORDS);
    expect(world.generateCalls).toEqual([12]);
    expect(world.creates).toEqual([]);
    expect(world.imports).toEqual([]);
  });

  it("IOS002_firstWalletAsksForThePassphraseBeforeThePhrase", async () => {
    const world = new FakeWorld();
    const model = make(world);
    await model.startCreate();
    expect(model.step).toBe("choosePassphrase");
    expect(world.generateCalls).toEqual([]);
    expect(await model.submitPassphrase("vault pass", "vault pass")).toBe(true);
    expect(model.step).toBe("showPhrase");
    // The vault is created only with the wallet, after the verify step (D2).
    expect(world.creates).toEqual([]);
  });

  it("IOS002_offers12And24Words", async () => {
    const world = unlockedWorld();
    const model = make(world);
    model.setWordCount(15);
    expect(model.wordCount).toBe(12);
    model.setWordCount(24);
    await model.startCreate();
    expect(world.generateCalls).toEqual([24]);
  });

  it("IOS006_theWordsAreOnScreenOnlyOnThePhraseStep", async () => {
    // The window's content protection covers the whole flow (D15, e2e/onboarding.spec.ts); the
    // model keeps the word list only while the phrase step shows.
    const model = make(unlockedWorld());
    expect(model.phraseWords).toEqual([]);
    await model.startCreate();
    expect(model.phraseWords).toEqual(WORDS);
    model.confirmWrittenDown();
    expect(model.phraseWords).toEqual([]);
    await solve(model);
    expect(model.step).toBe("done");
    expect(model.phraseWords).toEqual([]);
    expect(model.verifiedWords).toEqual([]);
    expect(model.verifyChips).toEqual([]);
  });

  it("IOS004_verifyChallengeNeedsTheRightWordsInOrder", async () => {
    const world = unlockedWorld();
    const model = make(world);
    await model.startCreate();
    model.confirmWrittenDown();
    expect(model.step).toBe("verifyPhrase");
    expect(model.verifyChallenge).toHaveLength(4);
    expect(model.verifyChallenge).toEqual(model.verifyChallenge.toSorted((a, b) => a - b));
    expect(model.verifyChips).toHaveLength(6);
    // The chips are the four asked words and two other words of the phrase.
    expect(new Set(model.verifyChips.map((c) => c.word)).size).toBe(6);
    for (const p of model.verifyChallenge) expect(model.verifyChips.map((c) => c.word)).toContain(WORDS[p]);
    for (const c of model.verifyChips) expect(WORDS).toContain(c.word);
    // A word that is not the next one is a mistake and does not advance.
    const wrong = model.verifyChips.find((c) => c.word !== WORDS[model.verifyChallenge[0]])!;
    await model.select(wrong.id);
    expect(model.verifyMistake).toBe(true);
    expect(model.verifiedCount).toBe(0);
    const first = model.verifyChips.find((c) => c.word === WORDS[model.verifyChallenge[0]])!;
    await model.select(first.id);
    expect(model.verifiedWords).toEqual([first.word]);
    expect(model.verifyMistake).toBe(false);
    await solve(model);
    expect(model.isVerified).toBe(false); // cleared with the phrase once the wallet is added
    expect(model.step).toBe("done");
    expect(world.imports).toHaveLength(1);
  });

  it("IOS004_verifyWordChecksPositions", async () => {
    const model = make(unlockedWorld());
    await model.startCreate();
    expect(model.verify("abandon", 0)).toBe(true);
    expect(model.verify("ability", 0)).toBe(false);
    expect(model.verify("abandon", 99)).toBe(false);
  });

  it("IOS004_thePhraseIsDroppedOnceTheWalletIsAdded", async () => {
    const model = make(unlockedWorld());
    await model.startCreate();
    model.confirmWrittenDown();
    await solve(model);
    expect(model.step).toBe("done");
    expect(model.walletId).toBe(NEXT_WALLET);
    expect(model.verify("abandon", 0)).toBe(false);
    expect(model.phraseWords).toEqual([]);
    expect(model.verifyChips).toEqual([]);
    expect(model.verifyChallenge).toEqual([]);
  });

  it("QT111_passphraseRules", async () => {
    const model = make(new FakeWorld());
    await model.startCreate();
    expect(await model.submitPassphrase("", "")).toBe(false);
    expect(model.passphraseError).toBe(PassphraseText.empty);
    expect(await model.submitPassphrase("one", "two")).toBe(false);
    expect(model.passphraseError).toBe(PassphraseText.mismatch);
    const long = "x".repeat(1025);
    expect(await model.submitPassphrase(long, long)).toBe(false);
    expect(model.passphraseError).toBe(PassphraseText.tooLong);
    model.passphraseEdited();
    expect(model.passphraseError).toBeNull();
    expect(["good", "strong"]).toContain(passphraseStrength("Tr0ub4dor&3xyz"));
    const max = "x".repeat(1024);
    expect(await model.submitPassphrase(max, max)).toBe(true);
    expect(model.passphraseError).toBeNull();
  });

  it("IOS010_createEncryptsTheNewVaultAndImportsAsANewWallet", async () => {
    const world = new FakeWorld();
    const model = make(world);
    await model.startCreate();
    await model.submitPassphrase("vault pass", "vault pass");
    model.confirmWrittenDown();
    await solve(model);
    expect(model.step).toBe("done");
    expect(model.walletId).toBe(NEXT_WALLET);
    expect(world.creates).toEqual(["vault pass"]);
    expect(world.imports).toEqual([
      // H-5: a new wallet lets the engine start at its tip.
      { mnemonic: PHRASE, bip39Passphrase: null, options: { name: null, birthHeight: null, coreCompat: false, lookahead: null } },
    ]);
  });

  it("IOS010_skippingEncryptionCreatesAnUnencryptedVault", async () => {
    const world = new FakeWorld();
    const model = make(world);
    await model.startCreate();
    await model.skipEncryption();
    model.confirmWrittenDown();
    await solve(model);
    expect(world.creates).toEqual([null]);
    expect(model.step).toBe("done");
  });

  it("secondWalletSkipsVaultCreation", async () => {
    const world = unlockedWorld();
    const model = make(world);
    await model.startCreate();
    model.confirmWrittenDown();
    await solve(model);
    expect(model.step).toBe("done");
    expect(world.creates).toEqual([]);
  });

  it("abandoningCreateStoresNothing", async () => {
    const world = new FakeWorld();
    const model = make(world);
    await model.startCreate();
    await model.submitPassphrase("vault pass", "vault pass");
    model.confirmWrittenDown();
    model.back();
    expect(model.step).toBe("showPhrase");
    model.back();
    expect(model.step).toBe("welcome");
    expect(model.flow).toBeNull();
    expect(model.verify("abandon", 0)).toBe(false);
    expect(world.creates).toEqual([]);
    expect(world.imports).toEqual([]);
  });

  it("D16_failuresShowCopyByCodeNeverTheEngineMessage", async () => {
    const world = unlockedWorld();
    world.importErrors = [appError("invalid_mnemonic", null, "unknown word: abandon")];
    const model = make(world);
    await model.startCreate();
    model.confirmWrittenDown();
    await solve(model);
    expect(model.step).toBe("failed");
    expect(model.failure).toBe(PhraseText.invalid);
    world.importErrors = [appError("storage", null, "disk said abandon")];
    model.back();
    model.confirmWrittenDown();
    await solve(model);
    expect(model.failure).toBe("Something went wrong (storage).");
  });

  it("aSecondClickWhileACallRunsIsIgnored", async () => {
    // Review r1 #3: a double click must not generate twice (the phrase on screen would change).
    const world = unlockedWorld();
    let open!: () => void;
    world.generateGate = new Promise((r) => (open = r));
    const model = make(world);
    const first = model.startCreate();
    await Promise.resolve();
    expect(model.busy).toBe(true);
    await model.startCreate();
    await model.startRestore();
    open();
    await first;
    expect(world.generateCalls).toEqual([12]);
    expect(model.step).toBe("showPhrase");
    expect(model.flow).toBe("create");
    expect(model.busy).toBe(false);
  });

  it("aPhraseThatArrivesAfterTheScreenClosedIsDropped", async () => {
    const world = unlockedWorld();
    let open!: () => void;
    world.generateGate = new Promise((r) => (open = r));
    const model = make(world);
    const start = model.startCreate();
    await Promise.resolve();
    model.dispose();
    open();
    await start;
    expect(model.phraseWords).toEqual([]);
    expect(model.step).not.toBe("showPhrase");
  });

  it("backWhileAPhraseIsGeneratedIsIgnored", async () => {
    // Review r2 #1: Back used to drop the chosen passphrase while the phrase arrived anyway, and
    // the wallet was then added to a vault without one.
    const world = new FakeWorld();
    let open!: () => void;
    world.generateGate = new Promise((r) => (open = r));
    const model = make(world);
    await model.startCreate();
    const submit = model.submitPassphrase("Correct horse 9!", "Correct horse 9!");
    model.back();
    expect(model.step).toBe("choosePassphrase");
    open();
    await submit;
    expect(model.step).toBe("showPhrase");
    model.confirmWrittenDown();
    await solve(model);
    expect(model.step).toBe("done");
    expect(world.creates).toEqual(["Correct horse 9!"]);
  });

  it("aModelMountedAgainStillWorks", async () => {
    // Review r2 #2: StrictMode disposes and mounts the same model; a flag left it dropping
    // every phrase.
    const model = make(unlockedWorld());
    model.dispose();
    await model.startCreate();
    expect(model.step).toBe("showPhrase");
    expect(model.phraseWords).toEqual(WORDS);
  });

  it("aRefusedVaultPassphraseCanBeReplacedWithoutANewPhrase", async () => {
    // Review r1 #6: the vault refuses the passphrase at the last chip; another one is chosen and
    // the verified phrase is added, with no second phrase and no passphrase kept meanwhile.
    const world = new FakeWorld();
    world.createErrors = [appError("vault.os_store_unavailable")];
    const model = make(world);
    await model.startCreate();
    await model.skipEncryption();
    model.confirmWrittenDown();
    await solve(model);
    expect(model.step).toBe("choosePassphrase");
    expect(model.passphraseError).toContain("no secret store");
    expect(await model.submitPassphrase("Fixture pass 42!", "Fixture pass 42!")).toBe(true);
    expect(world.generateCalls).toEqual([12]);
    expect(world.creates).toEqual([null, "Fixture pass 42!"]);
    expect(world.imports.map((i) => i.mnemonic)).toEqual([PHRASE]);
    expect(model.step).toBe("done");
  });

  it("everyWrongChipIsAnnounced", async () => {
    const model = make(unlockedWorld());
    await model.startCreate();
    model.confirmWrittenDown();
    const wrong = model.verifyChips.find((c) => c.word !== WORDS[model.verifyChallenge[0]])!;
    await model.select(wrong.id);
    await model.select(wrong.id);
    expect(model.verifyMistakes).toBe(2);
  });
});

describe("onboarding view model: unlock before adding", () => {
  it("IOS013_lockedVaultIsUnlockedBeforeAddingAWallet", async () => {
    const world = new FakeWorld();
    world.status = status({ state: "locked", walletsWithSecrets: [WALLET_A] });
    const model = make(world);
    await model.startRestore();
    expect(model.step).toBe("unlockVault");
    await model.unlockVault("wrong");
    expect(model.step).toBe("unlockVault");
    // The lock screen's policy and copy (D12): the count of attempts left.
    expect(model.lock.message).toBe("The passphrase entered for the wallet decryption was incorrect. 7 attempts remaining");
    await model.unlockVault("right");
    expect(world.unlockCalls).toEqual([
      { passphrase: "wrong", mixingOnly: false },
      { passphrase: "right", mixingOnly: false },
    ]);
    expect(model.step).toBe("restorePhrase");
    await model.updateRestoreText(PHRASE);
    model.continueRestore();
    await model.finishRestore();
    expect(model.step).toBe("done");
  });

  it("aMixingOnlyVaultIsUnlockedFullyBeforeCreating", async () => {
    const world = new FakeWorld();
    world.status = status({ state: "unlockedMixingOnly", walletsWithSecrets: [WALLET_A] });
    const model = make(world);
    await model.startCreate();
    expect(model.step).toBe("unlockVault");
    await model.unlockVault("right");
    expect(world.unlockCalls.at(-1)?.mixingOnly).toBe(false);
    expect(model.step).toBe("showPhrase");
  });

  it("importRefusedAsLockedAsksForTheVaultPassphrase", async () => {
    const world = unlockedWorld();
    world.importErrors = [appError("vault.locked")];
    const model = make(world);
    await model.startRestore();
    await model.updateRestoreText(PHRASE);
    model.continueRestore();
    await model.finishRestore();
    expect(model.step).toBe("unlockVault");
    await model.unlockVault("right");
    expect(model.step).toBe("done");
    expect(world.imports).toHaveLength(2);
  });

  it("failureAfterUnlockReturnsToTheStepBeforeIt", async () => {
    const world = new FakeWorld();
    world.status = status({ state: "locked", walletsWithSecrets: [WALLET_A] });
    world.importErrors = [appError("wallet_already_exists")];
    const model = make(world);
    await model.startRestore();
    await model.unlockVault("right");
    await model.updateRestoreText(PHRASE);
    model.continueRestore();
    await model.finishRestore();
    expect(model.step).toBe("failed");
    expect(model.failure).toBe("This wallet has already been added.");
    model.back();
    expect(model.step).toBe("restoreOptions");
  });

  it("emptyUnlockPassphraseIsNotSent", async () => {
    const world = new FakeWorld();
    world.status = status({ state: "locked" });
    const model = make(world);
    await model.startCreate();
    await model.unlockVault("");
    expect(world.unlockCalls).toEqual([]);
    expect(model.lock.message).toBe("Enter your wallet passphrase.");
  });
  it("backWhileUnlockingIsIgnored", async () => {
    // Review r2 #4: Back during the unlock left the restore going on with no flow.
    const world = unlockedWorld();
    world.status = status({ state: "locked", walletsWithSecrets: [WALLET_A] });
    const model = make(world);
    await model.startRestore();
    expect(model.step).toBe("unlockVault");
    const unlock = model.unlockVault("right");
    model.back();
    await unlock;
    expect(model.step).toBe("restorePhrase");
    expect(model.flow).toBe("restore");
    await model.updateRestoreText(PHRASE);
    model.continueRestore();
    await model.finishRestore();
    expect(model.step).toBe("done");
    expect(world.imports).toHaveLength(1);
  });
});

describe("onboarding view model: restore", () => {
  it("QT104_restoreDefaultsScanFromGenesisWithoutLookahead", async () => {
    const model = make(unlockedWorld());
    await model.startRestore();
    expect(model.step).toBe("restorePhrase");
    expect(model.options).toEqual({ birthHeight: 0, coreCompat: false });
  });

  it("IOS007_liveValidationAndSuggestions", async () => {
    const model = make(unlockedWorld());
    await model.startRestore();
    await model.updateRestoreText("aban");
    expect(model.entry.suggestions).toEqual(["abandon"]);
    await model.updateRestoreText("abandon ");
    expect(model.entry.suggestions).toEqual([]);
    expect(model.entry.problem).toBe(PhraseText.wordCount);
    expect(model.entry.valid).toBe(false);
    await model.updateRestoreText(`  ABANDON   ${WORDS.slice(1).join(" ")} `);
    expect(model.entry.check?.wordCount).toBe(12);
    expect(model.entry.problem).toBeNull();
    expect(model.entry.valid).toBe(true);
    model.continueRestore();
    expect(model.step).toBe("restoreOptions");
  });

  it("IOS007_unknownWordsAndBadChecksumAreReported", async () => {
    const world = unlockedWorld();
    const unknown = "abandon xyzzy able about above absent absorb abstract absurd abuse access accident";
    const badSum = Array(12).fill("abandon").join(" ");
    world.checks.set(unknown, { wordCount: 12, unknownWordIndices: [1], language: null, checksum: "invalid" });
    world.checks.set(badSum, { wordCount: 12, unknownWordIndices: [], language: "english", checksum: "invalid" });
    const model = make(world);
    await model.startRestore();
    await model.updateRestoreText(unknown);
    expect(model.entry.problem).toBe("Word 2 is not in the word list.");
    // While the last word is typed, only an earlier unknown word is worth saying.
    expect(model.entry.shownProblem(true)).toBe("Word 2 is not in the word list.");
    await model.updateRestoreText(badSum);
    expect(model.entry.problem).toBe(PhraseText.invalid);
    expect(model.entry.shownProblem(true)).toBeNull();
    expect(model.entry.shownProblem(false)).toBe(PhraseText.invalid);
    world.checks.set("abandon abil", { wordCount: 2, unknownWordIndices: [1], language: null, checksum: "invalid" });
    await model.updateRestoreText("abandon abil");
    expect(model.entry.shownProblem(true)).toBeNull();
    expect(model.entry.valid).toBe(false);
  });

  it("IOS007_aLateCheckDoesNotOverwriteANewerOne", async () => {
    const world = unlockedWorld();
    let release!: () => void;
    world.held.set("abandon", new Promise<void>((r) => (release = r)));
    const model = make(world);
    await model.startRestore();
    const slow = model.updateRestoreText("abandon");
    await model.updateRestoreText(PHRASE);
    expect(model.entry.valid).toBe(true);
    release();
    await slow;
    expect(model.entry.check?.wordCount).toBe(12);
    expect(model.entry.valid).toBe(true);
  });

  it("QT104_coreOnlyChecksumForcesCoreCompatibility", async () => {
    const world = unlockedWorld();
    world.checks.set(PHRASE, { wordCount: 12, unknownWordIndices: [], language: "english", checksum: "coreOnly" });
    const model = make(world);
    await model.startRestore();
    await model.updateRestoreText(PHRASE);
    expect(model.entry.coreOnly).toBe(true);
    expect(model.options.coreCompat).toBe(true);
    expect(model.entry.valid).toBe(true);
    model.setCoreCompatible(false);
    expect(model.options.coreCompat).toBe(true);
  });

  it("QT104_restoreWithBIP39PassphraseCoreCompatAndBirthHeight", async () => {
    const world = new FakeWorld();
    world.status = status({ state: "unencrypted", encrypted: false });
    const model = make(world);
    await model.startRestore();
    await model.updateRestoreText(PHRASE);
    model.continueRestore();
    model.setBip39Passphrase("25th word");
    model.setCoreCompatible(true);
    model.setBirthHeight(1_000_000);
    await model.finishRestore();
    expect(world.imports).toEqual([
      { mnemonic: PHRASE, bip39Passphrase: "25th word", options: { name: null, birthHeight: 1_000_000, coreCompat: true, lookahead: null } },
    ]);
    expect(model.step).toBe("done");
  });

  it("IOS010_restoreAsksForThePassphraseLastWhenThereIsNoVault", async () => {
    const world = new FakeWorld();
    const model = make(world);
    await model.startRestore();
    expect(model.step).toBe("restorePhrase");
    await model.updateRestoreText(PHRASE);
    model.continueRestore();
    await model.finishRestore();
    expect(model.step).toBe("choosePassphrase");
    model.back();
    expect(model.step).toBe("restoreOptions");
    await model.finishRestore();
    await model.submitPassphrase("vault pass", "vault pass");
    expect(world.creates).toEqual(["vault pass"]);
    expect(world.imports[0].mnemonic).toBe(PHRASE);
    expect(model.step).toBe("done");
  });

  it("QT105_birthDateBecomesAConservativeHeight", async () => {
    const world = unlockedWorld();
    const model = make(world, "mainnet");
    await model.startRestore();
    model.setBirthDate(new Date(world.clock + 86_400_000));
    expect(model.birthDateError).toBe(OnboardingText.birthDateInFuture);
    const date = new Date(1_700_000_000_000 - 86_400_000);
    model.setBirthDate(date);
    expect(model.birthDateError).toBeNull();
    expect(model.options.birthHeight).toBe(birthHeightFor(date, "mainnet"));
    model.setBirthDate(null);
    expect(model.options.birthHeight).toBe(0);
    model.setBirthHeight(null);
    expect(model.options.birthHeight).toBe(0);
    // Never NaN (it would reach the engine as `null`: the tip, a scan that misses everything).
    for (const bad of [Number.NaN, -5, 1.5, Infinity]) {
      model.setBirthHeight(bad);
      expect(model.options.birthHeight).toBe(0);
    }
    model.setBirthHeight(1_234);
    model.setBirthDate(new Date(Number.NaN));
    expect(model.options.birthHeight).toBe(0);
  });

  it("theScanStartSurvivesGoingBackAndForth", async () => {
    // Review r1 #8: the options step remounts; its choice and the typed height stay.
    const model = make(unlockedWorld());
    await model.startRestore();
    await model.updateRestoreText(PHRASE);
    model.continueRestore();
    model.setScanStart("height");
    model.setScanHeight("1234.7");
    model.setBip39Passphrase("25th word");
    model.back();
    model.continueRestore();
    expect(model.scanStart).toBe("height");
    expect(model.scanHeight).toBe("1234.7");
    expect(model.options.birthHeight).toBe(1234);
    expect(model.bip39Passphrase).toBe("25th word");
    model.setScanHeight("abc");
    expect(model.options.birthHeight).toBe(0);
    model.setScanStart("genesis");
    expect(model.options.birthHeight).toBe(0);
  });

  it("todayIsNotInTheFuture", async () => {
    // Review r1 #14: the date field's text is a local date; `valueAsDate` (UTC midnight) made
    // today "future" east of UTC early in the day.
    const world = unlockedWorld();
    const now = new Date(2026, 9, 14, 0, 30); // 00:30 local
    const model = new OnboardingModel(world, "mainnet", { random: seeded(42n), now: () => now });
    await model.startRestore();
    await model.updateRestoreText(PHRASE);
    model.continueRestore();
    model.setScanStart("date");
    model.setScanDate("2026-10-14");
    expect(model.birthDateError).toBeNull();
    model.setScanDate("2026-10-15");
    expect(model.birthDateError).not.toBeNull();
    model.setScanDate("");
    expect(model.birthDateError).toBeNull();
    expect(model.options.birthHeight).toBe(0);
  });

  it("birthHeightEstimateIsBelowTheRealHeight", () => {
    // A real checkpoint mined at least a week before the date, never an extrapolation (review r1
    // #1). Mainnet 1,900,000 was mined at 1_688_744_911 (2023-07-07).
    const day = 24 * 3600 * 1000;
    expect(birthHeightFor(new Date(1_688_744_911_000 + 8 * day), "mainnet")).toBe(1_900_000);
    expect(birthHeightFor(new Date(1_688_744_911_000 + 1 * day), "mainnet")).toBe(1_850_000);
    expect(birthHeightFor(new Date(0), "mainnet")).toBe(0);
    // Testnet's block rate varied by years. Testnet 1,000,000 was mined at 1_712_086_681.
    expect(birthHeightFor(new Date(1_712_086_681_000 + 8 * day), "testnet")).toBe(1_000_000);
    expect(birthHeightFor(new Date(1_712_086_681_000 + 6 * day), "testnet")).toBe(950_000);
    // Testnet stood near 941,877 on 2024-01-01 and 1,506,112 on 2026-07-01 (its headers).
    expect(birthHeightFor(new Date("2024-01-01"), "testnet")).toBeLessThanOrEqual(941_877);
    expect(birthHeightFor(new Date("2026-07-01"), "testnet")).toBeLessThanOrEqual(1_506_112);
    // After the last checkpoint: that checkpoint, not a guess.
    expect(birthHeightFor(new Date("2030-01-01"), "testnet")).toBe(1_500_000);
    expect(birthHeightFor(new Date(), "regtest")).toBe(0);
    expect(birthHeightFor(new Date(), "devnet")).toBe(0);
  });

  it("importFailureCanBeRetriedFromTheSameStep", async () => {
    const world = new FakeWorld();
    world.status = status({ state: "unencrypted", encrypted: false });
    world.importErrors = [appError("wallet_already_exists")];
    const model = make(world);
    await model.startRestore();
    await model.updateRestoreText(PHRASE);
    model.continueRestore();
    await model.finishRestore();
    expect(model.step).toBe("failed");
    model.back();
    expect(model.step).toBe("restoreOptions");
    await model.finishRestore();
    expect(model.step).toBe("done");
  });

  it("createAfterAbandonedRestoreUsesNewWalletOptions", async () => {
    const model = make(unlockedWorld());
    await model.startRestore();
    model.setBirthHeight(5);
    model.back();
    expect(model.step).toBe("welcome");
    await model.startCreate();
    expect(model.options.birthHeight).toBeNull();
  });

  it("passphraseStrengthScale", () => {
    expect(passphraseStrength("")).toBe("none");
    expect(passphraseStrength("short")).toBe("weak");
    expect(passphraseStrength("lowercase")).toBe("weak");
    expect(passphraseStrength("Lower1234")).toBe("fair");
    expect(passphraseStrength("Correct-Horse-Battery-9-Staple")).toBe("strong");
  });
});
