// Settings ▸ Security's view models (QT-111, IOS-009, IOS-015, IOS-016, IOS-108, IOS-109): the
// Swift `SecurityViewModel` and `AskPassphraseDialog` tests, ported by parity ID, over the
// fixture backend (the engine's rules as src/fixture models them).

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IpcBackend } from "../api/tauri";
import { installFixture, type FixtureHandle } from "../fixture";
import { FIXTURE_PASSPHRASE as PASS, SECOND_WALLET_ID, type ScenarioName } from "../fixture/scenarios";
import { LockText } from "../onboarding/lock";
import { WRONG_PASSPHRASE } from "../onboarding/model";
import { PassphraseText } from "../onboarding/passphrase";
import { AUTO_LOCK_CHOICES, ChangePassphraseModel, EncryptModel, PaymentAuthModel, SecurityText, WipeModel } from "./security";

let fixture: FixtureHandle;
let b: IpcBackend;
function install(scenario: ScenarioName) {
  fixture = installFixture({ scenario });
  b = new IpcBackend("fixture", () => fixture.now, fixture.locale);
}
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  install("funded");
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("encrypt wallet", () => {
  it("QT111_encryptWallet", async () => {
    install("unencrypted");
    const model = new EncryptModel(b);
    await model.encrypt("new-pass", "other");
    expect(model.error).toBe(PassphraseText.mismatch);
    await model.encrypt("", "");
    expect(model.error).toBe(PassphraseText.empty);
    await model.encrypt("new-pass", "new-pass");
    expect(model.step).toBe("done");
    // The engine leaves the vault locked; the new passphrase unlocks it.
    expect(await b.lockStatus()).toMatchObject({ state: "locked", encrypted: true });
    expect((await b.unlock("new-pass")).state).toBe("unlocked");
  });

  it("QT111_alreadyEncryptedIsRefused", async () => {
    const model = new EncryptModel(b);
    await model.encrypt("x", "x");
    expect(model.step).toBe("form");
    expect(model.error).toBe("The wallet is already encrypted.");
    expect(fixture.world.passphrase).toBe(PASS);
  });
});

describe("change passphrase", () => {
  it("QT111_changePassphrase", async () => {
    const model = new ChangePassphraseModel(b);
    await model.load();
    await model.change(PASS, "next", "nxet");
    expect(model.message).toBe(PassphraseText.mismatch);
    // The new passphrase is checked first: no vault attempt is counted for it.
    expect(fixture.world.lock.failedAttempts).toBe(0);
    await model.change(PASS, PASS, PASS);
    expect(model.message).toBe(SecurityText.sameAsOld);
    await model.change(PASS, "next", "next");
    expect(model.done).toBe(true);
    expect(fixture.world.passphrase).toBe("next");
  });

  it("QT111_aWrongCurrentPassphraseCountsAsAnAttempt", async () => {
    const model = new ChangePassphraseModel(b);
    await model.load();
    await model.change("wrong", "next", "next");
    expect(model.done).toBe(false);
    expect(model.message).toBe(`${WRONG_PASSPHRASE} ${LockText.attemptsRemaining(7)}`);
    expect(fixture.world.passphrase).toBe(PASS);
    model.edited();
    expect(model.message).toBe(`${WRONG_PASSPHRASE} ${LockText.attemptsRemaining(7)}`);
  });
});

describe("payment authentication", () => {
  it("IOS016_requireAuthenticationAndAutohideBalance", async () => {
    const model = new PaymentAuthModel(b);
    await model.load();
    expect(model.on).toBe(true);
    model.ask(false);
    expect(model.pending).toBe(false);
    await model.confirm("wrong");
    expect(model.on).toBe(true);
    expect(model.lock.message).toContain(WRONG_PASSPHRASE);
    await model.confirm(PASS);
    expect(model.on).toBe(false);
    expect(model.pending).toBeNull();
    expect(await b.paymentAuthentication()).toBe(false);
    // Asking for the value it has asks nothing.
    model.ask(false);
    expect(model.pending).toBeNull();
  });
});

describe("auto lock", () => {
  it("IOS015_autoLockDefaultsToNeverAndIsSet", () => {
    expect(AUTO_LOCK_CHOICES[0]).toEqual({ secs: null, label: "Never" });
    // iOS's "Immediately" has no desktop meaning (D12).
    expect(AUTO_LOCK_CHOICES.map((c) => c.secs)).toEqual([null, 60, 300, 3600, 86_400]);
  });
});

describe("wipe", () => {
  it("IOS009_existingDataPromptAndDeleteAllWithTypedSentence", async () => {
    install("two-wallets");
    const model = new WipeModel(b);
    await model.load();
    model.accept("I accept");
    expect(model.error).toBe(SecurityText.acceptPhraseMismatch);
    expect(model.step).toBe("sentence");
    model.accept(`  ${SecurityText.wipeAcceptPhrase} `);
    expect(model.step).toBe("passphrase");
    await model.wipe("wrong");
    expect(model.step).toBe("passphrase");
    expect(fixture.world.wallets).toHaveLength(2);
    await model.wipe(PASS);
    expect(model.step).toBe("done");
    expect(fixture.world.wallets).toEqual([]);
    expect(fixture.world.lock.state).toBe("noVault");
  });

  it("IOS109_wipeRemovesClosedWalletsToo", async () => {
    install("two-wallets");
    await b.unloadWallet(SECOND_WALLET_ID);
    const model = new WipeModel(b);
    await model.load();
    model.accept(SecurityText.wipeAcceptPhrase);
    await model.wipe(PASS);
    expect(model.step).toBe("done");
    expect(fixture.world.wallets).toEqual([]);
  });

  it("IOS109_aFailedRemovalStopsBeforeTheVaultIsDestroyed", async () => {
    install("two-wallets");
    fixture.fail("remove_wallet", "none");
    const model = new WipeModel(b);
    await model.load();
    model.accept(SecurityText.wipeAcceptPhrase);
    await model.wipe(PASS);
    expect(model.step).toBe("stopped");
    expect(model.error).toMatch(/^Main could not be removed: .* Nothing was deleted\.$/);
    expect(fixture.calls).not.toContain("destroy_vault");
    expect(fixture.world.lock.state).toBe("unlocked");
  });

  it("IOS109_aClosedWalletThatCannotOpenStopsTheWipeBeforeAnyRemoval", async () => {
    install("two-wallets");
    await b.unloadWallet(SECOND_WALLET_ID);
    fixture.fail("load_wallet", "none");
    const model = new WipeModel(b);
    await model.load();
    model.accept(SecurityText.wipeAcceptPhrase);
    await model.wipe(PASS);
    expect(model.step).toBe("stopped");
    expect(model.error).toMatch(/Nothing was deleted\.$/);
    expect(fixture.calls).not.toContain("remove_wallet");
    expect(fixture.world.wallets).toHaveLength(2);
  });

  it("IOS109_aStoppedWipeSaysWhatItAlreadyRemoved", () => {
    expect(SecurityText.wipeStopped("Main", "Busy.", 0)).toBe("Main could not be removed: Busy. Nothing was deleted.");
    expect(SecurityText.wipeStopped("C", "Busy.", 1)).toBe("C could not be removed: Busy. The wallet before it was removed; the others and the vault were kept.");
    expect(SecurityText.wipeStopped("D", "Busy.", 2)).toMatch(/The 2 wallets before it were removed;/);
  });

  it("IOS109_anUnencryptedVaultNeedsNoPassphrase", async () => {
    install("unencrypted");
    const model = new WipeModel(b);
    await model.load();
    expect(model.encrypted).toBe(false);
    model.accept(SecurityText.wipeAcceptPhrase);
    await model.wipe("");
    expect(model.step).toBe("done");
    expect(fixture.world.lock.state).toBe("noVault");
  });
});
